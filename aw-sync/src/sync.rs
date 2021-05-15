/// Basic syncing for ActivityWatch
/// Based on: https://github.com/ActivityWatch/aw-server/pull/50
///
/// This does not handle any direct peer interaction/connections/networking, it works as a "bring your own folder synchronizer".
///
/// It manages a sync-folder by syncing the aw-server datastore with a copy/staging datastore in the folder (one for each host).
/// The sync folder is then synced with remotes using Syncthing/Dropbox/whatever.
extern crate chrono;
extern crate reqwest;
extern crate serde_json;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aw_client_rust::AwClient;
use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, Event};

// This trait should be implemented by both AwClient and Datastore, unifying them under a single API
pub trait AccessMethod: std::fmt::Debug {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String>;
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError>;
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError>;
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String>;
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String>;
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String>;
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String>;
}

impl AccessMethod for Datastore {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(self.get_buckets().unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        self.get_bucket(bucket_id)
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        self.create_bucket(bucket)?;
        self.force_commit().unwrap();
        Ok(())
    }
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        Ok(self.get_events(bucket_id, start, end, limit).unwrap())
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        self.heartbeat(bucket_id, event, duration).unwrap();
        self.force_commit().unwrap();
        Ok(())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String> {
        let res = self.insert_events(bucket_id, &events[..]).unwrap();
        self.force_commit().unwrap();
        Ok(res)
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(self.get_event_count(bucket_id, None, None).unwrap())
    }
}

impl AccessMethod for AwClient {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(self.get_buckets().unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let bucket = self.get_bucket(bucket_id);
        match bucket {
            Ok(bucket) => Ok(bucket),
            Err(e) => {
                warn!("{:?}", e);
                let code = e.status().unwrap();
                if code == StatusCode::NOT_FOUND {
                    Err(DatastoreError::NoSuchBucket(bucket_id.into()))
                } else {
                    panic!("Unexpected error");
                }
            }
        }
    }
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        Ok(self.get_events(bucket_id, start, end, limit).unwrap())
    }
    fn insert_events(&self, _bucket_id: &str, _events: Vec<Event>) -> Result<Vec<Event>, String> {
        //Ok(self.insert_events(bucket_id, &events[..]).unwrap())
        Err("Not implemented".to_string())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(self.get_event_count(bucket_id).unwrap())
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        self.create_bucket(bucket.id.as_str(), bucket._type.as_str())
            .unwrap();
        Ok(())
        //Err(DatastoreError::InternalError("Not implemented".to_string()))
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        self.heartbeat(bucket_id, &event, duration)
            .map_err(|e| format!("{:?}", e))
    }
}

/// Performs a single sync pass
pub fn sync_run(sync_directory: &Path, client: AwClient) {
    fs::create_dir_all(sync_directory).unwrap();

    let info = client.get_info().unwrap();
    let ds_localremote = Datastore::new(
        sync_directory
            .join(format!("test-{}.db", info.device_id))
            .into_os_string()
            .into_string()
            .unwrap(),
        false,
    );
    info!("Set up remote for local device");

    //let ds_remotes = setup_test(sync_directory).unwrap();
    //info!("Set up remotes for testing");

    let remote_dbfiles = find_remotes_nonlocal(sync_directory, info.device_id.as_str());
    info!("Found remotes: {:?}", remote_dbfiles);
    let ds_remotes: Vec<Datastore> = remote_dbfiles.iter().map(create_datastore).collect();

    // Pull
    info!("Pulling...");
    for ds_from in &ds_remotes {
        sync_datastores(ds_from, &client, false, None);
    }

    // Push local server buckets to sync folder
    info!("Pushing...");
    sync_datastores(
        &client,
        &ds_localremote,
        true,
        Some(info.device_id.as_str()),
    );

    log_buckets(&client);
    log_buckets(&ds_localremote);
    for ds_from in &ds_remotes {
        log_buckets(ds_from);
    }
}

fn find_remotes(sync_directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let files = fs::read_dir(sync_directory)?
        .map(|res| res.ok().unwrap())
        .map(|e| e.path())
        .filter(|path| path.extension().unwrap() == "db") // FIXME: Is this the correct file ext?
        .collect();
    Ok(files)
}

fn find_remotes_nonlocal(sync_directory: &Path, device_id: &str) -> Vec<PathBuf> {
    let remotes_all = find_remotes(sync_directory).unwrap();
    // Filter out own remote
    remotes_all
        .into_iter()
        .filter(|path| {
            !path
                .clone()
                .into_os_string()
                .into_string()
                .unwrap()
                .contains(device_id)
        })
        .collect()
}

fn create_datastore(dspath: &PathBuf) -> Datastore {
    let pathstr = dspath.clone().into_os_string().into_string().unwrap();
    Datastore::new(pathstr, false)
}

fn setup_test(sync_directory: &Path) -> std::io::Result<Vec<Datastore>> {
    let mut datastores: Vec<Datastore> = Vec::new();
    for n in 0..2 {
        let dspath = sync_directory.join(format!("test-remote-{}.db", n));
        let ds_ = create_datastore(&dspath);
        let ds = &ds_ as &dyn AccessMethod;

        // Create a bucket
        // NOTE: Created with duplicate name to make sure it still works under such conditions
        let bucket_jsonstr = format!(
            r#"{{
                "id": "bucket",
                "type": "test",
                "hostname": "device-{}",
                "client": "test"
            }}"#,
            n
        );
        let bucket: Bucket = serde_json::from_str(&bucket_jsonstr)?;
        match ds.create_bucket(&bucket) {
            Ok(()) => (),
            Err(e) => match e {
                DatastoreError::BucketAlreadyExists(_) => {
                    debug!("bucket already exists, skipping");
                }
                e => panic!("woops! {:?}", e),
            },
        };

        // Insert some testing events into the bucket
        let events: Vec<Event> = (0..3)
            .map(|i| {
                let timestamp: DateTime<Utc> = Utc::now() + Duration::milliseconds(i * 10);
                let event_jsonstr = format!(
                    r#"{{
                "timestamp": "{}",
                "duration": 0,
                "data": {{"test": {} }}
            }}"#,
                    timestamp.to_rfc3339(),
                    i
                );
                serde_json::from_str(&event_jsonstr).unwrap()
            })
            .collect::<Vec<Event>>();

        ds.insert_events(bucket.id.as_str(), events).unwrap();
        //let new_eventcount = ds.get_event_count(bucket.id.as_str(), None, None).unwrap();
        //info!("Eventcount: {:?} ({} new)", new_eventcount, events.len());
        datastores.push(ds_);
    }
    Ok(datastores)
}

/// Returns the sync-destination bucket for a given bucket, creates it if it doesn't exist.
fn get_or_create_sync_bucket(
    bucket_from: &Bucket,
    ds_to: &dyn AccessMethod,
    is_push: bool,
) -> Bucket {
    let new_id = if is_push {
        bucket_from.id.clone()
    } else {
        // Ensure the bucket ID ends in "-synced-from-{device id}"
        let orig_bucketid = bucket_from.id.split("-synced-from-").next().unwrap();
        let fallback = serde_json::to_value(&bucket_from.hostname).unwrap();
        let origin = bucket_from
            .data
            .get("$aw.sync.origin")
            .unwrap_or(&fallback)
            .as_str()
            .unwrap();
        format!("{}-synced-from-{}", orig_bucketid, origin)
    };

    match ds_to.get_bucket(new_id.as_str()) {
        Ok(bucket) => bucket,
        Err(DatastoreError::NoSuchBucket(_)) => {
            let mut bucket_new = bucket_from.clone();
            bucket_new.id = new_id.clone();
            // TODO: Replace sync origin with hostname/GUID and discuss how we will treat the data
            // attributes for internal use.
            bucket_new.data.insert(
                "$aw.sync.origin".to_string(),
                serde_json::json!(bucket_from.hostname),
            );
            ds_to.create_bucket(&bucket_new).unwrap();
            ds_to.get_bucket(new_id.as_str()).unwrap()
        }
        Err(e) => panic!("{:?}", e),
    }
}

/// Syncs all buckets from `ds_from` to `ds_to` with `-synced` appended to the ID of the destination bucket.
pub fn sync_datastores(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    is_push: bool,
    src_did: Option<&str>,
) {
    // FIXME: "-synced" should only be appended when synced to the local database, not to the
    // staging area for local buckets.
    info!("Syncing {:?} to {:?}", ds_from, ds_to);

    let buckets_from: Vec<Bucket> = ds_from
        .get_buckets()
        .unwrap()
        .iter_mut()
        .map(|tup| {
            // TODO: Refuse to sync buckets without hostname/device ID set, or if set to 'unknown'
            if tup.1.hostname == "unknown" {
                warn!("Bucket hostname/device ID was invalid, setting to device ID/hostname");
                tup.1.hostname = src_did.unwrap().to_string();
            }
            tup.1.clone()
        })
        .collect();

    for bucket_from in buckets_from {
        let bucket_to = get_or_create_sync_bucket(&bucket_from, ds_to, is_push);
        let eventcount_to_old = ds_to.get_event_count(bucket_to.id.as_str()).unwrap();
        info!("Bucket: {:?}", bucket_to.id);

        // Sync events
        // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
        // for empty buckets (Should be None, is Some(unknown_time))
        // let resume_sync_at = bucket_to.metadata.end;
        let most_recent_events = ds_to
            .get_events(bucket_to.id.as_str(), None, None, Some(1))
            .unwrap();
        let resume_sync_at = match most_recent_events.first() {
            Some(e) => Some(e.timestamp + e.duration),
            None => None,
        };

        info!("Resumed at: {:?}", resume_sync_at);
        let mut events: Vec<Event> = ds_from
            .get_events(bucket_from.id.as_str(), resume_sync_at, None, None)
            .unwrap()
            .iter()
            .map(|e| {
                let mut new_e = e.clone();
                new_e.id = None;
                new_e
            })
            .collect();

        // Sort ascending
        events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        //info!("{:?}", events);
        for event in events {
            ds_to.heartbeat(bucket_to.id.as_str(), event, 0.0).unwrap();
        }

        let eventcount_to_new = ds_to.get_event_count(bucket_to.id.as_str()).unwrap();
        info!(
            "Synced {} new events",
            eventcount_to_new - eventcount_to_old
        );
    }
}

fn log_buckets(ds: &dyn AccessMethod) {
    // Logs all buckets and some metadata for a given datastore
    let buckets = ds.get_buckets().unwrap();
    info!("Buckets in {:?}:", ds);
    for bucket in buckets.values() {
        info!(" - {}", bucket.id.as_str());
        info!(
            "   eventcount: {:?}",
            ds.get_event_count(bucket.id.as_str()).unwrap()
        );
    }
}
