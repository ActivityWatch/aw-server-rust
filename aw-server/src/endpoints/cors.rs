use rocket::http::Method;
use rocket_cors::{AllowedHeaders, AllowedOrigins};

use crate::config::AWConfig;

pub fn cors(config: &AWConfig) -> rocket_cors::Cors {
    let root_url = format!("http://127.0.0.1:{}", config.port);
    let root_url_localhost = format!("http://localhost:{}", config.port);
    let mut allowed_exact_origins = vec![root_url.clone(), root_url_localhost.clone()];
    allowed_exact_origins.extend(config.cors.clone());

    let mut allowed_regex_origins = config.cors_regex.clone();

    if config.cors_allow_aw_chrome_extension {
        allowed_regex_origins.push("chrome-extension://nglaklhklhcoonedhgnpgddginnjdadi".to_string());
    }

    if config.cors_allow_all_mozilla_extension {
        // Every version of a mozilla extension has its own ID to avoid fingerprinting, so we
        // unfortunately have to allow all extensions to have access to aw-server
        allowed_regex_origins.push("moz-extension://.*".to_string());
    }

    if config.testing {
        allowed_exact_origins.extend(vec![
            "http://127.0.0.1:27180".to_string(),
            "http://localhost:27180".to_string(),
        ]);
        allowed_regex_origins.push("chrome-extension://.*".to_string());
    }

    let allowed_origins = AllowedOrigins::some(&allowed_exact_origins, &allowed_regex_origins);
    let allowed_methods = vec![Method::Get, Method::Post, Method::Delete]
        .into_iter()
        .map(From::from)
        .collect();
    let allowed_headers = AllowedHeaders::all(); // TODO: is this unsafe?

    // You can also deserialize this
    let cors_options = rocket_cors::CorsOptions {
        allowed_origins,
        allowed_methods,
        allowed_headers,
        allow_credentials: false,
        ..Default::default()
    };

    match cors_options.to_cors() {
        Ok(cors) => cors,
        Err(e) => {
            error!("Failed to set up CORS with provided origins: {:?}", e);
            error!("Exact origins: {:?}", allowed_exact_origins);
            error!("Regex origins: {:?}", allowed_regex_origins);
            // Fallback to a safe default to allow the server to at least start
            let fallback_origins = vec![root_url, root_url_localhost];
            let empty_regex: &[String] = &[];
            rocket_cors::CorsOptions {
                allowed_origins: AllowedOrigins::some(&fallback_origins, empty_regex),
                ..Default::default()
            }
            .to_cors()
            .expect("Safe default CORS should always work")
        }
    }
}
