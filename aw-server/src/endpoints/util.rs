use std::io::{Cursor, Seek, SeekFrom};

use rocket::http::ContentType;
use rocket::http::Header;
use rocket::http::Status;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use serde::Serialize;

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
        let body = serde_json::to_string(&self).map_err(|err| {
            error!("Failed to serialize error response: {err}");
            Status::InternalServerError
        })?;
        Response::build()
            .status(self.status)
            .sized_body(body.len(), Cursor::new(body))
            .header(ContentType::new("application", "json"))
            .ok()
    }
}

pub struct BucketsExportRocket {
    file: std::fs::File,
    filename: String,
}

impl BucketsExportRocket {
    pub fn new(
        datastore: &aw_datastore::Datastore,
        bucket_id: Option<&str>,
    ) -> Result<Self, HttpErrorJson> {
        let io_error = |err: std::io::Error| {
            error!("Failed to prepare export file: {err}");
            HttpErrorJson::new(
                Status::InternalServerError,
                "Failed to prepare export file".into(),
            )
        };
        // tempfile creates a private file and removes it when the response is
        // dropped. Spooling preserves HTTP errors even if serialization or disk
        // writes fail, while keeping event buffering bounded.
        let file = tempfile::tempfile().map_err(io_error)?;
        let (mut file, name) = datastore.export_to_file(bucket_id, file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let filename = match name {
            Some(id) => format!("attachment; filename=aw-bucket-export_{id}.json"),
            None => "attachment; filename=aw-buckets-export.json".into(),
        };
        Ok(Self { file, filename })
    }
}

impl<'r> Responder<'r, 'static> for BucketsExportRocket {
    fn respond_to(self, _: &Request) -> response::Result<'static> {
        Response::build()
            .status(Status::Ok)
            .header(Header::new("Content-Disposition", self.filename))
            .header(ContentType::JSON)
            .streamed_body(rocket::tokio::fs::File::from_std(self.file))
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
        }
    }
}
