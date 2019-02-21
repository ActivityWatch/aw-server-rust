use std::collections::HashMap;

use rocket_contrib::json::Json;

use chrono::DateTime;
use chrono::Utc;

use models::Bucket;
use models::Event;

use rocket::State;
use rocket::response::status;
use rocket::http::Status;

use endpoints::ServerState;

use datastore::DatastoreError;

/*
 * TODO:
 * - Fix some unwraps
 */

#[get("/")]
pub fn buckets_get(state: State<ServerState>) -> Result<Json<HashMap<String, Bucket>>, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    let bucketlist = datastore.get_buckets().unwrap();
    return Ok(Json(bucketlist));
}

#[get("/<bucket_id>")]
pub fn bucket_get(bucket_id: String, state: State<ServerState>) -> Result<Json<Bucket>, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.get_bucket(&bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            _ => {
                warn!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

// FIXME: The status::Custom return is used because if we simply used Status and return a
// Status::NotModified rocket will for some unknown reason consider that to be a
// "Invalid status used as responder" and converts it to a 500 which we do not want.
#[post("/<bucket_id>", data = "<message>")]
pub fn bucket_new(bucket_id: String, message: Json<Bucket>, state: State<ServerState>) -> status::Custom<()> {
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
            return status::Custom(Status::ServiceUnavailable, ());
        }
    };
    let ret = datastore.create_bucket(&bucket);
    match ret {
        Ok(_) => status::Custom(Status::Ok, ()),
        Err(e) => match e {
            DatastoreError::BucketAlreadyExists => status::Custom(Status::NotModified, ()),
            _ => {
                warn!("Unexpected error: {:?}", e);
                status::Custom(Status::InternalServerError, ())
            }
        }
    }
}

#[get("/<bucket_id>/events?<start>&<end>&<limit>")]
pub fn bucket_events_get(bucket_id: String, start: Option<String>, end: Option<String>, limit: Option<u64>, state: State<ServerState>) -> Result<Json<Vec<Event>>, Status> {
    let starttime : Option<DateTime<Utc>> = match start {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(e) => {
                    warn!("Failed to parse starttime, datetime needs to be in rfc3339 format: {}", e);
                    return Err(Status::BadRequest);
                }
            }
        },
        None => None
    };
    let endtime : Option<DateTime<Utc>> = match end {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(e) => {
                    warn!("Failed to parse endtime, datetime needs to be in rfc3339 format: {}", e);
                    return Err(Status::BadRequest);
                }
            }
        },
        None => None
    };
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.get_events(&bucket_id, starttime, endtime, limit);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                warn!("Failed to fetch events: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<bucket_id>/events", data = "<events>")]
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, state: State<ServerState>) -> Result<(), Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.insert_events(&bucket_id, &events);
    match res {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                warn!("Failed to create event(s): {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<bucket_id>/heartbeat?<pulsetime>", data = "<heartbeat_json>")]
pub fn bucket_events_heartbeat(bucket_id: String, heartbeat_json: Json<Event>, pulsetime: f64, state: State<ServerState>) -> Result<(), Status> {
    let heartbeat = heartbeat_json.into_inner();
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.heartbeat(&bucket_id, heartbeat, pulsetime) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                warn!("Heartbeat failed: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[get("/<bucket_id>/events/count")]
pub fn bucket_event_count(bucket_id: String, state: State<ServerState>) -> Result<Json<u64>, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    let res = datastore.get_event_count(&bucket_id, None, None);
    match res {
        Ok(eventcount) => Ok(Json(eventcount as u64)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                warn!("Failed to count events: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerState>) -> Result<(), Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.delete_bucket(&bucket_id) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                warn!("Failed to delete bucket: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}
