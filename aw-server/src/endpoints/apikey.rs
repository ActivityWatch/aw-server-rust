//! API key authentication via Bearer token.
//!
//! When `api_key` is set in the config, all API endpoints except `/api/0/info`
//! require an `Authorization: Bearer <key>` header. Requests missing or
//! presenting an invalid key receive a 401 Unauthorized response.
//!
//! By default `api_key` is `None`, meaning authentication is disabled.
//! To enable, add to `config.toml`:
//!
//! ```toml
//! api_key = "your-secret-key-here"
//! ```
//!
//! Exempt paths (always public):
//! - `GET /api/0/info` — health/version endpoint used by clients and the webui
//!
//! CORS preflight requests (OPTIONS) are also passed through unconditionally so
//! the browser can obtain allowed headers before sending the actual request.

use rocket::fairing::Fairing;
use rocket::http::uri::Origin;
use rocket::http::{Method, Status};
use rocket::route::Outcome;
use rocket::{Data, Request, Rocket, Route};

use crate::config::AWConfig;
use crate::endpoints::HttpErrorJson;

static FAIRING_ROUTE_BASE: &str = "/apikey_fairing";

/// Paths that are always accessible without authentication.
const PUBLIC_PATHS: &[&str] = &["/api/0/info"];

pub struct ApiKeyCheck {
    api_key: Option<String>,
}

impl ApiKeyCheck {
    pub fn new(config: &AWConfig) -> ApiKeyCheck {
        ApiKeyCheck {
            api_key: config.api_key.clone(),
        }
    }
}

/// Route handler that returns 401 Unauthorized for failed auth checks.
#[derive(Clone)]
struct FairingErrorRoute {}

#[rocket::async_trait]
impl rocket::route::Handler for FairingErrorRoute {
    async fn handle<'r>(
        &self,
        request: &'r Request<'_>,
        _: rocket::Data<'r>,
    ) -> rocket::route::Outcome<'r> {
        let err = HttpErrorJson::new(
            Status::Unauthorized,
            "Missing or invalid API key. Set 'Authorization: Bearer <key>' header.".to_string(),
        );
        Outcome::from(request, err)
    }
}

fn fairing_route() -> Route {
    Route::ranked(1, Method::Get, "/", FairingErrorRoute {})
}

fn redirect_unauthorized(request: &mut Request) {
    let uri = FAIRING_ROUTE_BASE.to_string();
    let origin = Origin::parse_owned(uri).unwrap();
    request.set_method(Method::Get);
    request.set_uri(origin);
}

#[rocket::async_trait]
impl Fairing for ApiKeyCheck {
    fn info(&self) -> rocket::fairing::Info {
        rocket::fairing::Info {
            name: "ApiKeyCheck",
            kind: rocket::fairing::Kind::Ignite | rocket::fairing::Kind::Request,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<rocket::Build>) -> rocket::fairing::Result {
        match &self.api_key {
            Some(_) => Ok(rocket.mount(FAIRING_ROUTE_BASE, vec![fairing_route()])),
            None => {
                debug!("API key authentication is disabled");
                Ok(rocket)
            }
        }
    }

    async fn on_request(&self, request: &mut Request<'_>, _: &mut Data<'_>) {
        let api_key = match &self.api_key {
            None => return, // auth disabled
            Some(k) => k,
        };

        // Always allow OPTIONS (CORS preflight)
        if request.method() == Method::Options {
            return;
        }

        // Always allow public paths
        if PUBLIC_PATHS.contains(&request.uri().path().as_str()) {
            return;
        }

        // Validate Authorization: Bearer <key>
        let auth_header = request.headers().get_one("Authorization");
        let valid = match auth_header {
            Some(value) => {
                if let Some(token) = value.strip_prefix("Bearer ") {
                    token == api_key
                } else {
                    false
                }
            }
            None => false,
        };

        if !valid {
            debug!("API key check failed for {}", request.uri());
            redirect_unauthorized(request);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rocket::http::{ContentType, Header, Status};
    use rocket::Rocket;

    use crate::config::AWConfig;
    use crate::endpoints;

    fn setup_testserver(api_key: Option<String>) -> Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_resolver: endpoints::AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = AWConfig::default();
        aw_config.api_key = api_key;
        endpoints::build_rocket(state, aw_config)
    }

    #[test]
    fn test_no_api_key_configured() {
        // When no api_key is set, all endpoints are accessible without auth.
        let server = setup_testserver(None);
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }

    #[test]
    fn test_api_key_required() {
        // With api_key set, requests without a key should be rejected.
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        // /api/0/info is always public
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        // Other endpoints require auth
        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Unauthorized);
    }

    #[test]
    fn test_api_key_valid() {
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Authorization", "Bearer secret123"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }

    #[test]
    fn test_api_key_invalid() {
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Authorization", "Bearer wrongkey"))
            .dispatch();
        assert_eq!(res.status(), Status::Unauthorized);
    }

    #[test]
    fn test_api_key_wrong_scheme() {
        // Must be Bearer, not Basic or bare key
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Authorization", "Basic secret123"))
            .dispatch();
        assert_eq!(res.status(), Status::Unauthorized);
    }
}
