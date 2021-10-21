/// Host header check needs to be performed to protect against DNS poisoning attacks.
///
/// Based on API key PR in [2].
///
/// [1]: https://github.com/ActivityWatch/activitywatch/security/advisories/GHSA-v9fg-6g9j-h4x4
/// [2]: https://github.com/ActivityWatch/aw-server-rust/pull/185
use rocket::http::Status;
use rocket::request::{self, FromRequest, Request};
use rocket::{Outcome, State};

use crate::config::AWConfig;

pub struct HostCheck();

#[derive(Debug)]
pub enum HostCheckError {
    Invalid,
}

// TODO: Should this be an app-wide fairing instead? (apparently fairings can't cancel/reject requests?)
// TODO: Use guard on any remaining sensitive endpoints
// TODO: Add tests to ensure enforced
impl<'a, 'r> FromRequest<'a, 'r> for HostCheck {
    type Error = HostCheckError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Self::Error> {
        let config = request.guard::<State<AWConfig>>().unwrap();
        let valid_hosts: Vec<&str> = vec!["127.0.0.1", "localhost"];
        if let Some(hostheader) = request.headers().get_one("host") {
            // TODO: Probably have to split hostheader on ':' as it may contain the port
            if &config.address == "127.0.0.1" || &config.address == "localhost" {
                if valid_hosts.contains(&hostheader) {
                    Outcome::Success(HostCheck())
                } else {
                    Outcome::Failure((Status::BadRequest, HostCheckError::Invalid))
                }
            } else {
                // If server is not set to listen to 127.0.0.1 or localhost, skip check.
                Outcome::Success(HostCheck())
            }
        } else {
            Outcome::Failure((Status::BadRequest, HostCheckError::Invalid))
        }
    }
}
