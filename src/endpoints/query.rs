use rocket::State;
use rocket::http::Status;
use rocket_contrib::json::Json;

use query;
use models::Query;
use endpoints::ServerState;

#[post("/", data = "<query_req>")]
pub fn query(query_req: Json<Query>, state: State<ServerState>) -> Result<Json<Vec<query::DataType>>, Status> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    for interval in intervals {
        let result = match query::query(&query_code, &interval, &state.datastore) {
            Ok(data) => data,
            Err(e) => {
                println!("{:?}", e);
                // TODO: Respond with a error message in the body
                return Err(Status::InternalServerError);
            }
        };
        results.push(result);
    }
    Ok(Json(results))
}
