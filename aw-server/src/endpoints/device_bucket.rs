//! Device-addressed adapters for the existing bucket handlers. Shared helpers
//! retain validation, write locking and query invalidation; scope travels with
//! the datastore handle, including the background streaming export workers.
use std::collections::HashMap;

use aw_models::{Bucket, Event};
use rocket::{serde::json::Json, State};

use super::bucket::{self, BulkDeleteRequest};
use super::util::{ApiJson, BucketEventsCsvRocket, BucketsExportRocket};
use super::{HttpErrorJson, ServerState};

#[get("/<device_id>/buckets")]
pub fn buckets_get(
    device_id: &str,
    state: &State<ServerState>,
) -> Result<Json<HashMap<String, Bucket>>, HttpErrorJson> {
    Ok(Json(state.bucket_datastore(Some(device_id)).get_buckets()?))
}

#[get("/<device_id>/buckets/<bucket_id>")]
pub fn bucket_get(
    device_id: &str,
    bucket_id: &str,
    state: &State<ServerState>,
) -> Result<Json<Bucket>, HttpErrorJson> {
    Ok(Json(
        state
            .bucket_datastore(Some(device_id))
            .get_bucket(bucket_id)?,
    ))
}

#[post(
    "/<device_id>/buckets/<bucket_id>",
    data = "<message>",
    format = "application/json"
)]
pub fn bucket_new(
    device_id: &str,
    bucket_id: &str,
    message: ApiJson<Bucket>,
    state: &State<ServerState>,
) -> Result<(), HttpErrorJson> {
    bucket::bucket_new_in(
        bucket_id,
        message,
        state,
        &state.bucket_datastore(Some(device_id)),
        state.normalize_device(device_id),
    )
}

#[delete("/<device_id>/buckets/<bucket_id>")]
pub fn bucket_delete(
    device_id: &str,
    bucket_id: &str,
    state: &State<ServerState>,
) -> Result<(), HttpErrorJson> {
    bucket::bucket_delete_in(bucket_id, state, &state.bucket_datastore(Some(device_id)))
}

#[get("/<device_id>/buckets/<bucket_id>/events?<start>&<end>&<limit>")]
pub fn events_get(
    device_id: &str,
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>,
    state: &State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    bucket::bucket_events_get_in(
        bucket_id,
        start,
        end,
        limit,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[get(
    "/<device_id>/buckets/<bucket_id>/events/<event_id>?<_unused..>",
    rank = 1
)]
pub fn event_get(
    device_id: &str,
    bucket_id: &str,
    event_id: i64,
    _unused: Option<u64>,
    state: &State<ServerState>,
) -> Result<Json<Event>, HttpErrorJson> {
    Ok(Json(
        state
            .bucket_datastore(Some(device_id))
            .get_event(bucket_id, event_id)?,
    ))
}

#[post(
    "/<device_id>/buckets/<bucket_id>/events",
    data = "<events>",
    format = "application/json"
)]
pub fn events_create(
    device_id: &str,
    bucket_id: &str,
    events: ApiJson<Vec<Event>>,
    state: &State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    bucket::bucket_events_create_in(
        bucket_id,
        events,
        state,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[post(
    "/<device_id>/buckets/<bucket_id>/heartbeat?<pulsetime>",
    data = "<heartbeat>",
    format = "application/json"
)]
pub fn heartbeat(
    device_id: &str,
    bucket_id: &str,
    heartbeat: ApiJson<Event>,
    pulsetime: f64,
    state: &State<ServerState>,
) -> Result<Json<Event>, HttpErrorJson> {
    bucket::bucket_events_heartbeat_in(
        bucket_id,
        heartbeat,
        pulsetime,
        state,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[get("/<device_id>/buckets/<bucket_id>/events/count?<start>&<end>")]
pub fn event_count(
    device_id: &str,
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    state: &State<ServerState>,
) -> Result<Json<u64>, HttpErrorJson> {
    bucket::bucket_event_count_in(
        bucket_id,
        start,
        end,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[delete("/<device_id>/buckets/<bucket_id>/events/<event_id>")]
pub fn event_delete(
    device_id: &str,
    bucket_id: &str,
    event_id: i64,
    state: &State<ServerState>,
) -> Result<(), HttpErrorJson> {
    bucket::bucket_events_delete_by_id_in(
        bucket_id,
        event_id,
        state,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[post(
    "/<device_id>/buckets/<bucket_id>/events/delete",
    data = "<body>",
    format = "application/json"
)]
pub fn events_delete_many(
    device_id: &str,
    bucket_id: &str,
    body: Json<BulkDeleteRequest>,
    state: &State<ServerState>,
) -> Result<Json<u64>, HttpErrorJson> {
    bucket::bucket_events_delete_many_in(
        bucket_id,
        body,
        state,
        &state.bucket_datastore(Some(device_id)),
    )
}

#[get("/<device_id>/buckets/<bucket_id>/export")]
pub fn bucket_export(
    device_id: &str,
    bucket_id: &str,
    state: &State<ServerState>,
) -> Result<BucketsExportRocket, HttpErrorJson> {
    BucketsExportRocket::new(&state.bucket_datastore(Some(device_id)), Some(bucket_id))
}

#[get("/<device_id>/export")]
pub fn buckets_export(
    device_id: &str,
    state: &State<ServerState>,
) -> Result<BucketsExportRocket, HttpErrorJson> {
    BucketsExportRocket::new(&state.bucket_datastore(Some(device_id)), None)
}

#[get("/<device_id>/buckets/<bucket_id>/export/csv?<start>&<end>&<limit>")]
pub fn events_csv(
    device_id: &str,
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>,
    state: &State<ServerState>,
) -> Result<BucketEventsCsvRocket, HttpErrorJson> {
    bucket::bucket_events_get_csv_in(
        bucket_id,
        start,
        end,
        limit,
        &state.bucket_datastore(Some(device_id)),
    )
}
