use rocket::form::Form;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;

use std::collections::{BTreeMap, HashMap, HashSet};

use aw_models::{BucketsExport, Event, TryVec};

use aw_datastore::DatastoreError;

use crate::endpoints::util::ApiJson;
use crate::endpoints::{HttpErrorJson, ServerState};

/// Computes a dedup identity tuple for an event.
///
/// Uses canonical JSON serialization (sorted keys via `BTreeMap`) so that
/// events with identical key-value pairs but different insertion order
/// (e.g., from different clients) are correctly identified as duplicates.
fn event_identity(
    event: &Event,
) -> Result<(chrono::DateTime<chrono::Utc>, i64, String), HttpErrorJson> {
    let duration_ns = event.duration.num_nanoseconds().ok_or_else(|| {
        HttpErrorJson::new(
            Status::InternalServerError,
            "Failed to encode event duration for dedup".to_string(),
        )
    })?;
    // Sort keys before serializing for canonical, order-independent dedup.
    // This prevents missed duplicates when events from different clients
    // serialize the same data with different key orderings.
    let sorted: BTreeMap<_, _> = event.data.iter().collect();
    let data_json = serde_json::to_string(&sorted).map_err(|e| {
        HttpErrorJson::new(
            Status::InternalServerError,
            format!("Failed to encode event data for dedup: {e}"),
        )
    })?;
    Ok((event.timestamp, duration_ns, data_json))
}

fn import(
    state: &ServerState,
    import: BucketsExport,
    device: Option<&str>,
) -> Result<(), HttpErrorJson> {
    let datastore = state.bucket_datastore(device);
    for (_bucketname, mut bucket) in import.buckets {
        // Match HTTP routing before resolving aliases or checking merge ownership.
        // An export of this server UUID must not create a second local owner.
        bucket.device_id = match device {
            Some(device) => state.normalize_device(device).to_owned(),
            None if bucket.device_id.is_empty() => "local".to_owned(),
            None => state.normalize_device(&bucket.device_id).to_owned(),
        };
        // Event ids are unique across all buckets in the datastore, and events are inserted
        // with INSERT OR REPLACE. An exported id (e.g. from another server) could therefore
        // overwrite an unrelated event, so imported events get new ids, except when merging into
        // an existing bucket, where an id may match that bucket's own event (see below).
        let imported = bucket.events.take().map(|e| e.take_inner());
        // Attach a stripped clone so create_bucket inserts events in the same
        // datastore request (same SQLite transaction). Splitting create + insert
        // across two requests can persist an empty bucket if insert fails or the
        // process dies in between. Keep `imported` with original ids for the
        // existing-bucket merge path. Do not consult get_bucket: it is cache-only
        // and can report a bucket after a rolled-back create; create_bucket itself
        // reports BucketAlreadyExists from the SQL constraint.
        if let Some(ref events) = imported {
            if !events.is_empty() {
                let mut stripped = events.clone();
                for event in &mut stripped {
                    event.id = None;
                }
                bucket.events = Some(TryVec::new(stripped));
            }
        }
        // Imports already name their device. Canonicalize that scoped alias
        // before using name-addressed creation/merge commands; a legacy watcher
        // POST must instead reject when its unscoped writes would select a peer.
        let incoming_scope = if bucket.device_id.is_empty() {
            "local"
        } else {
            &bucket.device_id
        };
        match datastore.get_bucket_for_device(incoming_scope, &bucket.id) {
            Ok(existing) => bucket.id = existing.id,
            Err(DatastoreError::NoSuchBucket(_)) => {}
            Err(err) => return Err(err.into()),
        }
        match datastore.create_bucket(&bucket) {
            Ok(_) => (),
            Err(DatastoreError::BucketAlreadyExists(canonical_id)) => {
                let existing = match datastore.get_bucket(&canonical_id) {
                    Ok(existing) => existing,
                    Err(DatastoreError::NoSuchBucket(_)) if device.is_some() => {
                        // The temporary global-name guard can reject a name
                        // owned by another device. Never merge outside scope.
                        return Err(HttpErrorJson::new(
                            Status::Conflict,
                            format!("Cannot import bucket '{canonical_id}': name is owned by another device"),
                        ));
                    }
                    Err(err) => return Err(err.into()),
                };
                let incoming_device = if bucket.device_id.is_empty() {
                    "local"
                } else {
                    &bucket.device_id
                };
                if existing.device_id != incoming_device {
                    return Err(HttpErrorJson::new(
                        Status::Conflict,
                        format!(
                            "Cannot merge bucket '{}': device identity differs",
                            bucket.id
                        ),
                    ));
                }
                // Retain the stored spelling when importing an old/new watcher alias.
                bucket.id = existing.id;
                // Bucket already exists — merge events, skipping duplicates
                info!("Bucket '{}' already exists, merging events", bucket.id);
                if let Some(events_vec) = imported {
                    if !events_vec.is_empty() {
                        // Determine time range of events to import
                        let start = events_vec.iter().map(|e| e.timestamp).min().unwrap();
                        let end = events_vec
                            .iter()
                            .map(|e| e.calculate_endtime())
                            .max()
                            .unwrap();

                        // Fetch existing events in that range to detect duplicates.
                        // Events without an explicit ID would otherwise be inserted as new rows
                        // via AUTOINCREMENT, silently creating duplicates on re-import.
                        //
                        // **Memory note**: This loads all events in the import time range into
                        // memory for O(1) dedup lookups. Typical Android re-imports involve a
                        // few thousand events (~1-2 MB), which is well within server bounds.
                        // Pathological cases (years of data) could be mitigated with pagination
                        // or a bloom filter if OOM issues arise in practice.
                        let existing = datastore
                            .get_events_unclipped(&bucket.id, Some(start), Some(end), None)
                            .map_err(|e| {
                                HttpErrorJson::new(
                                    Status::InternalServerError,
                                    format!(
                                        "Failed to fetch existing events for dedup in '{}': {e:?}",
                                        bucket.id
                                    ),
                                )
                            })?;

                        let existing_identities: HashSet<_> = existing
                            .iter()
                            .map(event_identity)
                            .collect::<Result<_, _>>()?;

                        // An exported id is only kept if it names an event of this bucket with the
                        // same start time: re-importing an updated export (e.g. a heartbeat-extended
                        // duration) then replaces that event instead of adding an overlapping copy.
                        // An older/shorter export must not clobber a longer local event.
                        let existing_by_id: HashMap<i64, &Event> = existing
                            .iter()
                            .filter_map(|e| e.id.map(|id| (id, e)))
                            .collect();

                        // Stored events went through the privacy filter, so compare the
                        // *filtered* form of each incoming event (a redacted title never
                        // matches its raw source). The raw event is what gets inserted: the
                        // datastore filters it once on insert, so rules are never applied twice.
                        // Events the filter would drop are skipped here.
                        let filtered =
                            datastore
                                .filter_events(&bucket.id, &events_vec)
                                .map_err(|e| {
                                    HttpErrorJson::new(
                                        Status::InternalServerError,
                                        format!(
                                            "Failed to preview privacy filter for '{}': {e:?}",
                                            bucket.id
                                        ),
                                    )
                                })?;

                        // Filter out events already present (matched by timestamp, duration, data).
                        // Stored rows may predate a privacy rule (raw/unredacted) or postdate it
                        // (redacted), so match each incoming event against both its raw and its
                        // filtered form. Comparing only the filtered form would make a rule enabled
                        // after storage append copies of rows the raw comparison used to skip.
                        let new_events: Vec<_> = events_vec
                            .into_iter()
                            .zip(filtered)
                            .filter_map(|(event, filtered)| filtered.map(|f| (event, f)))
                            .map(|(event, filtered)| {
                                let raw_identity = event_identity(&event)?;
                                let filtered_identity = event_identity(&filtered)?;
                                Ok((raw_identity, filtered_identity, event, filtered))
                            })
                            .collect::<Result<Vec<_>, HttpErrorJson>>()?
                            .into_iter()
                            .filter_map(|(raw_identity, filtered_identity, mut event, filtered)| {
                                if existing_identities.contains(&raw_identity)
                                    || existing_identities.contains(&filtered_identity)
                                {
                                    return None;
                                }
                                if let Some(id) = event.id {
                                    match existing_by_id.get(&id) {
                                        Some(existing_event)
                                            if existing_event.timestamp == event.timestamp =>
                                        {
                                            // Stale backup of the same activity: keep the
                                            // longer local event. A data change at the same
                                            // id+start is a correction and must replace,
                                            // even if duration shrank. Compare both forms:
                                            // a stored row that predates the rule is raw,
                                            // so the filtered incoming data would not match.
                                            if event.duration < existing_event.duration
                                                && (filtered.data == existing_event.data
                                                    || event.data == existing_event.data)
                                            {
                                                return None;
                                            }
                                        }
                                        _ => event.id = None,
                                    }
                                }
                                Some(event)
                            })
                            .collect();

                        if !new_events.is_empty() {
                            if let Err(e) = datastore.insert_events(&bucket.id, &new_events) {
                                let err_msg = format!(
                                    "Failed to merge events into existing bucket '{}': {e:?}",
                                    bucket.id
                                );
                                warn!("{}", err_msg);
                                return Err(HttpErrorJson::new(
                                    Status::InternalServerError,
                                    err_msg,
                                ));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let err_msg = format!("Failed to import bucket: {e:?}");
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::InternalServerError, err_msg));
            }
        }
    }
    Ok(())
}

#[post("/", data = "<json_data>", format = "application/json")]
pub fn bucket_import_json(
    state: &State<ServerState>,
    json_data: ApiJson<BucketsExport>,
) -> Result<(), HttpErrorJson> {
    let result = import(state, json_data.into_inner(), None);
    // Clear even on failure: a multi-bucket import can write earlier buckets
    // and then fail on a later one, leaving those writes cached-over.
    state.bucket_list_changed();
    result
}

#[derive(FromForm)]
pub struct ImportForm {
    // FIXME: In the web-ui the name of this field is buckets.json, but "." is not allowed in field
    // names in Rocket and just simply "buckets" seems to work apparently but not sure why.
    // FIXME: In aw-server python it will import all fields rather just the one named
    // "buckets.json", that should probably be done here as well.
    #[field(name = "buckets")]
    import: Json<BucketsExport>,
}

#[post("/", data = "<form>", format = "multipart/form-data")]
pub fn bucket_import_form(
    state: &State<ServerState>,
    form: Form<ImportForm>,
) -> Result<(), HttpErrorJson> {
    let result = import(state, form.into_inner().import.into_inner(), None);
    // Same partial-import failure mode as the JSON handler above.
    state.bucket_list_changed();
    result
}

/// Import a single device's legacy-shaped export. Empty/local metadata denotes
/// the source device selected by the route; an explicit differing peer is an
/// error, never silently reassigned. Validate the full envelope before writes.
#[post(
    "/<device_id>/import",
    data = "<json_data>",
    format = "application/json"
)]
pub fn device_import_json(
    device_id: &str,
    state: &State<ServerState>,
    json_data: ApiJson<BucketsExport>,
) -> Result<(), HttpErrorJson> {
    let device = state.normalize_device(device_id);
    let data = json_data.into_inner();
    for (id, bucket) in &data.buckets {
        if id != &bucket.id
            || (!bucket.device_id.is_empty()
                && bucket.device_id != "local"
                && state.normalize_device(&bucket.device_id) != device)
        {
            return Err(HttpErrorJson::new(
                Status::BadRequest,
                format!("Bucket '{id}' does not match the selected import identity"),
            ));
        }
    }
    let result = import(state, data, Some(device));
    // Keep the same partial-import invalidation guarantee as legacy imports.
    state.bucket_list_changed();
    result
}
