use rocket::form::Form;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;

use std::sync::Mutex;

use aw_models::BucketsExport;

use aw_datastore::{Datastore, DatastoreError};

use crate::endpoints::{HttpErrorJson, ServerState};

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
                        let existing = datastore
                            .get_events(&bucket.id, Some(start), Some(end), None)
                            .map_err(|e| {
                                HttpErrorJson::new(
                                    Status::InternalServerError,
                                    format!(
                                        "Failed to fetch existing events for dedup in '{}': {e:?}",
                                        bucket.id
                                    ),
                                )
                            })?;

                        // Filter out events already present (matched by timestamp, duration, data)
                        let new_events: Vec<_> = events_vec
                            .into_iter()
                            .filter(|e| !existing.contains(e))
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
