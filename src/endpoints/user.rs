use rocket_contrib::{Json, Value};

use chrono::Utc;

use models::Bucket;

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
    datastore::create_bucket(conn, &message.0);
    return Json(json!({ "status": "ok" }))
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
