use std::collections::HashMap;

use rocket_contrib::{Json, Value};

use chrono::DateTime;
use chrono::Utc;

use models::bucket::Bucket;
use models::event::Event;

use rocket::State;
use rocket::http::Status;
use rocket::Response;
use rocket::response::Failure;

use super::ServerState;

use super::super::datastore::DatastoreError;

use super::super::transform;

/*
 * TODO:
 * - Make sure that the mutex will never be able to be poisoned by unwraps
 */

#[get("/", format = "application/json")]
pub fn buckets_get(state: State<ServerState>) -> Result<Json<HashMap<String, Bucket>>, rocket::Error> {
    let datastore = state.datastore.lock().unwrap();
    let bucketlist = datastore.get_buckets().unwrap();
    return Ok(Json(bucketlist));
}

#[get("/<bucket_id>", format = "application/json")]
pub fn bucket_get(bucket_id: String, state: State<ServerState>) -> Result<Json<Bucket>, Failure> {
    let datastore = state.datastore.lock().unwrap();
    match datastore.get_bucket(&bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => match e {
            DatastoreError::NoSuchBucket => Err(Failure(Status::NotFound)),
            _ => Err(Failure(Status::InternalServerError))
        }
    }
}

#[post("/<bucket_id>", format = "application/json", data = "<message>")]
pub fn bucket_new(bucket_id: String, mut message: Json<Bucket>, state: State<ServerState>) -> Response {
    let mut res = Response::new();
    if message.0.id.chars().count() == 0 {
        message.0.id = bucket_id.clone();
    } else if message.0.id != bucket_id {
        res.set_status(Status::BadRequest);
        return res;
    }
    match message.created {
        Some(_) => (),
        None => message.created = Some(Utc::now())
    }
    let mut datastore = state.datastore.lock().unwrap();
    let ret = datastore.create_bucket(&message.0);
    match ret {
        Ok(_) => res.set_status(Status::Ok),
        Err(e) => match e {
            DatastoreError::BucketAlreadyExists => res.set_status(Status::NotModified),
            _ => res.set_status(Status::InternalServerError)
        }
    };
    return res
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
    let datastore = state.datastore.lock().unwrap();
    let res = datastore.get_events(&bucket_id, starttime, endtime, constraints.limit);
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
    let datastore = state.datastore.lock().unwrap();
    let res = datastore.insert_events(&bucket_id, &events);
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
    /* TODO: Improve performance with a last_event cache */
    let datastore = state.datastore.lock().unwrap();
    let mut last_event_vec = datastore.get_events(&bucket_id, None, None, Some(1)).unwrap();
    match last_event_vec.pop() {
        None => {
            datastore.insert_events(&bucket_id, &vec![heartbeat]).unwrap();
        }
        Some(last_event) => {
            match transform::heartbeat(&last_event, &heartbeat, constraints.pulsetime) {
                None => { println!("Failed to merge!"); datastore.insert_events(&bucket_id, &vec![heartbeat]).unwrap() },
                Some(merged_heartbeat) => datastore.replace_last_event(&bucket_id, &merged_heartbeat).unwrap()
            }
        }
    }
    return Ok(());
}

#[get("/<bucket_id>/events/count", format = "application/json")]
pub fn bucket_events_count(bucket_id: String, state: State<ServerState>) -> Result<Json<Value>, Failure> {
    let datastore = state.datastore.lock().unwrap();
    let res = datastore.get_events_count(&bucket_id, None, None);
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
    let mut datastore = state.datastore.lock().unwrap();
    match datastore.delete_bucket(&bucket_id) {
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
