use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::State;
use rocket_contrib::json::Json;
use std::sync::MutexGuard;

use aw_datastore::Datastore;
use aw_models::{Key, KeyValue};

use crate::endpoints::HttpErrorJson;

fn parse_key(key: String) -> Result<String, HttpErrorJson> {
    let namespace: String = "settings.".to_string();
    if key.len() >= 128 {
        Err(HttpErrorJson::new(
            Status::BadRequest,
            "Too long key".to_string(),
        ))
    } else {
        Ok(namespace + key.as_str())
    }
}

#[post("/", data = "<message>", format = "application/json")]
pub fn setting_set(
    state: State<ServerState>,
    message: Json<KeyValue>,
) -> Result<Status, HttpErrorJson> {
    let data = message.into_inner();

    let setting_key = parse_key(data.key)?;

    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.insert_key_value(&setting_key, &data.value.to_string());

    match result {
        Ok(_) => Ok(Status::Created),
        Err(err) => Err(err.into()),
    }
}

#[get("/")]
pub fn settings_list_get(state: State<ServerState>) -> Result<Json<Vec<Key>>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    let queryresults = match datastore.get_keys_starting("settings.%") {
        Ok(result) => Ok(result),
        Err(err) => Err(err.into()),
    };

    let mut output = Vec::<Key>::new();
    for i in queryresults? {
        output.push(Key { key: i });
    }

    Ok(Json(output))
}

#[get("/<key>")]
pub fn setting_get(
    state: State<ServerState>,
    key: String,
) -> Result<Json<KeyValue>, HttpErrorJson> {
    let setting_key = parse_key(key)?;

    let datastore = endpoints_get_lock!(state.datastore);

    match datastore.get_key_value(&setting_key) {
        Ok(result) => Ok(Json(result)),
        Err(err) => Err(err.into()),
    }
}

#[delete("/<key>")]
pub fn setting_delete(state: State<ServerState>, key: String) -> Result<(), HttpErrorJson> {
    let setting_key = parse_key(key)?;

    let datastore = endpoints_get_lock!(state.datastore);
    let result = datastore.delete_key_value(&setting_key);

    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(err.into()),
    }
}
