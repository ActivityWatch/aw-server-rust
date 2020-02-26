use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::State;
use rocket_contrib::json::Json;
use std::sync::MutexGuard;

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Key, KeyValue};

fn parse_key(key: String) -> Result<String, Status> {
    let namespace: String = "settings.".to_string();
    if key.len() >= 128 {
        return Err(Status::BadRequest);
    } else {
        Ok(namespace + key.as_str())
    }
}

#[post("/", data = "<message>")]
pub fn setting_set(state: State<ServerState>, message: Json<KeyValue>) -> Result<Status, Status> {
    let data = message.into_inner();

    let setting_key = parse_key(data.key)?;

    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.insert_key_value(&setting_key, &data.value);
    return match result {
        Ok(_) => Ok(Status::Created),
        Err(err) => {
            warn!("Unexpected error when creating setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}

#[get("/")]
pub fn settings_list_get(state: State<ServerState>) -> Result<Json<Vec<Key>>, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    let queryresults = match datastore.get_keys_starting("settings.%") {
        Ok(result) => Ok(result),
        Err(DatastoreError::NoSuchKey) => Err(Status::NotFound),
        Err(err) => {
            warn!("Unexpected error when getting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };

    let mut output = Vec::<Key>::new();
    for i in queryresults? {
        output.push(Key { key: i });
    }
    return Ok(Json(output));
}

#[get("/<key>")]
pub fn setting_get(state: State<ServerState>, key: String) -> Result<Json<KeyValue>, Status> {
    let setting_key = parse_key(key)?;

    let datastore = endpoints_get_lock!(state.datastore);
    return match datastore.get_key_value(&setting_key) {
        Ok(result) => Ok(Json(result)),
        Err(DatastoreError::NoSuchKey) => Err(Status::NotFound),
        Err(err) => {
            warn!("Unexpected error when getting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}

#[delete("/<key>")]
pub fn setting_delete(state: State<ServerState>, key: String) -> Result<(), Status> {
    let setting_key = parse_key(key)?;

    let datastore = endpoints_get_lock!(state.datastore);
    let result = datastore.delete_key_value(&setting_key);
    return match result {
        Ok(_) => Ok(()),
        Err(err) => {
            warn!("Unexpected error when deleting setting: {:?}", err);
            Err(Status::InternalServerError)
        }
    };
}
