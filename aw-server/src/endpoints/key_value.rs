use std::sync::MutexGuard;
use rocket_contrib::json::Json;
use rocket::http::Status;
use rocket::State;
use crate::endpoints::ServerState;

use aw_datastore::{Datastore, DatastoreError};

#[post("/<key>", data="<message>")]
pub fn value_new(state: State<ServerState>, key: String, message: Json<String>)
    -> Result<Status, Status> {

    let data = message.into_inner();
    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.create_value(&key, &data);
    return match result {
        Ok(_) => Ok(Status::Created),
        Err(_) => Err(Status::InternalServerError)
    }
}

#[get("/<key>")]
pub fn value_get(state: State<ServerState>, key: String) -> Result<String, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    return match datastore.get_value(&key) {
        Ok(result) => Ok(result + "\n"),
        Err(DatastoreError::NoSuchValue) => Ok(Status::NoContent.to_string()),
        Err(_) => Err(Status::InternalServerError)
    }
}

#[delete("/<key>")]
pub fn value_delete(state: State<ServerState>, key: String) -> Result<(), Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    datastore.delete_value(&key).expect("Error deleting value {}");
    return Ok(())
}