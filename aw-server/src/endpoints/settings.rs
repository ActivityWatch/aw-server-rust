use std::sync::MutexGuard;
use rocket_contrib::json::Json;
use rocket::http::Status;
use rocket::State;
use crate::endpoints::ServerState;

use aw_datastore::{Datastore, DatastoreError};

#[post("/<key>", data="<message>")]
pub fn setting_new(state: State<ServerState>, key: String, message: Json<String>)
    -> Result<Status, Status> {

    if key.len() >= 128 { return Err(Status::BadRequest) };;

    let data = message.into_inner();
    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.insert_value(&key, &data);
    return match result {
        // TODO: Different status for replacement / creation (requires some sql adjustment)
        Ok(_) => Ok(Status::Created),
        Err(err) => {
            warn!("Unexpected error when creating value: {:?}", err);
            Err(Status::InternalServerError)
        }
    }
}

#[get("/<key>")]
pub fn setting_get(state: State<ServerState>, key: String) -> Result<String, Status> {
    if key.len() >= 128 { return Err(Status::BadRequest) };;

    let datastore = endpoints_get_lock!(state.datastore);
    return match datastore.get_value(&key) {
        Ok(result) => Ok(result),
        Err(DatastoreError::NoSuchValue) => Err(Status::NotFound),
        Err(err) => {
            warn!("Unexpected error when getting value: {:?}", err);
            Err(Status::InternalServerError)
        }
    }
}

#[delete("/<key>")]
pub fn setting_delete(state: State<ServerState>, key: String) -> Result<(), Status> {
    if key.len() > 128 { return Err(Status::BadRequest) };;

    let datastore = endpoints_get_lock!(state.datastore);
    let result = datastore.delete_value(&key);
    return match result {
        Ok(_) => Ok(()),
        Err(err) => {
            warn!("Unexpected error when deleting value: {:?}", err);
            Err(Status::InternalServerError)
        }
    }
}