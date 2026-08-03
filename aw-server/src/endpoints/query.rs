use rocket::http::Status;
use rocket::serde::json::{json, Json, Value};
use rocket::State;

use aw_models::Query;
use aw_query::QueryError;

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

#[post("/", data = "<query_req>", format = "application/json")]
pub fn query(query_req: Json<Query>, state: &State<ServerState>) -> Result<Value, HttpErrorJson> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    let datastore = &state.datastore;
    for interval in intervals {
        let result = match aw_query::query(&query_code, interval, datastore) {
            Ok(data) => data,
            Err(e) => {
                warn!("Query failed: {:?}", e);
                return Err(HttpErrorJson::new(query_error_status(&e), e.to_string()));
            }
        };
        results.push(result);
    }
    Ok(json!(results))
}
