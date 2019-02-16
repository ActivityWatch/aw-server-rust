use std::collections::HashMap;

use rocket_contrib::json::Json;

use chrono::DateTime;
use chrono::Utc;

use models::Bucket;
use models::Event;

use rocket::State;
use rocket::http::Status;
use rocket::Response;

use endpoints::ServerState;

use datastore::DatastoreError;

/*
 * TODO:
 * - Fix some unwraps
 */

macro_rules! response_status {
    ($status:expr) => ({
        let mut res = Response::new();
        res.set_status($status);
        res
    })
}

#[get("/")]
pub fn buckets_get(state: State<ServerState>) -> Json<HashMap<String, Bucket>> {
    let bucketlist = state.datastore.get_buckets().unwrap();
    return Json(bucketlist);
}

#[get("/<bucket_id>")]
pub fn bucket_get(bucket_id: String, state: State<ServerState>) -> Result<Json<Bucket>, Status> {
    match state.datastore.get_bucket(&bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            _ => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<bucket_id>", data = "<message>")]
pub fn bucket_new(bucket_id: String, message: Json<Bucket>, state: State<ServerState>) -> Response {
    let mut bucket = message.into_inner();
    if bucket.id != bucket_id {
        bucket.id = bucket_id;
    }
    let ret = state.datastore.create_bucket(&bucket);
    match ret {
        Ok(_) => response_status!(Status::Ok),
        Err(e) => match e {
            DatastoreError::BucketAlreadyExists => response_status!(Status::NotModified),
            _ => {
                println!("Unexpected error: {:?}", e);
                response_status!(Status::InternalServerError)
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
                    println!("Failed to parse starttime, datetime needs to be in rfc3339 format: {}", e);
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
                    println!("Failed to parse endtime, datetime needs to be in rfc3339 format: {}", e);
                    return Err(Status::BadRequest);
                }
            }
        },
        None => None
    };
    let res = state.datastore.get_events(&bucket_id, starttime, endtime, limit);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<bucket_id>/events", data = "<events>")]
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, state: State<ServerState>) -> Result<(), Status> {
    let res = state.datastore.insert_events(&bucket_id, &events);
    match res {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[post("/<bucket_id>/heartbeat?<pulsetime>", data = "<heartbeat_json>")]
pub fn bucket_events_heartbeat(bucket_id: String, heartbeat_json: Json<Event>, pulsetime: f64, state: State<ServerState>) -> Result<(), Status> {
    let heartbeat = heartbeat_json.into_inner();
    match state.datastore.heartbeat(&bucket_id, heartbeat, pulsetime) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[get("/<bucket_id>/events/count")]
pub fn bucket_event_count(bucket_id: String, state: State<ServerState>) -> Result<Json<u64>, Status> {
    let res = state.datastore.get_event_count(&bucket_id, None, None);
    match res {
        Ok(eventcount) => Ok(Json(eventcount as u64)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerState>) -> Result<(), Status> {
    match state.datastore.delete_bucket(&bucket_id) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Status::NotFound),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Status::InternalServerError)
            }
        }
    }
}
