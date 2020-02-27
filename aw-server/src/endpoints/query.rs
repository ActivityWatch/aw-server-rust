use rocket::http::Status;
use rocket::response::status;
use rocket::State;
use rocket_contrib::json::{Json, JsonValue};

use aw_models::Query;

use crate::endpoints::ServerState;
use aw_query;
use aw_query::QueryError;

#[derive(Serialize)]
struct QueryErrorJson {
    status: u16,
    reason: String,
    message: String,
}

/* TODO: Slightly ugly code with ok() and error() */

fn ok(data: Vec<aw_query::DataType>) -> status::Custom<JsonValue> {
    status::Custom(Status::Ok, json!(data))
}

fn error(err: QueryError) -> status::Custom<JsonValue> {
    let body = QueryErrorJson {
        status: 500,
        reason: "Internal Server Error (Query Error)".to_string(),
        message: format!("{}", err),
    };
    status::Custom(Status::InternalServerError, json!(body))
}

#[post("/", data = "<query_req>")]
pub fn query(query_req: Json<Query>, state: State<ServerState>) -> status::Custom<JsonValue> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    for interval in intervals {
        // Cannot re-use endpoints_get_lock!() here because it returns Err(Status) on failure and this
        // function returns status::Custom
        let datastore = match state.datastore.lock() {
            Ok(ds) => ds,
            Err(e) => {
                warn!("Taking datastore lock failed, returning 500: {}", e);
                let body = QueryErrorJson {
                    status: 504,
                    reason: "Service Unavailable".to_string(),
                    message: "Taking datastore lock failed, see aw-server logs".to_string(),
                };
                return status::Custom(Status::ServiceUnavailable, json!(body));
            }
        };
        let result = match aw_query::query(&query_code, &interval, &datastore) {
            Ok(data) => data,
            Err(e) => {
                warn!("Query failed: {:?}", e);
                return error(e);
            }
        };
        results.push(result);
    }
    ok(results)
}
