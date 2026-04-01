use rocket::http::Method;
use rocket_cors::{AllowedHeaders, AllowedOrigins};
use aw_datastore::Datastore;
use std::sync::Mutex;

use crate::config::AWConfig;

pub fn cors(config: &AWConfig, datastore_mutex: &Mutex<Datastore>) -> rocket_cors::Cors {
    let root_url = format!("http://127.0.0.1:{}", config.port);
    let root_url_localhost = format!("http://localhost:{}", config.port);
    let mut allowed_exact_origins = vec![root_url, root_url_localhost];
    allowed_exact_origins.extend(config.cors.clone());

    let db = datastore_mutex.lock().unwrap();
    if let Ok(cors_origins_str) = db.get_key_value("settings.cors_origins") {

        let cors_origins: Vec<String> = cors_origins_str
            .trim_matches('"')
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .filter(|s| {
            let is_valid = s.starts_with("http://")
                        || s.starts_with("https://")
            if !is_valid {
                log::warn!("Ignoring invalid CORS origin: '{}'", s);
            }
            is_valid
            })
            .collect();
        info!("Parsed cors_origins from settings: {:?}", cors_origins);
        allowed_exact_origins.extend(cors_origins);
    }
    drop(db);

    if config.testing {
        allowed_exact_origins.push("http://127.0.0.1:27180".to_string());
        allowed_exact_origins.push("http://localhost:27180".to_string());
    }
    let mut allowed_regex_origins = vec![
        "chrome-extension://nglaklhklhcoonedhgnpgddginnjdadi".to_string(),
        // Every version of a mozilla extension has its own ID to avoid fingerprinting, so we
        // unfortunately have to allow all extensions to have access to aw-server
        "moz-extension://.*".to_string(),
    ];
    allowed_regex_origins.extend(config.cors_regex.clone());
    if config.testing {
        allowed_regex_origins.push("chrome-extension://.*".to_string());
    }

    let allowed_origins = AllowedOrigins::some(&allowed_exact_origins, &allowed_regex_origins);
    let allowed_methods = vec![Method::Get, Method::Post, Method::Delete]
        .into_iter()
        .map(From::from)
        .collect();
    let allowed_headers = AllowedHeaders::all(); // TODO: is this unsafe?

    // You can also deserialize this
    rocket_cors::CorsOptions {
        allowed_origins,
        allowed_methods,
        allowed_headers,
        allow_credentials: false,
        ..Default::default()
    }
    .to_cors()
    .expect("Failed to set up CORS")
}
