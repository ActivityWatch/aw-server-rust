use rocket_contrib::{Json, Value};

use chrono::DateTime;
use chrono::Utc;

use models::bucket::Bucket;
use models::event::Event;

use rocket::State;
use rocket::http::Status;
use super::ServerStateMutex;

use super::super::datastore;
use super::super::transform;

pub type BucketList = Vec<Bucket>;

#[get("/", format = "application/json")]
pub fn buckets_get(state: State<ServerStateMutex>) -> Result<Json<BucketList>, rocket::Error> {
    let conn = &state.lock().unwrap().dbconnection;
    let bucketlist = datastore::get_buckets(conn).unwrap();
    return Ok(Json(bucketlist));
}

#[get("/<bucket_id>", format = "application/json")]
pub fn bucket_get(bucket_id: String, state: State<ServerStateMutex>) -> Result<Json<Bucket>, rocket::http::Status> {
    let conn = &state.lock().unwrap().dbconnection;
    match datastore::get_bucket(conn, &bucket_id).unwrap() {
        Some(bucket) => Ok(Json(bucket)),
        None => Err(Status::NotFound)
    }
}

#[post("/<bucket_id>", format = "application/json", data = "<message>")]
pub fn bucket_new(bucket_id: String, mut message: Json<Bucket>, state: State<ServerStateMutex>) -> Json<Value> {
    let conn = &state.lock().unwrap().dbconnection;
    if message.0.id.chars().count() == 0 {
        message.0.id = bucket_id.clone();
    } else if message.0.id != bucket_id {
        // TODO: Return 400
        println!("{},{}", message.0.id, bucket_id);
        return Json(json!({
            "status": "error",
            "reason": "BucketID in URL and body doesn't match!"
        }))
    }
    if datastore::get_bucket(conn, &bucket_id).is_ok() {
        // TODO: Respond 304
        return Json(json!({
            "status": "error",
            "reason": "BucketID exists. 304"
        }))
    }
    match message.created {
        Some(_) => (),
        None => message.created = Some(Utc::now())
    }
    datastore::create_bucket(conn, &message.0).unwrap();
    return Json(json!({ "status": "ok" }))
}

#[derive(FromForm)]
pub struct GetEventsConstraints {
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>
}

/* FIXME: optional constraints do not work, you always need a ? in the request */
#[get("/<bucket_id>/events?<constraints>", format = "application/json")]
pub fn bucket_events_get(bucket_id: String, constraints: GetEventsConstraints, state: State<ServerStateMutex>) -> Json<Value> {
    let conn = &state.lock().unwrap().dbconnection;
    if datastore::get_bucket(conn, &bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    let starttime : Option<DateTime<Utc>> = match constraints.start {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(_e) => return Json(json!({
                    "status": "error",
                    "reason": "failed to parse starttime, datetime needs to be in rfc3339 format (similar to iso8601 but always full)",
                }))
            }
        },
        None => None
    };
    let endtime : Option<DateTime<Utc>> = match constraints.end {
        Some(dt_str) => {
            match DateTime::parse_from_rfc3339(&dt_str) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(_e) => return Json(json!({
                    "status": "error",
                    "reason": "failed to parse endtime, datetime needs to be in rfc3339 format (similar to iso8601 but always full)",
                }))
            }
        },
        None => None
    };
    Json(json!(datastore::get_events(conn, &bucket_id, starttime, endtime, constraints.limit).unwrap()))
}

#[post("/<bucket_id>/events", format = "application/json", data = "<events>")]
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, state: State<ServerStateMutex>) -> Json<Value> {
    let conn = &state.lock().unwrap().dbconnection;
    if datastore::get_bucket(conn, &bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    datastore::insert_events(&conn, &bucket_id, &events).unwrap();
    return Json(json!({"status": "ok"}))
}

#[derive(FromForm)]
pub struct HeartbeatConstraints {
    pulsetime: f64,
}

#[post("/<bucket_id>/heartbeat?<constraints>", format = "application/json", data = "<heartbeat_json>")]
pub fn bucket_events_heartbeat(bucket_id: String, heartbeat_json: Json<Event>, constraints: HeartbeatConstraints, state: State<ServerStateMutex>) -> Json<Value> {
    let heartbeat = heartbeat_json.into_inner();
    let conn = &state.lock().unwrap().dbconnection;
    if datastore::get_bucket(conn, &bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    /* TODO: Improve performance with a last_event cache */
    let mut last_event_vec = datastore::get_events(&conn, &bucket_id, None, None, Some(1)).unwrap();
    match last_event_vec.pop() {
        None => {
            datastore::insert_events(&conn, &bucket_id, &vec![heartbeat]).unwrap();
        }
        Some(last_event) => {
            match transform::heartbeat(&last_event, &heartbeat, constraints.pulsetime) {
                None => { println!("Failed to merge!"); datastore::insert_events(&conn, &bucket_id, &vec![heartbeat]).unwrap() },
                Some(merged_heartbeat) => datastore::replace_last_event(&conn, &bucket_id, &merged_heartbeat).unwrap()
            }
        }
    }
    return Json(json!({"status": "ok"}))
}

#[get("/<bucket_id>/events/count", format = "application/json")]
pub fn bucket_events_count(bucket_id: String, state: State<ServerStateMutex>) -> Json<Value> {
    let conn = &state.lock().unwrap().dbconnection;
    if datastore::get_bucket(conn, &bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    let eventcount = datastore::get_events_count(&conn, &bucket_id, None, None).unwrap();
    return Json(json!({ "count": eventcount }))
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerStateMutex>) -> Option<Json<Value>> {
    let conn = &state.lock().unwrap().dbconnection;
    match datastore::delete_bucket(conn, &bucket_id) {
        Ok(_) => Some(Json(json!({ "status": "ok" }))),
        Err(_) => None
    }
}
