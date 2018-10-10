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

use super::super::transform;

pub type BucketList = Vec<Bucket>;

#[get("/", format = "application/json")]
pub fn buckets_get(state: State<ServerState>) -> Result<Json<BucketList>, rocket::Error> {
    let bucketlist = state.datastore.get_buckets().unwrap();
    return Ok(Json(bucketlist));
}

#[get("/<bucket_id>", format = "application/json")]
pub fn bucket_get(bucket_id: String, state: State<ServerState>) -> Result<Json<Bucket>, Failure> {
    match state.datastore.get_bucket(&bucket_id).unwrap() {
        Some(bucket) => Ok(Json(bucket)),
        None => Err(Failure(rocket::http::Status::NotFound))
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
    let ret = state.datastore.create_bucket(&message.0).unwrap();
    match ret {
        true => res.set_status(Status::Ok),
        false => res.set_status(Status::NotModified)
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
pub fn bucket_events_get(bucket_id: String, constraints: GetEventsConstraints, state: State<ServerState>) -> Json<Value> {
    if state.datastore.get_bucket(&bucket_id).is_err() {
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
    Json(json!(state.datastore.get_events(&bucket_id, starttime, endtime, constraints.limit).unwrap()))
}

#[post("/<bucket_id>/events", format = "application/json", data = "<events>")]
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, state: State<ServerState>) -> Json<Value> {
    if state.datastore.get_bucket(&bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    state.datastore.insert_events(&bucket_id, &events).unwrap();
    return Json(json!({"status": "ok"}))
}

#[derive(FromForm)]
pub struct HeartbeatConstraints {
    pulsetime: f64,
}

#[post("/<bucket_id>/heartbeat?<constraints>", format = "application/json", data = "<heartbeat_json>")]
pub fn bucket_events_heartbeat(bucket_id: String, heartbeat_json: Json<Event>, constraints: HeartbeatConstraints, state: State<ServerState>) -> Json<Value> {
    let heartbeat = heartbeat_json.into_inner();
    if state.datastore.get_bucket(&bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    /* TODO: Improve performance with a last_event cache */
    let mut last_event_vec = state.datastore.get_events(&bucket_id, None, None, Some(1)).unwrap();
    match last_event_vec.pop() {
        None => {
            state.datastore.insert_events(&bucket_id, &vec![heartbeat]).unwrap();
        }
        Some(last_event) => {
            match transform::heartbeat(&last_event, &heartbeat, constraints.pulsetime) {
                None => { println!("Failed to merge!"); state.datastore.insert_events(&bucket_id, &vec![heartbeat]).unwrap() },
                Some(merged_heartbeat) => state.datastore.replace_last_event(&bucket_id, &merged_heartbeat).unwrap()
            }
        }
    }
    return Json(json!({"status": "ok"}))
}

#[get("/<bucket_id>/events/count", format = "application/json")]
pub fn bucket_events_count(bucket_id: String, state: State<ServerState>) -> Json<Value> {
    if state.datastore.get_bucket(&bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    let eventcount = state.datastore.get_events_count(&bucket_id, None, None).unwrap();
    return Json(json!({ "count": eventcount }))
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, state: State<ServerState>) -> Option<Json<Value>> {
    match state.datastore.delete_bucket(&bucket_id) {
        Ok(_) => Some(Json(json!({ "status": "ok" }))),
        Err(_) => None
    }
}
