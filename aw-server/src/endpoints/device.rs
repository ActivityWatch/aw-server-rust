//! Lossless device catalogue and grouped bucket metadata. The local UUID is
//! public identity; the datastore's `local` sentinel never escapes this envelope.
use std::collections::{BTreeMap, HashMap};

use aw_models::{Bucket, BucketsExport};
use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};

use super::{HttpErrorJson, ServerState};

#[derive(Serialize)]
pub struct Device {
    id: String,
    hostname: String,
}

#[derive(Serialize, Deserialize)]
pub struct DeviceBuckets {
    pub devices: BTreeMap<String, BucketsExport>,
}

pub fn grouped_buckets(state: &ServerState) -> Result<DeviceBuckets, HttpErrorJson> {
    let mut devices = BTreeMap::new();
    // Include the local device even before any watcher has created a bucket.
    devices.insert(
        state.device_id.clone(),
        BucketsExport {
            buckets: HashMap::new(),
        },
    );
    for (device, buckets) in state.datastore.get_device_buckets()? {
        let public_id = if device == "local" {
            &state.device_id
        } else {
            &device
        };
        let group = devices
            .entry(public_id.clone())
            .or_insert_with(|| BucketsExport {
                buckets: HashMap::new(),
            });
        for (id, mut bucket) in buckets {
            bucket.device_id = public_id.clone();
            if group.buckets.insert(id.clone(), bucket).is_some() {
                return Err(HttpErrorJson::new(
                    rocket::http::Status::Conflict,
                    format!(
                        "Multiple stored owners normalize to device '{public_id}', bucket '{id}'"
                    ),
                ));
            }
        }
    }
    Ok(DeviceBuckets { devices })
}

#[get("/")]
pub fn devices_get(state: &State<ServerState>) -> Result<Json<Vec<Device>>, HttpErrorJson> {
    let grouped = grouped_buckets(state)?;
    let local_hostname = gethostname::gethostname().to_string_lossy().into_owned();
    Ok(Json(
        grouped
            .devices
            .into_iter()
            .map(|(id, group)| {
                let hostname = if id == state.device_id {
                    local_hostname.clone()
                } else {
                    // A peer may retain several historical hostnames. No latest-hostname
                    // registry exists yet, so choose deterministically, never invent IDs.
                    group
                        .buckets
                        .values()
                        .map(|b: &Bucket| b.hostname.as_str())
                        .min()
                        .unwrap_or("")
                        .to_owned()
                };
                Device { id, hostname }
            })
            .collect(),
    ))
}

#[get("/buckets")]
pub fn buckets_get(state: &State<ServerState>) -> Result<Json<DeviceBuckets>, HttpErrorJson> {
    Ok(Json(grouped_buckets(state)?))
}

/// Stage the complete snapshot before sending headers. Event rows are streamed
/// to a private tempfile by the existing serialized datastore worker, not held
/// in HTTP memory; failures return an HTTP error rather than truncated JSON.
#[get("/export")]
pub fn export(
    state: &State<ServerState>,
) -> Result<(rocket::http::ContentType, rocket::tokio::fs::File), HttpErrorJson> {
    use std::io::{Seek, SeekFrom};
    let io_error = |e: std::io::Error| {
        HttpErrorJson::new(
            rocket::http::Status::InternalServerError,
            format!("Device export staging failed: {e}"),
        )
    };
    let file = tempfile::tempfile().map_err(io_error)?;
    let mut file = state
        .datastore
        .export_devices_to_file(&state.device_id, file)?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok((
        rocket::http::ContentType::JSON,
        rocket::tokio::fs::File::from_std(file),
    ))
}
