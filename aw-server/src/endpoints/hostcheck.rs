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

use crate::config::AWConfig;
use crate::endpoints::HttpErrorJson;

static FAIRING_ROUTE_BASE: &str = "/checkheader_fairing";

pub struct HostCheck {
    validate: bool,
}

impl HostCheck {
    pub fn new(config: &AWConfig) -> HostCheck {
        // We only validate requests if the server binds a local address
        let validate = config.address == "127.0.0.1" || config.address == "localhost";
        HostCheck { validate }
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
        let host_opt = hostheader.split(':').next();
        if host_opt.is_none() {
            info!("Host header '{}' not allowed, denying request", hostheader);
            redirect_bad_request(request);
            return;
        }

        // Deny requests to hosts that are not localhost
        let valid_hosts: Vec<&str> = vec!["127.0.0.1", "localhost"];
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
    use std::sync::Mutex;

    use rocket::http::{ContentType, Header, Status};
    use rocket::Rocket;

    use crate::config::AWConfig;
    use crate::endpoints;

    fn setup_testserver(address: String) -> Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_resolver: endpoints::AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = AWConfig::default();
        aw_config.address = address;
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
