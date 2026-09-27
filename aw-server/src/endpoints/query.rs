use std::sync::Arc;

use rocket::http::Status;
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
#[post("/?<cache>", data = "<query_req>", format = "application/json")]
pub fn query(
    query_req: Json<Query>,
    cache: Option<bool>,
    state: &State<ServerState>,
) -> Result<Json<Vec<Arc<aw_query::DataType>>>, HttpErrorJson> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let use_cache = state.query_cache_enabled && cache.unwrap_or(true);
    let mut results = Vec::new();
    let datastore = &state.datastore;
    for interval in intervals {
        if use_cache
            && state
                .query_cache
                .cacheable((interval.start().to_owned(), interval.end().to_owned()))
        {
            let period = (interval.start().to_owned(), interval.end().to_owned());
            let key = CacheKey::new(&query_code, period);
            if let Some(cached) = state.query_cache.get(&key) {
                results.push(cached);
                continue;
            }
            // Record the write generation before evaluating, so a write that
            // lands mid-query can refuse the store (see query_cache).
            let generation = state.query_cache.generation();
            let result = match aw_query::query(&query_code, interval, datastore) {
                Ok(data) => data,
                Err(e) => {
                    warn!("Query failed: {:?}", e);
                    return Err(HttpErrorJson::new(query_error_status(&e), e.to_string()));
                }
            };
            let result = Arc::new(result);
            state
                .query_cache
                .put(key, period, Arc::clone(&result), generation);
            results.push(result);
            continue;
        }
        let result = match aw_query::query(&query_code, interval, datastore) {
            Ok(data) => data,
            Err(e) => {
                warn!("Query failed: {:?}", e);
                return Err(HttpErrorJson::new(query_error_status(&e), e.to_string()));
            }
        };
        results.push(Arc::new(result));
    }
    // Serialize the results directly into the response body. Going through
    // json!() first built a full serde_json::Value copy of every event.
    Ok(Json(results))
}
