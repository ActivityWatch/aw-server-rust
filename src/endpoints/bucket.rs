use std::collections::HashMap;

use rocket_contrib::{Json, Value};

use chrono::DateTime;
use chrono::Utc;

use models::Bucket;
use models::Event;

use rocket::State;
use rocket::http::Status;
use rocket::Response;
use rocket::response::Failure;

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

#[get("/", format = "application/json")]
pub fn buckets_get(state: State<ServerState>) -> Result<Json<HashMap<String, Bucket>>, rocket::Error> {
    let bucketlist = state.datastore.get_buckets().unwrap();
    return Ok(Json(bucketlist));
}

#[get("/<bucket_id>", format = "application/json")]
pub fn bucket_get(bucket_id: String, state: State<ServerState>) -> Result<Json<Bucket>, Failure> {
    match state.datastore.get_bucket(&bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            _ => Err(Failure(Status::InternalServerError))
        }
    }
}

#[post("/<bucket_id>", format = "application/json", data = "<message>")]
pub fn bucket_new(bucket_id: String, message: Json<Bucket>, state: State<ServerState>) -> Response {
    let bucket = message.into_inner();
    if bucket.id != bucket_id {
        println!("endpoint bucketid doesn't match payload bucketid");
        return response_status!(Status::BadRequest)
    }
    let ret = state.datastore.create_bucket(&bucket);
    match ret {
        Ok(_) => response_status!(Status::Ok),
        Err(e) => match e {
            DatastoreError::BucketAlreadyExists => response_status!(Status::NotModified),
            _ => response_status!(Status::InternalServerError)
        }
    }
}

#[derive(FromForm)]
pub struct GetEventsConstraints {
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>
}

/* FIXME: optional constraints do not work, you always need a ? in the request */
#[get("/<bucket_id>/events?<constraints>", format = "application/json")]
pub fn bucket_events_get(bucket_id: String, constraints: GetEventsConstraints, state: State<ServerState>) -> Result<Json<Value>, Failure> {
    let starttime : Option<DateTime<Utc>> = match constraints.start {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(e) => {
                    println!("Failed to parse starttime, datetime needs to be in rfc3339 format: {}", e);
                    return Err(Failure(Status::BadRequest));
                }
            }
        },
        None => None
    };
    let endtime : Option<DateTime<Utc>> = match constraints.end {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(e) => {
                    println!("Failed to parse endtime, datetime needs to be in rfc3339 format: {}", e);
                    return Err(Failure(Status::BadRequest));
                }
            }
        },
        None => None
    };
    let res = state.datastore.get_events(&bucket_id, starttime, endtime, constraints.limit);
    match res {
        Ok(events) => Ok(Json(json!(events))),
        Err(err) => match err {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Failure(Status::InternalServerError))
            }
        }
    }
}

#[post("/<bucket_id>/events", format = "application/json", data = "<events>")]
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, state: State<ServerState>) -> Result<(), Failure> {
    let res = state.datastore.insert_events(&bucket_id, &events);
    match res {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Failure(Status::InternalServerError))
            }
        }
    }
}

#[derive(FromForm)]
pub struct HeartbeatConstraints {
    pulsetime: f64,
}

// TODO: Improve this endpoint!
#[post("/<bucket_id>/heartbeat?<constraints>", format = "application/json", data = "<heartbeat_json>")]
pub fn bucket_events_heartbeat(bucket_id: String, heartbeat_json: Json<Event>, constraints: HeartbeatConstraints, state: State<ServerState>) -> Result<(), Failure> {
    let heartbeat = heartbeat_json.into_inner();
    match state.datastore.heartbeat(&bucket_id, heartbeat, constraints.pulsetime) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Failure(Status::InternalServerError))
            }
        }
    }
}

#[get("/<bucket_id>/events/count", format = "application/json")]
pub fn bucket_event_count(bucket_id: String, state: State<ServerState>) -> Result<Json<Value>, Failure> {
    let res = state.datastore.get_event_count(&bucket_id, None, None);
    match res {
        Ok(eventcount) => Ok(Json(json!({"count": eventcount}))),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Failure(Status::InternalServerError))
            }
        }
    }
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerState>) -> Result<(), Failure> {
    match state.datastore.delete_bucket(&bucket_id) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            e => {
                println!("Unexpected error: {:?}", e);
                Err(Failure(Status::InternalServerError))
            }
        }
    }
}
