use std::sync::Arc;

use rocket::http::Status;
use rocket::response::content::RawJson;
use rocket::serde::json::Json;
use rocket::State;

use aw_models::Query;
use aw_query::QueryError;

use crate::endpoints::query_cache::CacheKey;
use crate::endpoints::{HttpErrorJson, ServerState};

fn query_error_status(e: &QueryError) -> Status {
    match e {
        // BucketQueryError also wraps datastore failures, which are server
        // errors rather than malformed client queries.
        QueryError::BucketQueryError(_) => Status::InternalServerError,
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
    query_req: Json<Query>,
    cache: Option<bool>,
    state: &State<ServerState>,
) -> Result<RawJson<String>, HttpErrorJson> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let use_cache = state.query_cache_enabled && cache.unwrap_or(true);
    let datastore = &state.datastore;

    let evaluate = |interval: &aw_models::TimeInterval| -> Result<Arc<str>, HttpErrorJson> {
        match aw_query::query(&query_code, interval, datastore) {
            Ok(data) => match serde_json::to_string(&data) {
                Ok(serialized) => Ok(Arc::from(serialized.as_str())),
                Err(e) => {
                    warn!("Failed to serialize query result: {e}");
                    Err(HttpErrorJson::new(
                        Status::InternalServerError,
                        e.to_string(),
                    ))
                }
            },
            Err(e) => {
                warn!("Query failed: {:?}", e);
                Err(HttpErrorJson::new(query_error_status(&e), e.to_string()))
            }
        }
    };

    let mut bodies: Vec<Arc<str>> = Vec::with_capacity(intervals.len());
    for interval in intervals {
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
