use crate::endpoints::ServerState;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use serde::{Deserialize, Serialize};
use crate::endpoints::HttpErrorJson;
use crate::config;

#[derive(Serialize, Deserialize)]
pub struct CorsConfig {
    pub cors: Vec<String>,
    pub cors_regex: Vec<String>,
    pub cors_allow_aw_chrome_extension: bool,
    pub cors_allow_all_mozilla_extension: bool,
    pub in_file: Vec<String>,
    #[serde(skip_deserializing)]
    pub needs_restart: bool,
}

#[get("/")]
pub fn cors_config_get(state: &State<ServerState>) -> Result<Json<CorsConfig>, HttpErrorJson> {
    let config = endpoints_get_lock!(state.config);
    let (_, missing_fields) = config::get_config_path(config.testing);
    let in_file = config::CORS_FIELDS
        .iter()
        .filter(|&&f| !missing_fields.contains(&f.to_string()))
        .map(|&f| f.to_string())
        .collect();
    Ok(Json(CorsConfig {
        cors: config.cors.clone(),
        cors_regex: config.cors_regex.clone(),
        cors_allow_aw_chrome_extension: config.cors_allow_aw_chrome_extension,
        cors_allow_all_mozilla_extension: config.cors_allow_all_mozilla_extension,
        in_file,
        needs_restart: true,
    }))
}

#[post("/", data = "<new_cors>")]
pub fn cors_config_set(
    state: &State<ServerState>,
    new_cors: Json<CorsConfig>,
) -> Result<Status, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);

    // Identify which fields are allowed to be modified (those missing from the TOML file)
    let (_, missing_fields) = {
        let config = endpoints_get_lock!(state.config);
        config::get_config_path(config.testing)
    };

    // Validate exact origins before persisting
    if missing_fields.contains(&"cors".to_string()) {
        for origin in &new_cors.cors {
            if !origin.starts_with("http://") && !origin.starts_with("https://") {
                return Err(HttpErrorJson::new(
                    Status::BadRequest,
                    format!("Invalid CORS origin: {}. Must start with 'http://' or 'https://'", origin),
                ));
            }
        }
    }

    // Validate regular expressions before persisting
    if missing_fields.contains(&"cors_regex".to_string()) {
        for pattern in &new_cors.cors_regex {
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(HttpErrorJson::new(
                    Status::BadRequest,
                    format!("Invalid regular expression in CORS settings: {}. Error: {}", pattern, e),
                ));
            }
        }
    }

    let fields = [
        ("cors", serde_json::to_string(&new_cors.cors).unwrap()),
        ("cors_regex", serde_json::to_string(&new_cors.cors_regex).unwrap()),
        (
            "cors_allow_aw_chrome_extension",
            serde_json::to_string(&new_cors.cors_allow_aw_chrome_extension).unwrap(),
        ),
        (
            "cors_allow_all_mozilla_extension",
            serde_json::to_string(&new_cors.cors_allow_all_mozilla_extension).unwrap(),
        ),
    ];

    for (field, value_str) in fields {
        // Only save to datastore if the field is not fixed in the config file
        if missing_fields.contains(&field.to_string()) {
            let key = format!("cors.{}", field);
            datastore.set_key_value(&key, &value_str).map_err(|e| {
                HttpErrorJson::new(
                    Status::InternalServerError,
                    format!("Failed to save {}: {:?}", field, e),
                )
            })?;
        }
    }

    // Update the in-memory config for permitted fields so that GET reflect the changes immediately
    {
        let mut config = endpoints_get_lock!(state.config);
        if missing_fields.contains(&"cors".to_string()) {
            config.cors = new_cors.cors.clone();
        }
        if missing_fields.contains(&"cors_regex".to_string()) {
            config.cors_regex = new_cors.cors_regex.clone();
        }
        if missing_fields.contains(&"cors_allow_aw_chrome_extension".to_string()) {
            config.cors_allow_aw_chrome_extension = new_cors.cors_allow_aw_chrome_extension;
        }
        if missing_fields.contains(&"cors_allow_all_mozilla_extension".to_string()) {
            config.cors_allow_all_mozilla_extension = new_cors.cors_allow_all_mozilla_extension;
        }
    }

    Ok(Status::Ok)
}
