use std::io::Cursor;

use rocket::http::ContentType;
use rocket::http::Header;
use rocket::http::Status;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use serde::Serialize;

use aw_models::BucketsExport;

#[derive(Serialize, Debug)]
pub struct HttpErrorJson {
    #[serde(skip_serializing)]
    status: Status,
    message: String,
}

impl HttpErrorJson {
    pub fn new(status: Status, err: String) -> HttpErrorJson {
        HttpErrorJson {
            status,
            message: err,
        }
    }
}

impl<'r> Responder<'r, 'static> for HttpErrorJson {
    fn respond_to(self, _: &Request) -> response::Result<'static> {
        // TODO: Fix unwrap
        let body = serde_json::to_string(&self).unwrap();
        Response::build()
            .status(self.status)
            .sized_body(body.len(), Cursor::new(body))
            .header(ContentType::new("application", "json"))
            .ok()
    }
}

pub struct BucketsExportRocket {
    inner: BucketsExport,
}

impl From<BucketsExport> for BucketsExportRocket {
    fn from(val: BucketsExport) -> Self {
        BucketsExportRocket { inner: val }
    }
}

impl<'r> Responder<'r, 'static> for BucketsExportRocket {
    fn respond_to(self, _: &Request) -> response::Result<'static> {
        let body = serde_json::to_string(&self.inner).unwrap();
        let header_content = match self.inner.buckets.len() == 1 {
            true => format!(
                "attachment; filename=aw-bucket-export_{}.json",
                self.inner.buckets.into_keys().next().unwrap()
            ),
            false => "attachment; filename=aw-buckets-export.json".to_string(),
        };
        // TODO: Fix unwrap
        Response::build()
            .status(Status::Ok)
            .header(Header::new("Content-Disposition", header_content))
            .sized_body(body.len(), Cursor::new(body))
            //.header(ContentType::new("application", "json"))
            .ok()
    }
}

use aw_datastore::DatastoreError;

impl From<DatastoreError> for HttpErrorJson {
    fn from(val: DatastoreError) -> Self {
        match val {
            DatastoreError::NoSuchBucket(bucket_id) => HttpErrorJson::new(
                Status::NotFound,
                format!("The requested bucket '{bucket_id}' does not exist"),
            ),
            DatastoreError::BucketAlreadyExists(bucket_id) => HttpErrorJson::new(
                Status::NotModified,
                format!("Bucket '{bucket_id}' already exists"),
            ),
            DatastoreError::NoSuchKey(key) => HttpErrorJson::new(
                Status::NotFound,
                format!("The requested key(s) '{key}' do not exist"),
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
            DatastoreError::CommitFailed(msg) => HttpErrorJson::new(
                Status::ServiceUnavailable,
                format!("Database commit failed (disk full?): {msg}"),
            ),
        }
    }
}

#[macro_export]
macro_rules! endpoints_get_lock {
    ( $lock:expr ) => {
        match $lock.lock() {
            Ok(r) => r,
            Err(e) => {
                use rocket::http::Status;
                let err_msg = format!("Taking datastore lock failed, returning 504: {}", e);
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::ServiceUnavailable, err_msg));
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use aw_datastore::DatastoreError;
    use rocket::http::Status;

    #[test]
    fn test_datastore_error_to_http_error() {
        // Test NoSuchBucket -> 404
        let err: HttpErrorJson = DatastoreError::NoSuchBucket("test-bucket".into()).into();
        assert_eq!(err.status, Status::NotFound);
        assert!(err.message.contains("test-bucket"));

        // Test BucketAlreadyExists -> 304
        let err: HttpErrorJson = DatastoreError::BucketAlreadyExists("test-bucket".into()).into();
        assert_eq!(err.status, Status::NotModified);

        // Test NoSuchKey -> 404
        let err: HttpErrorJson = DatastoreError::NoSuchKey("test-key".into()).into();
        assert_eq!(err.status, Status::NotFound);

        // Test MpscError -> 500
        let err: HttpErrorJson = DatastoreError::MpscError.into();
        assert_eq!(err.status, Status::InternalServerError);

        // Test InternalError -> 500
        let err: HttpErrorJson = DatastoreError::InternalError("internal".into()).into();
        assert_eq!(err.status, Status::InternalServerError);

        // Test Uninitialized -> 500
        let err: HttpErrorJson = DatastoreError::Uninitialized("uninitialized".into()).into();
        assert_eq!(err.status, Status::InternalServerError);

        // Test OldDbVersion -> 500
        let err: HttpErrorJson = DatastoreError::OldDbVersion("old version".into()).into();
        assert_eq!(err.status, Status::InternalServerError);

        // Test CommitFailed -> 503 (new test for disk-full handling)
        let err: HttpErrorJson =
            DatastoreError::CommitFailed("database or disk is full".into()).into();
        assert_eq!(err.status, Status::ServiceUnavailable);
        assert!(err.message.contains("disk full"));
    }
}
