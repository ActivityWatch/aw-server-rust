//! API key authentication via Bearer token.
//!
//! When `api_key` is set under `[auth]` in the config, all API endpoints except
//! `/api/0/info` require an `Authorization: Bearer <key>` header. Requests
//! missing or presenting an invalid key receive a 401 Unauthorized response.
//!
//! By default `api_key` is `None`, meaning authentication is disabled.
//! To enable, add to `config.toml`:
//!
//! ```toml
//! [auth]
//! api_key = "your-secret-key-here"
//! ```
//!
//! Exempt paths (always public):
//! - `GET /api/0/info` — health/version endpoint used by clients and the webui
//!
//! CORS preflight requests (OPTIONS) are also passed through unconditionally so
//! the browser can obtain allowed headers before sending the actual request.
//!
//! # Path matching
//!
//! The gate matches on `request.uri().path().segments()` — the same
//! percent-decoded, empty-skipping segment view Rocket's router uses to select
//! a handler (see `rocket::router::collider::paths_match`). Comparing the raw
//! path string instead lets an attacker desynchronize the gate from the router
//! (e.g. `/%61pi/0/buckets/` or `//api/0/buckets/`), skipping auth on a request
//! the router still dispatches to a real `/api/` handler.

use subtle::ConstantTimeEq;

use rocket::fairing::Fairing;
use rocket::http::uri::Origin;
use rocket::http::{Method, Status};
use rocket::route::Outcome;
use rocket::{Data, Request, Rocket, Route};

use crate::config::AWConfig;
use crate::endpoints::HttpErrorJson;

static FAIRING_ROUTE_BASE: &str = "/apikey_fairing";

/// Path segments gated by API key authentication.
const API_SEGMENT: &str = "api";

/// Paths that are always accessible without authentication, as decoded segments.
const PUBLIC_PATHS: &[&[&str]] = &[&["api", "0", "info"]];

pub struct ApiKeyCheck {
    api_key: Option<String>,
}

impl ApiKeyCheck {
    pub fn new(config: &AWConfig) -> ApiKeyCheck {
        let api_key = match &config.auth.api_key {
            Some(k) if k.is_empty() => {
                warn!("api_key is set to an empty string — authentication is disabled. Set a non-empty key to enable auth.");
                None
            }
            other => other.clone(),
        };
        ApiKeyCheck { api_key }
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

        // Match on the same decoded segment view the router uses to dispatch,
        // so the gate cannot be desynchronized from the handler it protects.
        let segments: Vec<&str> = request.uri().path().segments().collect();

        // Only gate API endpoints — static web UI assets are not under /api/
        if segments.first() != Some(&API_SEGMENT) {
            return;
        }

        // Always allow public API paths (e.g. /api/0/info for health checks)
        if PUBLIC_PATHS.contains(&segments.as_slice()) {
            return;
        }

        // Validate Authorization: Bearer <key>
        // Use constant-time comparison to prevent timing attacks.
        let auth_header = request.headers().get_one("Authorization");
        let valid = match auth_header {
            Some(value) => {
                if let Some(token) = value.strip_prefix("Bearer ") {
                    token.as_bytes().ct_eq(api_key.as_bytes()).into()
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

    use rocket::http::{ContentType, Header, Status};
    use rocket::Rocket;

    use crate::config::AWConfig;
    use crate::endpoints;

    fn setup_testserver(api_key: Option<String>) -> Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            datastore: aw_datastore::Datastore::new_in_memory(false),
            asset_resolver: endpoints::AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = AWConfig::default();
        aw_config.auth.api_key = api_key;
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

        // Double slash should also require auth
        let res = client
            .get("//api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Unauthorized);
    }

    /// Percent-encoded paths must not bypass the gate.
    ///
    /// Rocket's router decodes the path before matching, so a fairing that
    /// compares the *raw* path string can be desynchronized from the handler
    /// it protects: `/%61pi/0/buckets/` reaches the `/api/0/buckets/` handler
    /// while a raw `starts_with("/api/")` check sees `/%61pi/...` and skips auth.
    #[test]
    fn test_api_key_percent_encoded_paths_require_auth() {
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        // Each of these decodes to the real /api/0/buckets/ route, so each must
        // be rejected by the auth gate specifically — asserting "not 200" would
        // also pass on a 404, hiding a route miss as an auth success.
        for path in [
            "/%61pi/0/buckets/",     // lowercase hex, first char
            "/ap%69/0/buckets/",     // encoded char mid-segment
            "/%61%70%69/0/buckets/", // fully encoded segment
            "//%61pi/0/buckets/",    // encoding combined with the #588 double-slash trick
        ] {
            let res = client
                .get(path)
                .header(ContentType::JSON)
                .header(Header::new("Host", "localhost:5600"))
                .dispatch();
            assert_eq!(
                res.status(),
                Status::Unauthorized,
                "expected auth rejection for encoded path {path}"
            );
        }

        // Sanity check on the above: these paths really do reach the buckets
        // handler once a valid key is supplied, so the 401s are the gate
        // rejecting them rather than the router failing to match.
        for path in ["/%61pi/0/buckets/", "//%61pi/0/buckets/"] {
            let res = client
                .get(path)
                .header(ContentType::JSON)
                .header(Header::new("Host", "localhost:5600"))
                .header(Header::new("Authorization", "Bearer secret123"))
                .dispatch();
            assert_eq!(
                res.status(),
                Status::Ok,
                "encoded path {path} does not reach the real handler"
            );
        }

        // Case is significant: /API/ is not a registered route at all, so this
        // 404s at the router rather than exercising the gate. Asserted
        // explicitly so a future case-insensitive router change would show up
        // here instead of silently passing a "not 200" check.
        let res = client
            .get("/%41PI/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::NotFound);
    }

    /// Writes must be gated too — a bypass that only blocked GETs would still
    /// allow bucket creation / event insertion.
    #[test]
    fn test_api_key_percent_encoded_write_requires_auth() {
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .post("/%61pi/0/buckets/authbypass-poc")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .body(r#"{"type":"test","client":"authbypass-poc","hostname":"poc-host"}"#)
            .dispatch();
        assert_eq!(
            res.status(),
            Status::Unauthorized,
            "write bypassed via encoded path"
        );

        // And the bucket must not exist afterwards — 404 from an authenticated
        // lookup, not merely "some non-200 status".
        let res = client
            .get("/api/0/buckets/authbypass-poc")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Authorization", "Bearer secret123"))
            .dispatch();
        assert_eq!(
            res.status(),
            Status::NotFound,
            "bucket was created without auth"
        );
    }

    /// The public-path exemption is matched on decoded segments, so encoded
    /// spellings of `/api/0/info` stay public (parity with the router) while
    /// nothing else slips through the exemption.
    #[test]
    fn test_public_path_matched_on_decoded_segments() {
        let server = setup_testserver(Some("secret123".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/%69nfo")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        // A longer path that merely starts with the public prefix is not public:
        // the exemption is an exact segment match, so this hits the gate.
        let res = client
            .get("/api/0/info/../buckets/")
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

    #[test]
    fn test_empty_api_key_disables_auth() {
        // An empty string key should be treated as disabled (no auth required).
        let server = setup_testserver(Some("".to_string()));
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }
}
