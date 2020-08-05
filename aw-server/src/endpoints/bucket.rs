use std::collections::HashMap;
use std::io::Cursor;

use rocket_contrib::json::Json;

use chrono::DateTime;
use chrono::Utc;

use aw_models::Bucket;
use aw_models::BucketsExport;
use aw_models::Event;

use rocket::http::Header;
use rocket::http::Status;
use rocket::response::Response;
use rocket::State;

use crate::endpoints::{HttpErrorJson, ServerState};

use aw_datastore::DatastoreError;

#[get("/")]
pub fn buckets_get(
    state: State<ServerState>,
) -> Result<Json<HashMap<String, Bucket>>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.get_buckets() {
        Ok(bucketlist) => Ok(Json(bucketlist)),
        Err(e) => {
            let err_msg = format!("Unexpected error: {:?}", e);
            warn!("{}", err_msg);
            Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
        }
    }
}

#[get("/<bucket_id>")]
pub fn bucket_get(
    bucket_id: String,
    state: State<ServerState>,
) -> Result<Json<Bucket>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.get_bucket(&bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            _ => {
                let err_msg = format!("Unexpected error: {:?}", e);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[post("/<bucket_id>", data = "<message>", format = "application/json")]
pub fn bucket_new(
    bucket_id: String,
    message: Json<Bucket>,
    state: State<ServerState>,
) -> Result<(), HttpErrorJson> {
    let mut bucket = message.into_inner();
    if bucket.id != bucket_id {
        bucket.id = bucket_id;
    }
    // Cannot re-use endpoints_get_lock!() here because it returns Err(Status) on failure and this
    // function returns a Response
    let datastore = match state.datastore.lock() {
        Ok(ds) => ds,
        Err(e) => {
            warn!("Taking datastore lock failed, returning 504: {}", e);
            return Err(HttpErrorJson::new(
                Status::ServiceUnavailable,
                "Takind datastore lock failed".to_string(),
            ));
        }
    };
    let ret = datastore.create_bucket(&bucket);
    match ret {
        Ok(_) => Ok(()),
        Err(err) => match err {
            DatastoreError::BucketAlreadyExists => Err(HttpErrorJson::new(
                Status::NotModified,
                "Bucket already exists".to_string(),
            )),
            _ => {
                warn!("Unexpected error: {:?}", err);
                Err(HttpErrorJson::new(
                    Status::InternalServerError,
                    format!("{:?}", err),
                ))
            }
        },
    }
}

#[get("/<bucket_id>/events?<start>&<end>&<limit>")]
pub fn bucket_events_get(
    bucket_id: String,
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>,
    state: State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    let starttime: Option<DateTime<Utc>> = match start {
        Some(dt_str) => match DateTime::parse_from_rfc3339(&dt_str) {
            Ok(dt) => Some(dt.with_timezone(&Utc)),
            Err(e) => {
                let err_msg = format!(
                    "Failed to parse starttime, datetime needs to be in rfc3339 format: {}",
                    e
                );
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
            }
        },
        None => None,
    };
    let endtime: Option<DateTime<Utc>> = match end {
        Some(dt_str) => match DateTime::parse_from_rfc3339(&dt_str) {
            Ok(dt) => Some(dt.with_timezone(&Utc)),
            Err(e) => {
                let err_msg = format!(
                    "Failed to parse endtime, datetime needs to be in rfc3339 format: {}",
                    e
                );
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
            }
        },
        None => None,
    };
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.get_events(&bucket_id, starttime, endtime, limit);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            e => {
                let err_msg = format!("Failed to fetch events: {:?}", e);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[post("/<bucket_id>/events", data = "<events>", format = "application/json")]
pub fn bucket_events_create(
    bucket_id: String,
    events: Json<Vec<Event>>,
    state: State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.insert_events(&bucket_id, &events);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            e => {
                let err_msg = format!("Failed to create event(s): {:?}", e);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[post(
    "/<bucket_id>/heartbeat?<pulsetime>",
    data = "<heartbeat_json>",
    format = "application/json"
)]
pub fn bucket_events_heartbeat(
    bucket_id: String,
    heartbeat_json: Json<Event>,
    pulsetime: f64,
    state: State<ServerState>,
) -> Result<Json<Event>, HttpErrorJson> {
    let heartbeat = heartbeat_json.into_inner();
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.heartbeat(&bucket_id, heartbeat, pulsetime) {
        Ok(e) => Ok(Json(e)),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            err => {
                let err_msg = format!("Heartbeat failed: {:?}", err);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[get("/<bucket_id>/events/count")]
pub fn bucket_event_count(
    bucket_id: String,
    state: State<ServerState>,
) -> Result<Json<u64>, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.get_event_count(&bucket_id, None, None);
    match res {
        Ok(eventcount) => Ok(Json(eventcount as u64)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            e => {
                let err_msg = format!("Failed to count events: {:?}", e);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[delete("/<bucket_id>/events/<event_id>")]
pub fn bucket_events_delete_by_id(
    bucket_id: String,
    event_id: i64,
    state: State<ServerState>,
) -> Result<(), HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.delete_events_by_id(&bucket_id, vec![event_id]) {
        Ok(_) => Ok(()),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            err => {
                let err_msg = format!("Delete events by id failed: {:?}", err);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}

#[get("/<bucket_id>/export")]
pub fn bucket_export(
    bucket_id: String,
    state: State<ServerState>,
) -> Result<Response, HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    let mut export = BucketsExport {
        buckets: HashMap::new(),
    };
    let mut bucket = match datastore.get_bucket(&bucket_id) {
        Ok(bucket) => bucket,
        Err(err) => match err {
            DatastoreError::NoSuchBucket => {
                return Err(HttpErrorJson::new(
                    Status::NotFound,
                    "The requested bucket does not exist".to_string(),
                ))
            }
            e => {
                let err_msg = format!("Failed to fetch events: {:?}", e);
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::InternalServerError, err_msg));
            }
        },
    };
    bucket.events = Some(
        datastore
            .get_events(&bucket_id, None, None, None)
            .expect("Failed to get events for bucket"),
    );
    export.buckets.insert(bucket_id.clone(), bucket);
    let filename = format!("aw-bucket-export_{}.json", bucket_id);

    let header_content = format!("attachment; filename={}", filename);
    Ok(Response::build()
        .status(Status::Ok)
        .header(Header::new("Content-Disposition", header_content))
        .sized_body(Cursor::new(
            serde_json::to_string(&export).expect("Failed to serialize"),
        ))
        .finalize())
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerState>) -> Result<(), HttpErrorJson> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.delete_bucket(&bucket_id) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(HttpErrorJson::new(
                Status::NotFound,
                "The requested bucket does not exist".to_string(),
            )),
            e => {
                let err_msg = format!("Failed to delete bucket: {:?}", e);
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::InternalServerError, err_msg))
            }
        },
    }
}
