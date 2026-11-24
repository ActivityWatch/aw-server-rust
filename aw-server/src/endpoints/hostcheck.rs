//! Host header check needs to be performed to protect against DNS poisoning
//! attacks[1].
//!
//! Uses a Request Fairing to intercept the request before it's handled.
//! If the Host header is not valid, the request will be rerouted to a
//! BadRequest
//!
//! [1]: https://github.com/ActivityWatch/activitywatch/security/advisories/GHSA-v9fg-6g9j-h4x4
use rocket::fairing::Fairing;
use rocket::http::uri::Origin;
use rocket::http::{Method, Status};
use rocket::route::Outcome;
use rocket::{Data, Request, Rocket, Route};
use std::net::IpAddr;

use crate::config::AWConfig;
use crate::endpoints::HttpErrorJson;

static FAIRING_ROUTE_BASE: &str = "/checkheader_fairing";

pub struct HostCheck {
    validate: bool,
    /// The bound address, accepted as a Host in addition to the loopback names
    /// (a server bound to 127.0.0.2 is reached as http://127.0.0.2:5600).
    bound_host: String,
}

impl HostCheck {
    pub fn new(config: &AWConfig) -> HostCheck {
        // We only validate requests if the server binds a local address. Any
        // loopback address counts (127.0.0.0/8 and ::1), not just 127.0.0.1:
        // a DNS-rebinding page reaches ::1 or 127.0.0.2 just as easily.
        let validate = match config.address.parse::<IpAddr>() {
            // to_canonical: ::ffff:127.0.0.1 is the IPv4 loopback too
            Ok(ip) => ip.to_canonical().is_loopback(),
            Err(_) => config.address == "localhost",
        };
        HostCheck {
            validate,
            bound_host: config.address.clone(),
        }
    }
}

/// Strip the port from a Host header value, keeping IPv6 literals intact:
/// `[::1]:5600` -> `::1`, `localhost:5600` -> `localhost`.
fn host_without_port(hostheader: &str) -> Option<&str> {
    match hostheader.strip_prefix('[') {
        Some(rest) => {
            let (host, after) = rest.split_once(']')?;
            (after.is_empty() || after.starts_with(':')).then_some(host)
        }
        None => hostheader.split(':').next(),
    }
}

/// Create a `Handler` for Fairing error handling
#[derive(Clone)]
struct FairingErrorRoute {}

#[rocket::async_trait]
impl rocket::route::Handler for FairingErrorRoute {
    async fn handle<'r>(
        &self,
        request: &'r Request<'_>,
        _: rocket::Data<'r>,
    ) -> rocket::route::Outcome<'r> {
        let err = HttpErrorJson::new(Status::BadRequest, "Host header is invalid".to_string());
        Outcome::from(request, err)
    }
}

/// Create a new `Route` for Fairing handling
fn fairing_route() -> Route {
    Route::ranked(1, Method::Get, "/", FairingErrorRoute {})
}

fn redirect_bad_request(request: &mut Request) {
    let uri = FAIRING_ROUTE_BASE.to_string();
    let origin = Origin::parse_owned(uri).unwrap();
    request.set_method(Method::Get);
    request.set_uri(origin);
}

#[rocket::async_trait]
impl Fairing for HostCheck {
    fn info(&self) -> rocket::fairing::Info {
        rocket::fairing::Info {
            name: "HostCheck",
            kind: rocket::fairing::Kind::Ignite | rocket::fairing::Kind::Request,
        }
    }

    async fn on_ignite(&self, rocket: Rocket<rocket::Build>) -> rocket::fairing::Result {
        match self.validate {
            true => Ok(rocket.mount(FAIRING_ROUTE_BASE, vec![fairing_route()])),
            false => {
                warn!("Host header validation is turned off, this is a security risk");
                Ok(rocket)
            }
        }
    }

    async fn on_request(&self, request: &mut Request<'_>, _: &mut Data<'_>) {
        if !self.validate {
            // host header check is disabled
            return;
        }

        // Fetch header
        let hostheader_opt = request.headers().get_one("host");
        if hostheader_opt.is_none() {
            info!("Missing 'Host' header, denying request");
            redirect_bad_request(request);
            return;
        }

        // Parse hostname from host header
        // hostname contains port, which we don't care about and filter out
        let hostheader = hostheader_opt.unwrap();
        let host_opt = host_without_port(hostheader);
        if host_opt.is_none() {
            info!("Host header '{}' not allowed, denying request", hostheader);
            redirect_bad_request(request);
            return;
        }

        // Deny requests to hosts that are not localhost
        let valid_hosts: Vec<&str> = vec!["127.0.0.1", "localhost", "::1", &self.bound_host];
        let host = host_opt.unwrap();
        if !valid_hosts.contains(&host) {
            info!("Host header '{}' not allowed, denying request", hostheader);
            redirect_bad_request(request);
        }

        // host header is verified, proceed with request
    }
}

#[cfg(test)]
mod tests {

    use rocket::http::{ContentType, Header, Status};
    use rocket::Rocket;

    use crate::config::AWConfig;
    use crate::endpoints;

    fn setup_testserver(address: String) -> Rocket<rocket::Build> {
        let state = endpoints::ServerState::new(
            aw_datastore::Datastore::new_in_memory(false),
            endpoints::AssetResolver::new(None),
            "test_id".to_string(),
        );
        let aw_config = AWConfig {
            address,
            ..AWConfig::default()
        };
        endpoints::build_rocket(state, aw_config)
    }

    #[test]
    fn test_public_address() {
        let server = setup_testserver("0.0.0.0".to_string());
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        // When a public address is used, request should always pass, regardless
        // if the Host header is missing
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }

    #[test]
    fn test_localhost_address() {
        let server = setup_testserver("127.0.0.1".to_string());
        let client = rocket::local::blocking::Client::tracked(server).expect("valid instance");

        // If Host header is missing we should get a BadRequest
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest);

        // If Host header is not 127.0.0.1 or localhost we should get BadRequest
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "192.168.0.1:1234"))
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest);

        // If Host header is 127.0.0.1:5600 we should get OK
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        // If Host header is localhost:5600 we should get OK
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);

        // If Host header is missing port, we should still get OK
        let res = client
            .get("/api/0/info")
            .header(ContentType::JSON)
            .header(Header::new("Host", "localhost"))
            .dispatch();
        assert_eq!(res.status(), Status::Ok);
    }
}
