use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use std::collections::HashMap;

use aw_datastore::DatastoreError;

use crate::endpoints::HttpErrorJson;

/// Map a settings API key to the datastore key (`settings.<key>`).
///
/// Dots are allowed: `GET /api/0/settings/foo.bar` stores and retrieves
/// `settings.foo.bar`. Keys of length >= 128 are rejected (same rule as the
/// HTTP handler). Android JNI `getSetting` uses this helper so native callers
/// cannot silently miss a value the HTTP API would return.
pub(crate) fn settings_datastore_key(key: &str) -> Result<String, &'static str> {
    if key.len() >= 128 {
        Err("Too long key")
    } else {
        Ok(format!("settings.{}", key))
    }
}

fn parse_key(key: String) -> Result<String, HttpErrorJson> {
    settings_datastore_key(&key)
        .map_err(|msg| HttpErrorJson::new(Status::BadRequest, msg.to_string()))
}

#[cfg(test)]
mod key_tests {
    use super::settings_datastore_key;

    #[test]
    fn simple_keys_are_namespaced() {
        assert_eq!(
            settings_datastore_key("classes").unwrap(),
            "settings.classes"
        );
        assert_eq!(
            settings_datastore_key("startOfDay").unwrap(),
            "settings.startOfDay"
        );
    }

    #[test]
    fn dotted_keys_are_namespaced_not_rejected() {
        assert_eq!(
            settings_datastore_key("foo.bar").unwrap(),
            "settings.foo.bar"
        );
        assert_eq!(settings_datastore_key("a.b.c").unwrap(), "settings.a.b.c");
    }

    #[test]
    fn overly_long_keys_are_rejected() {
        assert!(settings_datastore_key(&"a".repeat(128)).is_err());
        assert_eq!(
            settings_datastore_key(&"a".repeat(127)).unwrap(),
            format!("settings.{}", "a".repeat(127))
        );
    }
}

#[get("/")]
pub fn settings_get(
    state: &State<ServerState>,
) -> Result<Json<HashMap<String, serde_json::Value>>, HttpErrorJson> {
    let datastore = &state.datastore;
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
    let datastore = &state.datastore;

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

    let datastore = &state.datastore;
    let result = datastore.set_key_value(&setting_key, &value_str);

    match result {
        Ok(_) => {
            // Worker also reloads on SetKeyValue of this key; this second
            // RefreshPrivacyFilter is belt-and-suspenders so the HTTP path
            // still works if a future writer bypasses that hook.
            if setting_key == "settings.privacy_filters" {
                let _ = datastore.refresh_privacy_filter();
            }
            Ok(Status::Created)
        }
        Err(err) => Err(err.into()),
    }
}

#[delete("/<key>")]
pub fn setting_delete(state: &State<ServerState>, key: String) -> Result<(), HttpErrorJson> {
    let setting_key = parse_key(key)?;

    let datastore = &state.datastore;
    let result = datastore.delete_key_value(&setting_key);

    match result {
        Ok(_) => {
            if setting_key == "settings.privacy_filters" {
                let _ = datastore.refresh_privacy_filter();
            }
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
