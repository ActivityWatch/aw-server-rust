use rocket_contrib::json::Json;

use rocket::State;
use rocket::http::Status;
use query as q;
use endpoints::ServerState;

use models::Query;

#[post("/", format = "application/json", data = "<query_req>")]
pub fn query(query_req: Json<Query>, state: State<ServerState>) -> Result<Json<Vec<q::DataType>>, Status> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut res = Vec::new();
    for interval in intervals {
        // TODO: don't unwrap
        res.push(q::query(&query_code, &interval, &state.datastore).unwrap());
    }
    Ok(Json(res))
}
