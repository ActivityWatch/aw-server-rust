use rocket::http::Method;
use rocket_cors::{AllowedHeaders, AllowedOrigins};

use crate::config::AWConfig;

const CHROME_EXTENSION_PREFIX: &str = "chrome-extension://";

pub fn cors(config: &AWConfig) -> rocket_cors::Cors {
    let root_url = format!("http://127.0.0.1:{}", config.port);
    let root_url_localhost = format!("http://localhost:{}", config.port);
    let mut allowed_exact_origins = vec![root_url, root_url_localhost];
    // url with chrome-extension:// is parsed by url crate as Opaque, so it
    // should be used as regex origin
    allowed_exact_origins.extend(
        config
            .cors
            .clone()
            .into_iter()
            .filter(|c| !c.starts_with(CHROME_EXTENSION_PREFIX)),
    );

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
    if config.testing {
        allowed_regex_origins.push("chrome-extension://.*".to_string());
    }
    allowed_regex_origins.extend(
        config
            .cors
            .clone()
            .into_iter()
            .filter(|c| c.starts_with(CHROME_EXTENSION_PREFIX)),
    );

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
    .inspect_err(|e| log::error!("failed to setup cors: {e}"))
    .expect("Failed to set up CORS")
}
