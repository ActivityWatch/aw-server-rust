use rocket::http::Status;
use rocket::State;
use rocket_contrib::json::Json;

use aw_models::Query;

use crate::endpoints::{http_err, http_ok, HttpResponse, ServerState};

#[post("/", data = "<query_req>", format = "application/json")]
pub fn query(query_req: Json<Query>, state: State<ServerState>) -> HttpResponse {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    for interval in intervals {
        // Cannot re-use endpoints_get_lock!() here because it returns Err(Status) on failure and this
        // function returns HttpResponse
        let datastore = match state.datastore.lock() {
            Ok(ds) => ds,
            Err(e) => {
                warn!("Taking datastore lock failed, returning 500: {}", e);
                return http_err(
                    Status::ServiceUnavailable,
                    "Taking datastore lock failed, see aw-server logs".to_string(),
                );
            }
        };
        let result = match aw_query::query(&query_code, &interval, &datastore) {
            Ok(data) => data,
            Err(e) => {
                warn!("Query failed: {:?}", e);
                return http_err(Status::InternalServerError, e.to_string());
            }
        };
        results.push(result);
    }
    http_ok(json!(results))
}
