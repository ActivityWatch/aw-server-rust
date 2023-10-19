use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use std::collections::HashMap;
use std::sync::MutexGuard;

use aw_datastore::{Datastore, DatastoreError};

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

#[get("/")]
pub fn settings_get(
    state: &State<ServerState>,
) -> Result<Json<HashMap<String, serde_json::Value>>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    let queryresults = match datastore.get_key_values("settings.%") {
        Ok(result) => Ok(result),
        Err(err) => Err(err.into()),
    };

    match queryresults {
        Ok(settings) => {
            // strip 'settings.' prefix from keys
            let mut map: HashMap<String, serde_json::Value> = HashMap::new();
            for (key, value) in settings.iter() {
                map.insert(
                    key.strip_prefix("settings.").unwrap_or(key).to_string(),
                    serde_json::from_str(value.clone().as_str()).unwrap(),
                );
            }
            Ok(Json(map))
        }
        Err(err) => Err(err),
    }
}

#[get("/<key>")]
pub fn setting_get(
    state: &State<ServerState>,
    key: String,
) -> Result<Json<serde_json::Value>, HttpErrorJson> {
    let setting_key = parse_key(key)?;
    let datastore = endpoints_get_lock!(state.datastore);

    match datastore.get_key_value(&setting_key) {
        Ok(value) => Ok(Json(serde_json::from_str(&value).unwrap())),
        Err(DatastoreError::NoSuchKey(_)) => Ok(Json(serde_json::from_str("null").unwrap())),
        Err(err) => Err(err.into()),
    }
}

#[post("/<key>", data = "<value>", format = "application/json")]
pub fn setting_set(
    state: &State<ServerState>,
    key: String,
    value: Json<serde_json::Value>,
) -> Result<Status, HttpErrorJson> {
    let setting_key = parse_key(key)?;
    let value_str = match serde_json::to_string(&value.0) {
        Ok(value) => value,
        Err(err) => {
            return Err(HttpErrorJson::new(
                Status::BadRequest,
                format!("Invalid JSON: {}", err),
            ))
        }
    };

    let datastore: MutexGuard<'_, Datastore> = endpoints_get_lock!(state.datastore);
    let result = datastore.set_key_value(&setting_key, &value_str);

    match result {
        Ok(_) => Ok(Status::Created),
        Err(err) => Err(err.into()),
    }
}

#[delete("/<key>")]
pub fn setting_delete(state: &State<ServerState>, key: String) -> Result<(), HttpErrorJson> {
    let setting_key = parse_key(key)?;

    let datastore = endpoints_get_lock!(state.datastore);
    let result = datastore.delete_key_value(&setting_key);

    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(err.into()),
    }
}
