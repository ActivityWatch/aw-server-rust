use std::collections::HashMap;
use std::io::Cursor;

use rocket_contrib::json::Json;

use aw_models::Bucket;
use aw_models::BucketsExport;
use aw_models::Event;

use rocket::State;
use rocket::response::status;
use rocket::response::Response;
use rocket::http::Header;
use rocket::http::Status;

use crate::endpoints::ServerState;

use aw_datastore::DatastoreError;


#[get("/<key>")]
pub fn value_get(key: String) -> Result<Json(&str), Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.get_value(key) {
        Ok(result) => Ok(Json(result)),
        Err(e) => match e {
            _ => {
                warn!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<key>", data = "<message>")]
pub fn value_new(key: String, message: Json<String>) -> Result<(), Status>{
    let data = message.into_inner();

    let datastore = endpoints_get_lock!(state.datastore);
    
    let ret = datastore.create_value(key, data);
    match ret {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::Custom=> Err(Status::Custom(Status::NotModified, ())),
            _ => {
                Err(Status::InternalServerError)
            }
        }
    }
}
