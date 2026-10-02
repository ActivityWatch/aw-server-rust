use std::fs::File;
use std::io::{copy, pipe, Cursor, PipeReader, PipeWriter, Seek, SeekFrom};
use std::thread;

use chrono::{DateTime, Utc};
use rocket::data::{self, Data, FromData};
use rocket::http::ContentType;
use rocket::http::Header;
use rocket::http::Status;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use rocket::serde::json::Json;
use serde::Deserialize;
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

/// Reason a JSON request body was rejected, stashed in the request-local cache
/// so the `/api` error catchers can report it (Rocket only logs it).
#[derive(Default)]
struct BodyError(std::sync::Mutex<Option<String>>);

/// Drop-in replacement for `Json<T>` as a data guard: identical parsing, but the
/// serde error is kept so [`api_error_json`] can return it to the client.
pub struct ApiJson<T>(pub T);

impl<T> ApiJson<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for ApiJson<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

#[rocket::async_trait]
impl<'r, T: Deserialize<'r>> FromData<'r> for ApiJson<T> {
    type Error = String;

    async fn from_data(req: &'r Request<'_>, data: Data<'r>) -> data::Outcome<'r, Self> {
        match <Json<T> as FromData>::from_data(req, data).await {
            data::Outcome::Success(json) => data::Outcome::Success(ApiJson(json.into_inner())),
            data::Outcome::Error((status, err)) => {
                let msg = err.to_string();
                *req.local_cache(BodyError::default).0.lock().unwrap() = Some(msg.clone());
                data::Outcome::Error((status, msg))
            }
            data::Outcome::Forward(data) => data::Outcome::Forward(data),
        }
    }
}

/// Build the JSON error body shared by the `/api` catchers.
pub fn api_error_json(status: Status, req: &Request) -> HttpErrorJson {
    let reason = req.local_cache(BodyError::default).0.lock().unwrap().take();
    let message = match reason {
        Some(detail) => format!("{}: {}", status.reason_lossy(), detail),
        None => status.reason_lossy().to_string(),
    };
    HttpErrorJson::new(status, message)
}

pub struct BucketsExportRocket {
    datastore: aw_datastore::Datastore,
    bucket_id: Option<String>,
    filename: String,
}

/// Make a client-supplied bucket id safe to interpolate into a response header.
///
/// Bucket ids are not restricted at creation, and Rocket percent-decodes path
/// segments, so an id containing CR/LF would otherwise split the
/// `Content-Disposition` header (response splitting / header injection).
fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '"' | '\\' | ';' => '_',
            c => c,
        })
        .collect()
}

/// Build a `Content-Disposition` value for an attachment download.
///
/// The filename is quoted: bucket ids may legally contain spaces, and an
/// unquoted `filename=my bucket.csv` is a malformed parameter that clients may
/// drop. [`sanitize_header_value`] has already removed the characters that could
/// break out of the quoted string.
fn content_disposition(filename: &str) -> String {
    format!(
        "attachment; filename=\"{}\"",
        sanitize_header_value(filename)
    )
}

fn export_filename(
    datastore: &aw_datastore::Datastore,
    bucket_id: Option<&str>,
) -> Result<String, HttpErrorJson> {
    let name = match bucket_id {
        Some(id) => {
            datastore.get_bucket(id)?;
            Some(id.to_owned())
        }
        None => {
            let buckets = datastore.get_buckets()?;
            (buckets.len() == 1).then(|| buckets.into_keys().next().unwrap())
        }
    };
    Ok(match name {
        Some(id) => content_disposition(&format!("aw-bucket-export_{id}.json")),
        None => content_disposition("aw-buckets-export.json"),
    })
}

#[cfg(not(any(unix, windows)))]
compile_error!("export streaming requires unix or windows anonymous pipes");

fn pipe_reader_to_file(reader: PipeReader) -> File {
    #[cfg(unix)]
    {
        File::from(std::os::fd::OwnedFd::from(reader))
    }
    #[cfg(windows)]
    {
        File::from(std::os::windows::io::OwnedHandle::from(reader))
    }
}

fn pipe_writer_to_file(writer: PipeWriter) -> File {
    #[cfg(unix)]
    {
        File::from(std::os::fd::OwnedFd::from(writer))
    }
    #[cfg(windows)]
    {
        File::from(std::os::windows::io::OwnedHandle::from(writer))
    }
}

/// Serialize on the datastore worker into a private tempfile, then copy to
/// the client pipe from this thread. The worker stays disk-paced; a slow
/// or dropped download must not stall heartbeats (see `ServerState`).
fn spawn_export_stream(
    datastore: aw_datastore::Datastore,
    bucket_id: Option<String>,
    writer: PipeWriter,
) {
    thread::spawn(move || {
        let staging = match tempfile::tempfile() {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create export staging file: {err}");
                return;
            }
        };
        let mut staging = match datastore.export_to_file(bucket_id.as_deref(), staging) {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Export stream failed: {err:?}");
                return;
            }
        };
        if let Err(err) = staging.seek(SeekFrom::Start(0)) {
            error!("Failed to rewind export staging file: {err}");
            return;
        }
        let mut writer = pipe_writer_to_file(writer);
        if let Err(err) = copy(&mut staging, &mut writer) {
            error!("Export stream copy failed: {err}");
        }
    });
}

impl BucketsExportRocket {
    pub fn new(
        datastore: &aw_datastore::Datastore,
        bucket_id: Option<&str>,
    ) -> Result<Self, HttpErrorJson> {
        // Resolve the download name and 404 missing buckets before the
        // response is built. Serialization itself runs after headers so a
        // slow export does not look like a hung connection.
        let filename = export_filename(datastore, bucket_id)?;
        Ok(Self {
            datastore: datastore.clone(),
            bucket_id: bucket_id.map(str::to_owned),
            filename,
        })
    }
}

impl<'r> Responder<'r, 'static> for BucketsExportRocket {
    fn respond_to(self, _: &Request) -> response::Result<'static> {
        let (reader, writer) = pipe().map_err(|err| {
            error!("Failed to open export pipe: {err}");
            Status::InternalServerError
        })?;
        spawn_export_stream(self.datastore, self.bucket_id, writer);
        Response::build()
            .status(Status::Ok)
            .header(Header::new("Content-Disposition", self.filename))
            .header(ContentType::JSON)
            .streamed_body(rocket::tokio::fs::File::from_std(pipe_reader_to_file(
                reader,
            )))
            .ok()
    }
}

// ── CSV streaming export ──────────────────────────────────────────────────────

fn spawn_csv_export_stream(
    datastore: aw_datastore::Datastore,
    bucket_id: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    limit: Option<u64>,
    staging: File,
    writer: PipeWriter,
) {
    thread::spawn(move || {
        // Serialize on the datastore worker, one SQL row at a time, into the
        // staging file (created during preflight, before the 200 was
        // committed). The full event set is never materialized in memory, and
        // a flush failure (e.g. full staging filesystem) surfaces as an error
        // instead of a silently truncated CSV.
        let mut staging = match datastore.export_csv_to_file(&bucket_id, start, end, limit, staging)
        {
            Ok(file) => file,
            Err(err) => {
                error!("CSV export serialization failed: {err:?}");
                return;
            }
        };
        if let Err(err) = staging.seek(SeekFrom::Start(0)) {
            error!("CSV staging rewind failed: {err}");
            return;
        }
        let mut writer = pipe_writer_to_file(writer);
        if let Err(err) = copy(&mut staging, &mut writer) {
            error!("CSV export copy failed: {err}");
        }
    });
}

pub struct BucketEventsCsvRocket {
    datastore: aw_datastore::Datastore,
    bucket_id: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    limit: Option<u64>,
    filename: String,
    staging: File,
}

impl BucketEventsCsvRocket {
    pub fn new(
        datastore: &aw_datastore::Datastore,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Self, HttpErrorJson> {
        // Resolve 404/500 before headers commit. get_bucket catches a missing
        // bucket; LIMIT 1 forces the same SQL the full export will run so a
        // down worker or a prepare/read failure still returns JSON instead of
        // a 200 with an empty CSV. The staging file is also created here: a
        // full staging filesystem is still reportable as JSON at this point.
        // Mid-stream failures after 200 cannot change the status without
        // delaying headers until serialization finishes — that hung-connection
        // behavior is what this endpoint exists to avoid (same tradeoff as
        // JSON export / #721).
        datastore.get_bucket(bucket_id)?;
        datastore.get_events(bucket_id, start, end, Some(1))?;
        let staging = tempfile::tempfile().map_err(|err| {
            HttpErrorJson::new(
                Status::InternalServerError,
                format!("Failed to create CSV staging file: {err}"),
            )
        })?;
        let filename = content_disposition(&format!("aw-events-export-{bucket_id}.csv"));
        Ok(Self {
            datastore: datastore.clone(),
            bucket_id: bucket_id.to_owned(),
            start,
            end,
            limit,
            filename,
            staging,
        })
    }
}

impl<'r> Responder<'r, 'static> for BucketEventsCsvRocket {
    fn respond_to(self, _: &Request) -> response::Result<'static> {
        let Self {
            datastore,
            bucket_id,
            start,
            end,
            limit,
            filename,
            staging,
        } = self;
        let (reader, writer) = pipe().map_err(|err| {
            error!("Failed to open CSV export pipe: {err}");
            Status::InternalServerError
        })?;
        spawn_csv_export_stream(datastore, bucket_id, start, end, limit, staging, writer);
        Response::build()
            .status(Status::Ok)
            .header(Header::new("Content-Disposition", filename))
            .header(ContentType::new("text", "csv"))
            .streamed_body(rocket::tokio::fs::File::from_std(pipe_reader_to_file(
                reader,
            )))
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
            DatastoreError::NoSuchEvent(bucket_id, event_id) => HttpErrorJson::new(
                Status::NotFound,
                format!("The requested event '{event_id}' does not exist in bucket '{bucket_id}'"),
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

#[cfg(test)]
mod tests {
    use super::{content_disposition, sanitize_header_value};

    #[test]
    fn content_disposition_quotes_the_filename() {
        // Spaces are legal in bucket ids; an unquoted filename= parameter with a
        // space is malformed and clients may drop it.
        assert_eq!(
            content_disposition("aw-events-export-my bucket.csv"),
            "attachment; filename=\"aw-events-export-my bucket.csv\""
        );
        assert_eq!(
            content_disposition("aw-buckets-export.json"),
            "attachment; filename=\"aw-buckets-export.json\""
        );
    }

    #[test]
    fn sanitize_header_value_strips_header_metacharacters() {
        assert_eq!(
            sanitize_header_value("aw-watcher-window_host"),
            "aw-watcher-window_host"
        );
        // CR/LF would split the header; quote/backslash/semicolon would end or
        // re-parameterize the filename value.
        assert_eq!(
            sanitize_header_value("evil\r\nX-Injected: 1"),
            "evil__X-Injected: 1"
        );
        assert_eq!(sanitize_header_value("a\"b\\c;d"), "a_b_c_d");
    }
}
