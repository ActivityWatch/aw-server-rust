use rocket::State;
use rocket::http::Status;
use rocket::response::status;
use rocket_contrib::json::Json;
use rocket_contrib::json::JsonValue;

use query;
use query::QueryError;
use models::Query;
use endpoints::ServerState;

#[derive(Serialize)]
struct QueryErrorJson {
    status: u16,
    reason: String,
    message: String
}

/* TODO: Slightly ugly code with ok() and error() */

fn ok(data: Vec<query::DataType>) -> status::Custom<JsonValue> {
    status::Custom(Status::Ok, json!(data))
}

fn error(err: QueryError) -> status::Custom<JsonValue> {
    let body = QueryErrorJson {
        status: 500,
        reason: "Internal Server Error (Query Error)".to_string(),
        message: format!("{}", err)
    };
    status::Custom(Status::InternalServerError, json!(body))
}

#[post("/", data = "<query_req>")]
pub fn query(query_req: Json<Query>, state: State<ServerState>) -> status::Custom<JsonValue> {
    let query_code = query_req.0.query.join("\n");
    let intervals = &query_req.0.timeperiods;
    let mut results = Vec::new();
    for interval in intervals {
        let result = match query::query(&query_code, &interval, &state.datastore) {
            Ok(data) => data,
            Err(e) => {
                println!("{:?}", e);
                return error(e);
            }
        };
        results.push(result);
    }
    ok(results)
}
