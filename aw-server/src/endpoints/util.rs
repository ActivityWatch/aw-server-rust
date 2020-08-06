use std::io::Cursor;

use rocket::http::ContentType;
use rocket::http::Status;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};

#[derive(Serialize, Debug)]
pub struct HttpErrorJson {
    #[serde(skip_serializing)]
    status: Status,
    message: String,
}

impl HttpErrorJson {
    pub fn new(status: Status, err: String) -> HttpErrorJson {
        HttpErrorJson {
            status: status,
            message: format!("{}", err),
        }
    }
}

impl<'r> Responder<'r> for HttpErrorJson {
    fn respond_to(self, _: &Request) -> response::Result<'r> {
        Response::build()
            .status(self.status)
            .sized_body(Cursor::new(format!("{{\"message\":\"{}\"}}", self.message)))
            .header(ContentType::new("application", "json"))
            .ok()
    }
}

use aw_datastore::DatastoreError;

impl Into<HttpErrorJson> for DatastoreError {
    fn into(self) -> HttpErrorJson {
        match self {
            DatastoreError::NoSuchBucket => HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            ),
            DatastoreError::BucketAlreadyExists => {
                HttpErrorJson::new(Status::NotModified, "Bucket already exists".to_string())
            }
            DatastoreError::NoSuchKey => HttpErrorJson::new(
                Status::NotFound,
                "The requested key does not exist".to_string(),
            ),
            DatastoreError::MpscError => HttpErrorJson::new(
                Status::InternalServerError,
                "Unexpected Mpsc error!".to_string(),
            ),
            DatastoreError::InternalError(msg) => {
                HttpErrorJson::new(Status::InternalServerError, msg)
            }
            // When upgrade is disabled
            DatastoreError::Uninitialized(msg) => {
                HttpErrorJson::new(Status::InternalServerError, msg)
            }
            DatastoreError::OldDbVersion(msg) => {
                HttpErrorJson::new(Status::InternalServerError, msg)
            }
        }
    }
}

#[macro_export]
macro_rules! endpoints_get_lock {
    ( $lock:expr ) => {
        match $lock.lock() {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("Taking datastore lock failed, returning 504: {}", e);
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::ServiceUnavailable, err_msg));
            }
        }
    };
}
