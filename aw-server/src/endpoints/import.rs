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
                // Bucket already exists — merge events into it instead of failing
                info!("Bucket '{}' already exists, merging events", bucket.id);
                if let Some(events) = bucket.events.take() {
                    let events_vec = events.take_inner();
                    if !events_vec.is_empty() {
                        if let Err(e) = datastore.insert_events(&bucket.id, &events_vec) {
                            let err_msg = format!(
                                "Failed to merge events into existing bucket '{}': {e:?}",
                                bucket.id
                            );
                            warn!("{}", err_msg);
                            return Err(HttpErrorJson::new(Status::InternalServerError, err_msg));
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
