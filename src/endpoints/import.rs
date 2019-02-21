use rocket::State;
use rocket::http::Status;
use rocket_contrib::json::Json;

use std::collections::HashMap;

use endpoints::ServerState;
use models::Bucket;

#[derive(Clone,Serialize,Deserialize)]
#[serde(untagged)]
pub enum ImportFormat {
    Single(Bucket),
    Multiple(HashMap<String, Bucket>),
}

#[post("/", data = "<json_data>", format = "application/json")]
pub fn bucket_import(state: State<ServerState>, json_data: Json<ImportFormat>) -> Result<(), Status> {
    match json_data.into_inner() {
        ImportFormat::Single(bucket) => match endpoints_get_lock!(state.datastore).create_bucket(&bucket) {
            Ok(_) => (),
            Err(e) => {
                warn!("Failed to import bucket: {:?}", e);
                return Err(Status::InternalServerError)
            }
        },
        ImportFormat::Multiple(buckets) => {
            for (_bucketname, bucket) in buckets {
                match endpoints_get_lock!(state.datastore).create_bucket(&bucket) {
                    Ok(_) => (),
                    Err(e) => {
                        warn!("Failed to import bucket: {:?}", e);
                        return Err(Status::InternalServerError)
                    },
                }
            }
        }
    }
    Ok(())
}

