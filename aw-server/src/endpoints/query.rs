use rocket::http::Status;
use rocket::serde::json::{json, Json, Value};
use rocket::State;

use aw_models::Query;

use crate::endpoints::{HttpErrorJson, ServerState};

#[post("/", data = "<query_req>", format = "application/json")]
pub fn query(query_req: Json<Query>, state: &State<ServerState>) -> Result<Value, HttpErrorJson> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    let datastore = endpoints_get_lock!(state.datastore);
    for interval in intervals {
        let result = match aw_query::query(&query_code, interval, &datastore) {
            Ok(data) => data,
            Err(e) => {
                warn!("Query failed: {:?}", e);
                return Err(HttpErrorJson::new(
                    Status::InternalServerError,
                    e.to_string(),
                ));
            }
        };
        results.push(result);
    }
    Ok(json!(results))
}
