use rocket::State;
use rocket::Response;
use rocket::http::Status;
use rocket_contrib::Json;

use std::collections::HashMap;

use endpoints::ServerState;
use models::Bucket;

macro_rules! response_status {
    ($status:expr) => ({
        let mut res = Response::new();
        res.set_status($status);
        res
    })
}

#[derive(Clone,Serialize,Deserialize)]
#[serde(untagged)]
pub enum ImportFormat {
    Single(Bucket),
    Multiple(HashMap<String, Bucket>),
}

#[post("/", data = "<json_data>", format = "application/json")]
pub fn bucket_import(state: State<ServerState>, json_data: Json<ImportFormat>) -> Response {
    match json_data.into_inner() {
        ImportFormat::Single(bucket) => match state.datastore.create_bucket(&bucket) {
            Ok(_) => response_status!(Status::Ok),
            Err(_) => response_status!(Status::InternalServerError)
        },
        ImportFormat::Multiple(buckets) => {
            let mut result = Status::Ok;
            for (_bucketname, bucket) in buckets {
                match state.datastore.create_bucket(&bucket) {
                    Ok(_) => (),
                    Err(_) => result = Status::InternalServerError,
                }
            }
            response_status!(result)
        }
    }
}

