use rocket::http::ContentType;
use rocket::http::Status;
use rocket::Data;
use rocket::State;
use rocket_contrib::json::Json;

use multipart::server::Multipart;

use std::io::Read;
use std::sync::Mutex;

use aw_models::BucketsExport;

use aw_datastore::Datastore;

use crate::endpoints::ServerState;

fn import(datastore_mutex: &Mutex<Datastore>, import: BucketsExport) -> Result<(), Status> {
    let datastore = endpoints_get_lock!(datastore_mutex);
    for (_bucketname, bucket) in import.buckets {
        match datastore.create_bucket(&bucket) {
            Ok(_) => (),
            Err(e) => {
                warn!("Failed to import bucket: {:?}", e);
                return Err(Status::InternalServerError);
            }
        }
    }
    Ok(())
}

#[post("/", data = "<json_data>", format = "application/json")]
pub fn bucket_import_json(
    state: State<ServerState>,
    json_data: Json<BucketsExport>,
) -> Result<(), Status> {
    import(&state.datastore, json_data.into_inner())
}

// FIXME: This eats a lot of RAM (double the amount of the size of the file imported)
// In Rocket 0.5 this will likely be improved when native multipart support is added
#[post("/", data = "<data>", format = "multipart/form-data")]
pub fn bucket_import_form(
    state: State<ServerState>,
    cont_type: &ContentType,
    data: Data,
) -> Result<(), Status> {
    let (_, boundary) = cont_type
        .params()
        .find(|&(k, _)| k == "boundary")
        .ok_or_else(|| {
            warn!("`Content-Type: multipart/form-data` boundary param not provided");
            return Status::BadRequest;
        })?;

    let string = process_multipart_packets(boundary, data);

    let import_data: BucketsExport = serde_json::from_str(&string)
        .expect("Failed to deserialize import data as JSON to bucket format");

    import(&state.datastore, import_data)
}

// NOTE: this is far from a optimal way of parsing multipart packets as it doesn't check
// headers and can be used for denial-of-service attacks as we don't have a size limit and
// store everything in RAM
fn process_multipart_packets(boundary: &str, data: Data) -> String {
    let mut content = String::new();
    Multipart::with_body(data.open(), boundary)
        .foreach_entry(|mut entry| {
            let mut string = String::new();
            entry
                .data
                .read_to_string(&mut string)
                .expect("Failed to parse multipart data to utf-8");
            content.push_str(&string);
        })
        .expect("Failed to retrieve multipart upload");

    content
}
