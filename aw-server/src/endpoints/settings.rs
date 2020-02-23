use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::State;
use rocket_contrib::json::{Json, JsonValue};
use std::sync::MutexGuard;

use aw_datastore::{Datastore, DatastoreError};

fn parse_key(key: String) -> Result<String, Status> {
    let namespace: String = "settings.".to_string();
    if key.len() >= 128 {
        return Err(Status::BadRequest);
    } else {
        Ok(namespace + key.as_str())
    }
}

#[post("/<key>", data = "<message>")]
pub fn setting_new(
    state: State<ServerState>,
    key: String,
    message: Json<String>,
) -> Result<Status, Status> {
    let setting_key = match parse_key(key) {
        Ok(k) => k,
        Err(err) => return Err(err),
    };
    let data = message.into_inner();
    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.insert_value(&setting_key, &data);
    return match result {
        // TODO: Different status for replacement / creation (requires some sql adjustment)
        Ok(_) => Ok(Status::Created),
        Err(err) => {
            warn!("Unexpected error when creating setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}

#[get("/")]
pub fn settings_list_get(
    state: State<ServerState>,
) -> Result<JsonValue, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    return match datastore.get_values_starting("settings.") {
        Ok(result) => Ok(json!(result)),
        Err(DatastoreError::NoSuchValue) => Err(Status::NotFound),
        Err(err) => {
            warn!("Unexpected error when getting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}

#[get("/<key>")]
pub fn setting_get(state: State<ServerState>, key: String) -> Result<String, Status> {
    let setting_key = match parse_key(key) {
        Ok(k) => k,
        Err(err) => return Err(err),
    };
    let datastore = endpoints_get_lock!(state.datastore);
    return match datastore.get_value(&setting_key) {
        Ok(result) => Ok(result),
        Err(DatastoreError::NoSuchValue) => Err(Status::NotFound),
        Err(err) => {
            warn!("Unexpected error when getting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}

#[delete("/<key>")]
pub fn setting_delete(state: State<ServerState>, key: String) -> Result<(), Status> {
    let setting_key = match parse_key(key) {
        Ok(k) => k,
        Err(err) => return Err(err),
    };
    let datastore = endpoints_get_lock!(state.datastore);
    let result = datastore.delete_value(&setting_key);
    return match result {
        Ok(_) => Ok(()),
        Err(err) => {
            warn!("Unexpected error when deleting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}
