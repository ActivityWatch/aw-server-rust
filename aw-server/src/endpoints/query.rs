use std::sync::Arc;
use std::time::{Duration, Instant};

use rocket::http::Status;
use rocket::response::content::RawJson;
use rocket::State;

use aw_models::Query;
use aw_query::QueryError;

use crate::endpoints::query_cache::CacheKey;
use crate::endpoints::util::ApiJson;
use crate::endpoints::{HttpErrorJson, ServerState};

/// Queries slower than this are logged at `info` with their timeperiod and
/// size, so a server that gets bogged down by long-range queries leaves a
/// trace in the log (ActivityWatch/aw-server-rust#805).
const SLOW_QUERY_THRESHOLD: Duration = Duration::from_secs(1);

fn query_error_status(e: &QueryError) -> Status {
    match e {
        // BucketQueryError also wraps datastore failures, which are server
        // errors rather than malformed client queries.
        QueryError::BucketQueryError(_) => Status::InternalServerError,
        // The server gave up, not the client's fault: 503 tells it to retry
        // (with a shorter range), like the Python server's busy heartbeat.
        QueryError::TimeBudgetExceeded(_) => Status::ServiceUnavailable,
        QueryError::ParsingError(_)
        | QueryError::EmptyQuery()
        | QueryError::BucketNotFound(_)
        | QueryError::VariableNotDefined(_)
        | QueryError::MathError(_)
        | QueryError::InvalidType(_)
        | QueryError::InvalidFunctionParameters(_)
        | QueryError::TimeIntervalError(_)
        | QueryError::RegexCompileError(_) => Status::BadRequest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_error_status_distinguishes_client_and_server_errors() {
        assert_eq!(
            query_error_status(&QueryError::RegexCompileError("invalid regex".into())),
            Status::BadRequest
        );
        assert_eq!(
            query_error_status(&QueryError::BucketNotFound("missing bucket".into())),
            Status::BadRequest
        );
        assert_eq!(
            query_error_status(&QueryError::BucketQueryError("datastore failure".into())),
            Status::InternalServerError
        );
        assert_eq!(
            query_error_status(&QueryError::TimeBudgetExceeded("budget".into())),
            Status::ServiceUnavailable
        );
    }
}

/// Query endpoint.
///
/// Results for timeperiods that ended at least 10 minutes ago are served from
/// an in-memory cache unless disabled per request with `?cache=false` or
/// globally with `query_cache = false` in the config (see `query_cache`).
///
/// Results are serialized once, into the same string the cache stores, so a
/// cache miss does not pay a second full serialization for the response.
#[post("/?<cache>", data = "<query_req>", format = "application/json")]
pub fn query(
    query_req: ApiJson<Query>,
    cache: Option<bool>,
    state: &State<ServerState>,
) -> Result<RawJson<String>, HttpErrorJson> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let use_cache = state.query_cache_enabled && cache.unwrap_or(true);
    let datastore = state.query_datastore();
    let request_start = Instant::now();
    // The reader only sees committed data, and writes (heartbeats, but also
    // edits to old events and imports) are acknowledged before the writer's
    // batch commits. Flush first when any write since the last flush touched
    // one of the periods; untouched past periods, the bulk of a Year / All
    // time view, skip the fsync.
    if state.reader.is_some() {
        let periods: Vec<_> = intervals
            .iter()
            .map(|i| (i.start().to_owned(), i.end().to_owned()))
            .collect();
        if state.query_cache.take_pending_overlapping(&periods) {
            if let Err(e) = state.datastore.force_commit() {
                warn!("Failed to flush writes before query: {e:?}");
            }
        }
    }
    // One budget for the whole request: a client that sends many timeperiods
    // in one request is bounded the same as one that sends one long period.
    // An absurdly large budget that does not fit in an Instant means unlimited.
    let deadline = state
        .query_timeout
        .and_then(|budget| request_start.checked_add(budget));

    let evaluate = |interval: &aw_models::TimeInterval| -> Result<Arc<str>, HttpErrorJson> {
        let started = Instant::now();
        let result = aw_query::query_with_deadline(&query_code, interval, datastore, deadline);
        match result {
            Ok(data) => match serde_json::to_string(&data) {
                Ok(serialized) => {
                    // Serialization of a large result is part of the cost.
                    let elapsed = started.elapsed();
                    if elapsed >= SLOW_QUERY_THRESHOLD {
                        info!(
                            "Slow query ({:.1}s, {} bytes result) for timeperiod {interval}",
                            elapsed.as_secs_f64(),
                            serialized.len()
                        );
                    } else {
                        debug!(
                            "Query took {:.3}s for timeperiod {interval}",
                            elapsed.as_secs_f64()
                        );
                    }
                    Ok(Arc::from(serialized.as_str()))
                }
                Err(e) => {
                    warn!("Failed to serialize query result: {e}");
                    Err(HttpErrorJson::new(
                        Status::InternalServerError,
                        e.to_string(),
                    ))
                }
            },
            Err(e) => {
                warn!(
                    "Query failed after {:.1}s for timeperiod {interval}: {:?}",
                    started.elapsed().as_secs_f64(),
                    e
                );
                Err(HttpErrorJson::new(query_error_status(&e), e.to_string()))
            }
        }
    };

    // The budget also covers cache hits and response assembly: a request
    // that repeats a large cached period many times is bounded too.
    let budget_exceeded = |what: &str| -> HttpErrorJson {
        HttpErrorJson::new(
            Status::ServiceUnavailable,
            format!("query stopped before {what}: time budget exceeded"),
        )
    };
    let past_deadline = || deadline.is_some_and(|d| Instant::now() >= d);

    let mut bodies: Vec<Arc<str>> = Vec::with_capacity(intervals.len());
    for (i, interval) in intervals.iter().enumerate() {
        if past_deadline() {
            return Err(budget_exceeded(&format!(
                "timeperiod {} of {}",
                i + 1,
                intervals.len()
            )));
        }
        let period = (interval.start().to_owned(), interval.end().to_owned());
        if use_cache && state.query_cache.cacheable(period) {
            let key = CacheKey::new(&query_code, period);
            if let Some(cached) = state.query_cache.get(&key) {
                bodies.push(cached);
                continue;
            }
            // Record the write generation before evaluating, so a write that
            // lands mid-query can refuse the store (see query_cache).
            let generation = state.query_cache.generation();
            let serialized = evaluate(interval)?;
            state
                .query_cache
                .put(key, period, Arc::clone(&serialized), generation);
            bodies.push(serialized);
            continue;
        }
        bodies.push(evaluate(interval)?);
    }

    if past_deadline() {
        return Err(budget_exceeded("building the response"));
    }

    // Join the already-serialized results into the JSON array response body.
    // The parts come either from the cache or from a single serialization, so
    // no per-request `Value` copy is built.
    let mut body = String::with_capacity(2 + bodies.len() * 16);
    body.push('[');
    for (i, part) in bodies.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str(part);
    }
    body.push(']');
    Ok(RawJson(body))
}
