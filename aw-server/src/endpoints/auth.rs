/// On most systems we do not concern ourselves with local security [1], however on Android
/// apps have their storage isolated yet ActivityWatch happily exposes its API and database
/// through the HTTP API when the server is running. This could be considered a severe
/// security flaw, and fixing it would significantly improve security on Android (as mentioned in [2]).
///
/// Requiring an API key can also be useful in other scenarios where an extra level of security is
/// desired.
///
/// Based on the ApiKey example at [3].
///
/// [1]: https://docs.activitywatch.net/en/latest/security.html#activitywatch-is-only-as-secure-as-your-system
/// [2]: https://forum.activitywatch.net/t/rest-api-supported-with-android-version-of-activity-watch/854/6?u=erikbjare
/// [3]: https://api.rocket.rs/v0.4/rocket/request/trait.FromRequest.html
use rocket::http::Status;
use rocket::request::{self, FromRequest, Request};
use rocket::{Outcome, State};

use crate::config::AWConfig;

struct ApiKey(Option<String>);

#[derive(Debug)]
enum ApiKeyError {
    BadCount,
    Missing,
    Invalid,
}

// TODO: Use guard on endpoints
// TODO: Add tests for protected endpoints (important to ensure security)
impl<'a, 'r> FromRequest<'a, 'r> for ApiKey {
    type Error = ApiKeyError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Self::Error> {
        // TODO: How will this key be configured by the user?
        let config = request.guard::<State<AWConfig>>().unwrap();
        match &config.apikey {
            None => Outcome::Success(ApiKey(None)),
            Some(apikey) => {
                // TODO: How will this header be set in the browser?
                let keys: Vec<_> = request.headers().get("x-api-key").collect();
                match keys.len() {
                    0 => Outcome::Failure((Status::BadRequest, ApiKeyError::Missing)),
                    1 if apikey == keys[0] => Outcome::Success(ApiKey(Some(keys[0].to_string()))),
                    1 => Outcome::Failure((Status::BadRequest, ApiKeyError::Invalid)),
                    _ => Outcome::Failure((Status::BadRequest, ApiKeyError::BadCount)),
                }
            }
        }
    }
}
