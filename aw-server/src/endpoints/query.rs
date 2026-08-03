use rocket::http::Status;
use rocket::serde::json::{json, Json, Value};
use rocket::State;

use aw_models::Query;
use aw_query::QueryError;

use crate::endpoints::{HttpErrorJson, ServerState};

fn query_error_status(e: &QueryError) -> Status {
    // All QueryError variants represent bad user input (invalid query syntax,
    // invalid regex, undefined variables, wrong types, etc.). None of them
    // indicate a server-side fault, so return 400 Bad Request instead of 500.
    match e {
        QueryError::ParsingError(_)
        | QueryError::EmptyQuery()
        | QueryError::VariableNotDefined(_)
        | QueryError::MathError(_)
        | QueryError::InvalidType(_)
        | QueryError::InvalidFunctionParameters(_)
        | QueryError::TimeIntervalError(_)
        | QueryError::BucketQueryError(_)
        | QueryError::RegexCompileError(_) => Status::BadRequest,
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
