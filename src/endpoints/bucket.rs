use rocket_contrib::{Json, Value};

use chrono::DateTime;
use chrono::Utc;

use models::bucket::Bucket;
use models::event::Event;

use rocket::State;
use super::ServerStateMutex;

use super::super::datastore;

pub type BucketList = Vec<Bucket>;

#[get("/", format = "application/json")]
pub fn buckets_get(map: State<ServerStateMutex>) -> Json<BucketList> {
    let conn = &map.lock().unwrap().dbconnection;
    let bucketlist = datastore::get_buckets(conn).unwrap();
    return Json(bucketlist);
}

#[get("/<bucket_id>", format = "application/json")]
pub fn bucket_get(bucket_id: String, map: State<ServerStateMutex>) -> Option<Json<Bucket>> {
    let conn = &map.lock().unwrap().dbconnection;
    return Some(Json(datastore::get_bucket(conn, &bucket_id).unwrap()));
}

#[post("/<bucket_id>", format = "application/json", data = "<message>")]
pub fn bucket_new(bucket_id: String, mut message: Json<Bucket>, map: State<ServerStateMutex>) -> Json<Value> {
    let conn = &map.lock().unwrap().dbconnection;
    if message.0.id != bucket_id {
        // TODO: Return 400
        return Json(json!({
            "status": "error",
            "reason": "BucketID in URL and body doesn't match!"
        }))
    }
    else if datastore::get_bucket(conn, &bucket_id).is_ok() {
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
pub struct Constraints {
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>
}

/* FIXME: optional constraints do not work, you always need a ? in the request */
#[get("/<bucket_id>/events?<constraints>", format = "application/json")]
pub fn bucket_events_get(bucket_id: String, constraints: Constraints, map: State<ServerStateMutex>) -> Json<Value> {
    let conn = &map.lock().unwrap().dbconnection;
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
pub fn bucket_events_create(bucket_id: String, events: Json<Vec<Event>>, map: State<ServerStateMutex>) -> Json<Value> {
    let conn = &map.lock().unwrap().dbconnection;
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

#[get("/<bucket_id>/events/count", format = "application/json")]
pub fn bucket_event_count(bucket_id: String, map: State<ServerStateMutex>) -> Json<Value> {
    let conn = &map.lock().unwrap().dbconnection;
    if datastore::get_bucket(conn, &bucket_id).is_err() {
        // TODO: Respond 400
        return Json(json!({
            "status": "error",
            "reason": "Bucket with that ID doesn't exist"
        }))
    }
    return Json(json!({ "count": "1" }))
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: String, map: State<ServerStateMutex>) -> Option<Json<Value>> {
    let conn = &map.lock().unwrap().dbconnection;
    match datastore::delete_bucket(conn, &bucket_id) {
        Ok(_) => Some(Json(json!({ "status": "ok" }))),
        Err(_) => None
    }
}
