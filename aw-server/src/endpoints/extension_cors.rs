//! Endpoint scoping for the blanket `moz-extension://` CORS origin.
//!
//! Firefox gives every install of every extension its own random origin, so
//! there is no way to allowlist *only* `aw-watcher-web` up front — hence the
//! `moz-extension://.*` wildcard in [`crate::endpoints::cors`]. The cost is
//! that any installed extension can talk to the local API, with no host
//! permission and therefore no install-time prompt naming ActivityWatch.
//!
//! This fairing narrows that blanket trust to the endpoints `aw-watcher-web`
//! actually uses:
//!
//! - `GET /api/0/info` — hostname/version detection
//! - `POST /api/0/buckets/aw-watcher-web-<id>` — ensure its web-watcher bucket
//! - `POST /api/0/buckets/aw-watcher-web-<id>/heartbeat` — heartbeats
//!
//! # Accepted limitation
//!
//! The bucket prefix is a coarse scope, not an origin-to-bucket ownership
//! boundary. Any wildcard-matched Firefox extension can still create or send
//! heartbeats to another existing `aw-watcher-web-*` bucket. This preserves
//! the official watcher's zero-configuration flow while blocking writes to
//! other watcher types; eliminating web-bucket poisoning requires pairing an
//! extension origin with its bucket in a future protocol.
//!
//! Everything else (`/api/0/export`, `/api/0/import`, `/api/0/query`,
//! `/api/0/settings`, event reads, bucket deletion, ...) is closed to
//! wildcard-matched extension origins. That removes the data-exfiltration half
//! of the problem — app names, window titles, tab URLs, AFK data — without any
//! client change.
//!
//! Origins the *user* allowed explicitly via `cors_regex` in `config.toml`
//! are exempt: an explicit allowlist entry is a deliberate opt-in, unlike the
//! built-in wildcard. (The exact `cors` list cannot express an extension
//! origin — rocket_cors rejects `moz-extension://` there as an opaque origin —
//! so `cors_regex` is the only opt-in path for these.)
//!
//! # Enforcement
//!
//! Two stages, because CORS alone only hides *responses*:
//!
//! - `on_request` rejects disallowed requests with 403 before they reach a
//!   handler. Without this, a "simple" cross-origin `POST` (e.g.
//!   `text/plain`) is sent by the browser and still executes server-side even
//!   though the response is unreadable.
//! - `on_response` strips the `Access-Control-*` headers so preflights for
//!   disallowed endpoints fail in the browser.
//!
//! # Path matching
//!
//! Like [`crate::endpoints::apikey`], matching is done on
//! `request.uri().path().segments()` — the same percent-decoded,
//! empty-skipping view Rocket's router dispatches on. Matching the raw path
//! string would let `/%61pi/0/export` desynchronize this gate from the handler
//! it protects (the bug class of #588 and #636).

use regex::RegexSet;

use rocket::fairing::Fairing;
use rocket::http::uri::Origin;
use rocket::http::{Method, Status};
use rocket::route::Outcome;
use rocket::{Data, Request, Response, Rocket, Route};

use crate::config::AWConfig;
use crate::endpoints::HttpErrorJson;

static FAIRING_ROUTE_BASE: &str = "/extension_cors_fairing";

/// Scheme of the origins covered by the built-in wildcard.
const EXTENSION_ORIGIN_SCHEME: &str = "moz-extension://";

/// CORS response headers to strip when an extension origin is out of scope.
const CORS_HEADERS: &[&str] = &[
    "Access-Control-Allow-Origin",
    "Access-Control-Allow-Methods",
    "Access-Control-Allow-Headers",
    "Access-Control-Allow-Credentials",
    "Access-Control-Expose-Headers",
    "Access-Control-Max-Age",
];

pub struct ExtensionCorsScope {
    /// Regexes from `[server] cors_regex`, matched unanchored like rocket_cors.
    /// The exact `cors` list is not consulted: rocket_cors cannot hold an
    /// extension origin there, so it can never allow one.
    user_regex_origins: Option<RegexSet>,
}

impl ExtensionCorsScope {
    pub fn new(config: &AWConfig) -> ExtensionCorsScope {
        let user_regex_origins = if config.cors_regex.is_empty() {
            None
        } else {
            match RegexSet::new(&config.cors_regex) {
                Ok(set) => Some(set),
                Err(e) => {
                    // rocket_cors will fail on the same input; don't also panic here.
                    warn!("Invalid cors_regex in config, ignoring for extension scoping: {e}");
                    None
                }
            }
        };
        ExtensionCorsScope { user_regex_origins }
    }

    /// True if the user explicitly allowed this origin in their config.
    fn user_allowed(&self, origin: &str) -> bool {
        match &self.user_regex_origins {
            Some(set) => set.is_match(origin),
            None => false,
        }
    }

    /// True if this request must be blocked for the given origin.
    fn is_blocked(&self, request: &Request) -> bool {
        let origin = match request.headers().get_one("Origin") {
            // No Origin header: not a browser cross-origin request (native
            // watchers, curl). CORS is not the relevant control there.
            None => return false,
            Some(origin) => origin,
        };

        if !is_extension_origin(origin) || self.user_allowed(origin) {
            return false;
        }

        let method = match effective_method(request) {
            // A preflight without Access-Control-Request-Method tells us
            // nothing about the real request; fail closed.
            None => return true,
            Some(method) => method,
        };
        let segments: Vec<&str> = request.uri().path().segments().collect();
        !is_watcher_endpoint(method, &segments)
    }
}

/// Origins matched by the built-in `moz-extension://.*` wildcard.
fn is_extension_origin(origin: &str) -> bool {
    origin
        .to_ascii_lowercase()
        .starts_with(EXTENSION_ORIGIN_SCHEME)
}

/// The method the browser will actually use: for a preflight that is the
/// `Access-Control-Request-Method` header, not `OPTIONS`.
fn effective_method(request: &Request) -> Option<Method> {
    if request.method() == Method::Options {
        request
            .headers()
            .get_one("Access-Control-Request-Method")
            .and_then(|m| m.parse::<Method>().ok())
    } else {
        Some(request.method())
    }
}

/// The endpoints `aw-watcher-web` needs, as decoded path segments.
///
/// The prefix blocks writes to other watcher types, but deliberately does not
/// bind an extension origin to one `aw-watcher-web-*` bucket. See the accepted
/// limitation in the module documentation.
fn is_watcher_endpoint(method: Method, segments: &[&str]) -> bool {
    match (method, segments) {
        (Method::Get, ["api", "0", "info"]) => true,
        (Method::Post, ["api", "0", "buckets", bucket_id])
        | (Method::Post, ["api", "0", "buckets", bucket_id, "heartbeat"]) => {
            bucket_id.starts_with("aw-watcher-web-")
        }
        _ => false,
    }
}

/// Route handler returning 403 for out-of-scope extension requests.
#[derive(Clone)]
struct FairingErrorRoute {}

#[rocket::async_trait]
impl rocket::route::Handler for FairingErrorRoute {
    async fn handle<'r>(&self, request: &'r Request<'_>, _: Data<'r>) -> Outcome<'r> {
        let err = HttpErrorJson::new(
            Status::Forbidden,
            "This endpoint is not available to browser extension origins. \
             Add the origin to 'cors_regex' in config.toml to allow it."
                .to_string(),
        );
        Outcome::from(request, err)
    }
}

fn fairing_route() -> Route {
    Route::ranked(1, Method::Get, "/", FairingErrorRoute {})
}

fn redirect_forbidden(request: &mut Request) {
    let uri = FAIRING_ROUTE_BASE.to_string();
    let origin = Origin::parse_owned(uri).unwrap();
    request.set_method(Method::Get);
    request.set_uri(origin);
}

#[rocket::async_trait]
impl Fairing for ExtensionCorsScope {
    fn info(&self) -> rocket::fairing::Info {
        rocket::fairing::Info {
            name: "ExtensionCorsScope",
            kind: rocket::fairing::Kind::Ignite
                | rocket::fairing::Kind::Request
                | rocket::fairing::Kind::Response,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<rocket::Build>) -> rocket::fairing::Result {
        Ok(rocket.mount(FAIRING_ROUTE_BASE, vec![fairing_route()]))
    }

    async fn on_request(&self, request: &mut Request<'_>, _: &mut Data<'_>) {
        // Preflights are answered by rocket_cors; on_response strips the
        // CORS headers so the browser rejects the preflight itself.
        if request.method() == Method::Options {
            return;
        }
        if self.is_blocked(request) {
            debug!(
                "Blocking extension origin {:?} from {}",
                request.headers().get_one("Origin"),
                request.uri()
            );
            redirect_forbidden(request);
        }
    }

    async fn on_response<'r>(&self, request: &'r Request<'_>, response: &mut Response<'r>) {
        if self.is_blocked(request) {
            for header in CORS_HEADERS {
                response.remove_header(header);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rocket::http::{ContentType, Header, Status};
    use rocket::local::blocking::Client;
    use rocket::Rocket;

    use crate::config::AWConfig;
    use crate::endpoints;

    const EXT_ORIGIN: &str = "moz-extension://3f2b1c9d-dead-beef-cafe-000000000000";
    const WEBUI_ORIGIN: &str = "http://127.0.0.1:5600";

    fn setup_testserver(cors_regex: Vec<String>) -> Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            datastore: aw_datastore::Datastore::new_in_memory(false),
            asset_resolver: endpoints::AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = AWConfig::default();
        // Pin the port: the default is derived from a mutable global that other
        // tests flip via create_config(), which would otherwise make the
        // allowed webui origin depend on test execution order.
        aw_config.port = 5600;
        aw_config.cors_regex = cors_regex;
        endpoints::build_rocket(state, aw_config)
    }

    fn client() -> Client {
        Client::tracked(setup_testserver(vec![])).expect("valid instance")
    }

    fn allow_origin(res: &rocket::local::blocking::LocalResponse) -> Option<String> {
        res.headers()
            .get_one("Access-Control-Allow-Origin")
            .map(str::to_string)
    }

    /// The reported exfiltration path: an extension reading the full export.
    #[test]
    fn test_extension_origin_blocked_from_export() {
        let client = client();
        let res = client
            .get("/api/0/export")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden);
        assert_eq!(allow_origin(&res), None, "export readable by any extension");
    }

    /// The other half of the report: writing events through /api/0/import.
    #[test]
    fn test_extension_origin_blocked_from_import_preflight() {
        let client = client();
        let res = client
            .options("/api/0/import")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .header(Header::new("Access-Control-Request-Method", "POST"))
            .header(Header::new(
                "Access-Control-Request-Headers",
                "content-type",
            ))
            .dispatch();
        assert_eq!(allow_origin(&res), None, "import preflight still allowed");
        assert_eq!(res.headers().get_one("Access-Control-Allow-Methods"), None);
    }

    /// Everything aw-watcher-web needs must keep working, unchanged.
    #[test]
    fn test_watcher_endpoints_still_allowed() {
        let client = client();

        let res = client
            .get("/api/0/info")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
        assert_eq!(allow_origin(&res).as_deref(), Some(EXT_ORIGIN));

        let res = client
            .post("/api/0/buckets/aw-watcher-web-firefox_testhost")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .body(r#"{"type":"web.tab.current","client":"aw-client-web","hostname":"testhost"}"#)
            .dispatch();
        assert_ne!(res.status(), Status::Forbidden, "ensureBucket blocked");
        assert_eq!(allow_origin(&res).as_deref(), Some(EXT_ORIGIN));

        let res = client
            .post("/api/0/buckets/aw-watcher-web-firefox_testhost/heartbeat?pulsetime=60")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .body(
                r#"{"timestamp":"2026-07-28T09:00:00+00:00","duration":0,
                    "data":{"url":"https://example.com","title":"Example","audible":false,
                    "incognito":false,"tabCount":1}}"#,
            )
            .dispatch();
        assert_eq!(res.status(), Status::Ok, "heartbeat blocked");
        assert_eq!(allow_origin(&res).as_deref(), Some(EXT_ORIGIN));
    }

    /// A wildcard-matched extension must not create or write to buckets owned
    /// by other watchers. The URL bucket ID is the downstream authorization
    /// boundary, so this check must happen before the handler runs.
    #[test]
    fn test_extension_cannot_write_other_watcher_bucket() {
        let client = client();

        let res = client
            .post("/api/0/buckets/aw-watcher-window_testhost")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .body(r#"{"type":"currentwindow","client":"aw-client","hostname":"testhost"}"#)
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden, "foreign bucket created");
        assert_eq!(allow_origin(&res), None);

        let res = client
            .post("/api/0/buckets/aw-watcher-window_testhost/heartbeat?pulsetime=60")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .body(
                r#"{"timestamp":"2026-07-28T09:00:00+00:00","duration":0,
                    "data":{"app":"attacker","title":"injected"}}"#,
            )
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden, "foreign heartbeat written");
        assert_eq!(allow_origin(&res), None);
    }

    /// Preflight for a watcher endpoint must still succeed — aw-client-js
    /// sends JSON, which is always preflighted.
    #[test]
    fn test_watcher_preflight_still_allowed() {
        let client = client();
        let res = client
            .options("/api/0/buckets/aw-watcher-web-firefox_testhost/heartbeat")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .header(Header::new("Access-Control-Request-Method", "POST"))
            .header(Header::new(
                "Access-Control-Request-Headers",
                "content-type",
            ))
            .dispatch();
        assert_eq!(allow_origin(&res).as_deref(), Some(EXT_ORIGIN));
    }

    /// Scoping is per (method, path): the bucket path is writable for create,
    /// but must not become a deletion or read channel.
    #[test]
    fn test_bucket_path_scoped_by_method() {
        let client = client();

        let res = client
            .delete("/api/0/buckets/aw-watcher-web-firefox_testhost")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden, "bucket deletable");

        let res = client
            .get("/api/0/buckets/aw-watcher-web-firefox_testhost/events")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden, "events readable");
    }

    /// Same desync bug class as #588/#636: matching the raw path string would
    /// let an encoded spelling reach the handler while skipping this gate.
    #[test]
    fn test_encoded_paths_do_not_bypass_scope() {
        let client = client();
        for path in [
            "/%61pi/0/export",      // encoded 'a' in /api/
            "/api/0/%65xport",      // encoded 'e' in /export
            "//api/0/export",       // #588 double-slash trick
            "/%61pi/0/%65xport",    // both
            "/api/0/info/../query", // dot-segment
        ] {
            let res = client
                .get(path)
                .header(Header::new("Host", "localhost:5600"))
                .header(Header::new("Origin", EXT_ORIGIN))
                .dispatch();
            assert_eq!(
                res.status(),
                Status::Forbidden,
                "extension scope bypassed via {path}"
            );
            assert_eq!(allow_origin(&res), None, "CORS header leaked for {path}");
        }

        // Sanity check: these paths really do reach the export handler without
        // an extension Origin, so the 403s above are this gate rejecting them
        // rather than the router failing to match.
        for path in ["/%61pi/0/export", "//api/0/export", "/api/0/%65xport"] {
            let res = client
                .get(path)
                .header(Header::new("Host", "localhost:5600"))
                .dispatch();
            assert_eq!(
                res.status(),
                Status::Ok,
                "{path} does not reach the handler"
            );
        }
    }

    /// Non-extension origins are untouched — the webui is same-origin and
    /// still needs full access.
    #[test]
    fn test_webui_origin_unaffected() {
        let client = client();
        let res = client
            .get("/api/0/export")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", WEBUI_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
        assert_eq!(allow_origin(&res).as_deref(), Some(WEBUI_ORIGIN));
    }

    /// Native clients send no Origin header and must be unaffected.
    #[test]
    fn test_no_origin_unaffected() {
        let client = client();
        let res = client
            .get("/api/0/export")
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }

    /// An explicit `cors_regex` entry is a deliberate opt-in and keeps full
    /// access — but only for the origins it actually matches.
    #[test]
    fn test_user_regex_allowlist_scoped_to_match() {
        let server = setup_testserver(vec![format!("^{EXT_ORIGIN}$")]);
        let client = Client::tracked(server).expect("valid instance");

        let res = client
            .get("/api/0/export")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        // A different extension is still scoped.
        let res = client
            .get("/api/0/export")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new(
                "Origin",
                "moz-extension://00000000-0000-0000-0000-999999999999",
            ))
            .dispatch();
        assert_eq!(res.status(), Status::Forbidden);
    }

    /// A preflight that hides its real method must not pass.
    #[test]
    fn test_preflight_without_request_method_is_blocked() {
        let client = client();
        let res = client
            .options("/api/0/buckets/some-bucket")
            .header(Header::new("Host", "localhost:5600"))
            .header(Header::new("Origin", EXT_ORIGIN))
            .dispatch();
        assert_eq!(allow_origin(&res), None);
    }
}
