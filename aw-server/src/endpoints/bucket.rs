use std::collections::HashMap;
use std::collections::HashSet;

use gethostname::gethostname;
use rocket::serde::json::Json;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

use aw_datastore::DatastoreError;
use aw_models::Bucket;
use aw_models::Event;

use rocket::http::Status;
use rocket::State;

use crate::endpoints::query_cache::event_range;
use crate::endpoints::util::{ApiJson, BucketEventsCsvRocket, BucketsExportRocket};
use crate::endpoints::{HttpErrorJson, ServerState};

#[get("/")]
pub fn buckets_get(
    state: &State<ServerState>,
) -> Result<Json<HashMap<String, Bucket>>, HttpErrorJson> {
    let datastore = &state.datastore;
    match datastore.get_buckets() {
        Ok(bucketlist) => Ok(Json(bucketlist)),
        Err(err) => Err(err.into()),
    }
}

#[get("/<bucket_id>")]
pub fn bucket_get(
    bucket_id: &str,
    state: &State<ServerState>,
) -> Result<Json<Bucket>, HttpErrorJson> {
    let datastore = &state.datastore;
    match datastore.get_bucket(bucket_id) {
        Ok(bucket) => Ok(Json(bucket)),
        Err(e) => Err(e.into()),
    }
}

/// Create a new bucket
///
/// If hostname is "!local", the hostname and device_id will be set from the server info.
/// This is useful for watchers which are known/assumed to run locally but might not know their hostname (like aw-watcher-web).
#[post("/<bucket_id>", data = "<message>", format = "application/json")]
pub fn bucket_new(
    bucket_id: &str,
    message: ApiJson<Bucket>,
    state: &State<ServerState>,
) -> Result<(), HttpErrorJson> {
    let mut bucket = message.into_inner();
    if bucket.id != bucket_id {
        bucket.id = bucket_id.to_string();
    }
    // Reject client-supplied hostnames containing whitespace. Such hostnames come from
    // misconfigured clients and produce buckets that are awkward to address and query.
    //
    // Two deliberate exemptions keep this from breaking working setups:
    //  - The "!local" sentinel is resolved server-side below and is not validated here,
    //    so a machine whose own hostname contains a space is unaffected.
    //  - Buckets that already exist are not validated, so watchers that idempotently
    //    re-create a pre-existing bucket on startup keep getting the established
    //    "already exists" response instead of a new hard error.
    if bucket.hostname != "!local" && bucket.hostname.contains(char::is_whitespace) {
        // Distinguish "bucket genuinely absent" from "datastore unavailable":
        // only the former should trigger the whitespace rejection (400). An
        // InternalError means the datastore worker is down and must become 500,
        // not be silently swallowed and misreported as invalid client input.
        match state.datastore.get_bucket(&bucket.id) {
            Ok(_) => {} // existing bucket — skip validation; create_bucket returns 304
            Err(DatastoreError::NoSuchBucket(_)) => {
                // Format the hostname with Debug to escape control characters, so
                // untrusted request data cannot inject newlines into the log.
                let err_msg = format!(
                    "Invalid hostname {:?}: hostname may not contain whitespace",
                    bucket.hostname
                );
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
            }
            Err(e) => return Err(e.into()), // datastore failure → 500
        }
    }
    if bucket.hostname == "!local" {
        bucket.hostname = gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        bucket
            .data
            .insert("device_id".to_string(), state.device_id.clone().into());
    }
    let datastore = &state.datastore;
    let ret = datastore.create_bucket(&bucket);
    match ret {
        Ok(_) => {
            // A new bucket changes what the bucket list resolves to, and a
            // re-created bucket may have gained events.
            state.query_cache.clear();
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Parse an optional RFC 3339 query parameter, answering 400 when it doesn't parse.
fn parse_time_param(
    name: &str,
    value: Option<String>,
) -> Result<Option<DateTime<Utc>>, HttpErrorJson> {
    match value {
        Some(dt_str) => match DateTime::parse_from_rfc3339(&dt_str) {
            // Event times are stored as nanoseconds since the epoch, which only covers
            // 1677-2262; reject anything outside that instead of passing it on.
            Ok(dt) if dt.timestamp_nanos_opt().is_none() => {
                let err_msg = format!("{name} {dt_str} is outside the supported range (1677-2262)");
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::BadRequest, err_msg))
            }
            Ok(dt) => Ok(Some(dt.with_timezone(&Utc))),
            Err(e) => {
                let err_msg =
                    format!("Failed to parse {name}, datetime needs to be in rfc3339 format: {e}");
                warn!("{}", err_msg);
                Err(HttpErrorJson::new(Status::BadRequest, err_msg))
            }
        },
        None => Ok(None),
    }
}

#[get("/<bucket_id>/events?<start>&<end>&<limit>")]
pub fn bucket_events_get(
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>,
    state: &State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    let starttime = parse_time_param("starttime", start)?;
    let endtime = parse_time_param("endtime", end)?;
    let datastore = &state.datastore;
    let res = datastore.get_events(bucket_id, starttime, endtime, limit);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(err) => Err(err.into()),
    }
}

// Ranked below bucket_event_count so that `/events/count` isn't parsed as an event id;
// both routes take query parameters, so they would otherwise collide.
// See: https://api.rocket.rs/master/rocket/struct.Route.html#resolving-collisions
#[get("/<bucket_id>/events/<event_id>?<_unused..>", rank = 1)]
pub fn bucket_events_get_single(
    bucket_id: &str,
    event_id: i64,
    _unused: Option<u64>,
    state: &State<ServerState>,
) -> Result<Json<Event>, HttpErrorJson> {
    let datastore = &state.datastore;
    let res = datastore.get_event(bucket_id, event_id);
    match res {
        Ok(events) => Ok(Json(events)),
        Err(err) => Err(err.into()),
    }
}

#[post("/<bucket_id>/events", data = "<events>", format = "application/json")]
pub fn bucket_events_create(
    bucket_id: &str,
    events: ApiJson<Vec<Event>>,
    state: &State<ServerState>,
) -> Result<Json<Vec<Event>>, HttpErrorJson> {
    // Hold the write lock across (read old ranges + write + invalidate) so a
    // concurrent replacement cannot move an event to a range we never record.
    let _guard = state.write_lock.lock().unwrap();
    // Reject events with negative duration; they cannot represent a valid time span
    // and silently corrupt totals in the Activity view (#602, #239).
    for event in events.iter() {
        if event.duration < Duration::zero() {
            let err_msg = format!(
                "Invalid event: duration must be non-negative, got {}s",
                event.duration.num_milliseconds() as f64 / 1000.0
            );
            warn!("{}", err_msg);
            return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
        }
    }
    let datastore = &state.datastore;
    // Every inserted event changes its own extent; an event with an ID replaces
    // a stored one, whose range changes too.
    let mut affected: Vec<_> = events.iter().map(event_range).collect();
    for event in events.iter() {
        if let Some(id) = event.id {
            if let Ok(old) = datastore.get_event(bucket_id, id) {
                affected.push(event_range(&old));
            }
        }
    }
    let res = datastore.insert_events(bucket_id, &events);
    match res {
        Ok(events) => {
            state.query_cache.invalidate(affected);
            Ok(Json(events))
        }
        Err(err) => Err(err.into()),
    }
}

#[post(
    "/<bucket_id>/heartbeat?<pulsetime>",
    data = "<heartbeat_json>",
    format = "application/json"
)]
pub fn bucket_events_heartbeat(
    bucket_id: &str,
    heartbeat_json: ApiJson<Event>,
    pulsetime: f64,
    state: &State<ServerState>,
) -> Result<Json<Event>, HttpErrorJson> {
    let _guard = state.write_lock.lock().unwrap();
    // Reject negative pulsetime; it has no meaningful interpretation and is
    // almost always a client bug (negated constant, sign-flip on subtraction).
    if pulsetime < 0.0 {
        let err_msg = format!("Invalid pulsetime: must be non-negative, got {pulsetime}");
        warn!("{}", err_msg);
        return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
    }
    let heartbeat = heartbeat_json.into_inner();
    // The returned event spans every merged/replaced event, so invalidating its
    // extent covers the stored previous event even if it had a different range.
    let heartbeat_range = event_range(&heartbeat);
    let datastore = &state.datastore;
    match datastore.heartbeat(bucket_id, heartbeat, pulsetime) {
        Ok(e) => {
            state
                .query_cache
                .invalidate(vec![event_range(&e), heartbeat_range]);
            Ok(Json(e))
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(serde::Deserialize)]
pub struct BulkDeleteRequest {
    pub ids: Vec<i64>,
}

#[get("/<bucket_id>/events/count?<start>&<end>")]
pub fn bucket_event_count(
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    state: &State<ServerState>,
) -> Result<Json<u64>, HttpErrorJson> {
    let starttime = parse_time_param("starttime", start)?;
    let endtime = parse_time_param("endtime", end)?;
    let datastore = &state.datastore;
    let res = datastore.get_event_count(bucket_id, starttime, endtime);
    match res {
        Ok(eventcount) => Ok(Json(eventcount as u64)),
        Err(err) => Err(err.into()),
    }
}

#[delete("/<bucket_id>/events/<event_id>")]
pub fn bucket_events_delete_by_id(
    bucket_id: &str,
    event_id: i64,
    state: &State<ServerState>,
) -> Result<(), HttpErrorJson> {
    let _guard = state.write_lock.lock().unwrap();
    let datastore = &state.datastore;
    let old = datastore.get_event(bucket_id, event_id).ok();
    match datastore.delete_events_by_id(bucket_id, vec![event_id]) {
        Ok(_) => {
            if let Some(event) = old {
                state.query_cache.invalidate(vec![event_range(&event)]);
            }
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Delete many events from one bucket in a single request.
///
/// Body: `{"ids": [1, 2, 3]}`. Unknown ids are ignored; the response is the
/// number of events that actually existed and were deleted.
#[post(
    "/<bucket_id>/events/delete",
    data = "<body>",
    format = "application/json"
)]
pub fn bucket_events_delete_many(
    bucket_id: &str,
    body: Json<BulkDeleteRequest>,
    state: &State<ServerState>,
) -> Result<Json<u64>, HttpErrorJson> {
    // Hold the write lock across (read old ranges + delete + invalidate), as
    // bucket_events_create and bucket_events_delete_by_id do. If the read ran
    // outside the lock, a concurrent insert/heartbeat could replace an event
    // with one of the same id at a different range; the delete would remove
    // that new event while only the stale range was invalidated, and the
    // returned count could include ids another request had already deleted.
    let _guard = state.write_lock.lock().unwrap();
    let datastore = &state.datastore;

    // Validate bucket exists before iterating (handles empty-ids case too).
    datastore
        .get_bucket(bucket_id)
        .map_err(HttpErrorJson::from)?;

    // Collect the events that exist. Duplicate ids collapse to one entry.
    let mut existing: Vec<Event> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for id in body.ids.iter() {
        if !seen.insert(*id) {
            continue;
        }
        match datastore.get_event(bucket_id, *id) {
            Ok(event) => existing.push(event),
            Err(DatastoreError::NoSuchEvent(_, _)) => {}
            Err(err) => return Err(err.into()),
        }
    }
    let ids: Vec<i64> = existing.iter().filter_map(|e| e.id).collect();
    if ids.is_empty() {
        return Ok(Json(0));
    }

    // Ranges are collected under the same lock that deletes, so they describe
    // exactly the events that the delete removes.
    let ranges: Vec<_> = existing.iter().map(event_range).collect();
    match datastore.delete_events_by_id(bucket_id, ids.clone()) {
        Ok(_) => {
            state.query_cache.invalidate(ranges);
            Ok(Json(ids.len() as u64))
        }
        Err(err) => {
            // Invalidate cache even on failure: some events may have been
            // deleted by the datastore worker before the error was returned.
            state.query_cache.invalidate(ranges);
            Err(err.into())
        }
    }
}

#[get("/<bucket_id>/export")]
pub fn bucket_export(
    bucket_id: &str,
    state: &State<ServerState>,
) -> Result<BucketsExportRocket, HttpErrorJson> {
    BucketsExportRocket::new(&state.datastore, Some(bucket_id))
}

/// Stream events for a single bucket as a CSV file.
///
/// Mirrors `GET /<bucket_id>/export` (JSON) but returns `text/csv`.
/// Sends HTTP headers before serialization begins so large buckets don't
/// look like a hung connection on Android WebView or slow networks.
/// Accepts the same `start`, `end`, and `limit` query params as the JSON
/// events endpoint.
#[get("/<bucket_id>/export/csv?<start>&<end>&<limit>")]
pub fn bucket_events_get_csv(
    bucket_id: &str,
    start: Option<String>,
    end: Option<String>,
    limit: Option<u64>,
    state: &State<ServerState>,
) -> Result<BucketEventsCsvRocket, HttpErrorJson> {
    let starttime: Option<DateTime<Utc>> = match start {
        Some(dt_str) => match DateTime::parse_from_rfc3339(&dt_str) {
            Ok(dt) => Some(dt.with_timezone(&Utc)),
            Err(e) => {
                let err_msg = format!(
                    "Failed to parse starttime, datetime needs to be in rfc3339 format: {e}"
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
                let err_msg =
                    format!("Failed to parse endtime, datetime needs to be in rfc3339 format: {e}");
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
            }
        },
        None => None,
    };
    BucketEventsCsvRocket::new(&state.datastore, bucket_id, starttime, endtime, limit)
}

#[delete("/<bucket_id>")]
pub fn bucket_delete(bucket_id: &str, state: &State<ServerState>) -> Result<(), HttpErrorJson> {
    let datastore = &state.datastore;
    match datastore.delete_bucket(bucket_id) {
        Ok(_) => {
            // Removing a bucket changes what the bucket list resolves to.
            state.query_cache.clear();
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
