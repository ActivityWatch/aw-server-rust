use rocket::form::Form;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use aw_models::{BucketsExport, Event};

use aw_datastore::{Datastore, DatastoreError};

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

fn import(datastore_mutex: &Mutex<Datastore>, import: BucketsExport) -> Result<(), HttpErrorJson> {
    let datastore = endpoints_get_lock!(datastore_mutex);
    for (_bucketname, mut bucket) in import.buckets {
        match datastore.create_bucket(&bucket) {
            Ok(_) => (),
            Err(DatastoreError::BucketAlreadyExists(_)) => {
                // Bucket already exists — merge events, skipping duplicates
                info!("Bucket '{}' already exists, merging events", bucket.id);
                if let Some(events) = bucket.events.take() {
                    let events_vec = events.take_inner();
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

                        // Filter out events already present (matched by timestamp, duration, data)
                        let new_events: Vec<_> = events_vec
                            .into_iter()
                            .map(|event| Ok((event_identity(&event)?, event)))
                            .collect::<Result<Vec<_>, HttpErrorJson>>()?
                            .into_iter()
                            .filter_map(|(identity, event)| {
                                (!existing_identities.contains(&identity)).then_some(event)
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
    json_data: Json<BucketsExport>,
) -> Result<(), HttpErrorJson> {
    import(&state.datastore, json_data.into_inner())
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
    import(&state.datastore, form.into_inner().import.into_inner())
}
