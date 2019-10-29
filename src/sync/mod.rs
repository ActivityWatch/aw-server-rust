/// Basic syncing for ActivityWatch
/// Based on: https://github.com/ActivityWatch/aw-server/pull/50
///
/// This does not handle any direct peer interaction/connections/networking, it works as a "bring your own folder synchronizer".
///
/// It manages a sync-folder by syncing the aw-server datastore with a copy/staging datastore in the folder (one for each host).
/// The sync folder is then synced with remotes using Syncthing/Dropbox/whatever.

use std::fs;
use std::path::Path;
use std::collections::HashMap;

use chrono::{DateTime, Utc, Duration};
use serde_json;
use aw_client_rust::{AwClient};

use crate::datastore::{Datastore, DatastoreError};
use crate::models::{Event, Bucket};

trait AccessMethod {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String>;
    fn get_events(&self, bucket_id: &str, limit: Option<u64>) -> Result<Vec<Event>, String>;
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String>;
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String>;
}

struct DirectDatastore {
    ds: Datastore,
}

impl AccessMethod for DirectDatastore {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(self.ds.get_buckets().unwrap())
    }
    fn get_events(&self, bucket_id: &str, limit: Option<u64>) -> Result<Vec<Event>, String> {
        Ok(self.ds.get_events(bucket_id, None, None, limit).unwrap())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String> {
        Ok(self.ds.insert_events(bucket_id, &events[..]).unwrap())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(self.ds.get_event_count(bucket_id, None, None).unwrap())
    }
}

struct APIDatastore {
    awc: AwClient,
}

//impl AccessMethod for APIDatastore {
//    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
//        Ok(self.awc.get_buckets().unwrap())
//    }
//    fn get_events(&self, bucket_id: &str, limit: Option<u64>) -> Result<Vec<Event>, String> {
//        Ok(self.awc.get_events(bucket_id, None, None, limit).unwrap())
//    }
//    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String> {
//        Ok(self.awc.insert_events(bucket_id, &events[..]).unwrap())
//    }
//    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
//        Ok(self.awc.get_event_count(bucket_id, None, None).unwrap())
//    }
//}

/// Performs a single sync pass
pub fn sync_run() {
    // TODO: Get path using dirs module
    let sync_directory = Path::new("/tmp/aw-sync-rust/testing");
    fs::create_dir_all(sync_directory).unwrap();

    // TODO: Use the local datastore here, preferably passed from main
    let ds_local = Datastore::new(sync_directory.join("test-local.db").into_os_string().into_string().unwrap());
    info!("Set up local datastore");
    //log_buckets(&ds_local)?;

    let ds_remotes = setup_test(sync_directory).unwrap();
    info!("Set up remote datastores");

    // FIXME: These are not the datastores that should actually be synced, I'm just testing
    for ds_from in &ds_remotes {
        sync_datastores(&ds_from, &ds_local);
    }

    log_buckets(&ds_local);
    for ds_from in &ds_remotes {
        ds_from.force_commit().unwrap();
        log_buckets(&ds_from);
    }
}

fn setup_test(sync_directory: &Path) -> std::io::Result<Vec<Datastore>> {
    let mut datastores: Vec<Datastore> = Vec::new();
    for n in 0..2 {
        let ds = Datastore::new(sync_directory.join(format!("test-remote-{}.db", n)).to_str().unwrap().to_string());

        // Create a bucket
        let bucket_jsonstr = format!(r#"{{
            "id": "bucket-{}",
            "type": "test",
            "hostname": "device-{}",
            "client": "test"
        }}"#, n, n);
        let bucket: Bucket = serde_json::from_str(&bucket_jsonstr)?;
        match ds.create_bucket(&bucket) {
            Ok(()) => (),
            Err(e) => match e {
                DatastoreError::BucketAlreadyExists => {
                    debug!("bucket already exists, skipping");
                }
                e => panic!("woops! {:?}", e),
            }
        };

        // Insert some testing events into the bucket
        let events: Vec<Event> = (0..3).map(|i| {
            let timestamp: DateTime<Utc> = Utc::now() + Duration::milliseconds(i*10);
            let event_jsonstr = format!(r#"{{
                "timestamp": "{}",
                "duration": 0,
                "data": {{"test": {} }}
            }}"#, timestamp.to_rfc3339(), i);
            let event = serde_json::from_str(&event_jsonstr).unwrap();
            event
        }).collect::<Vec<Event>>();

        ds.insert_events(bucket.id.as_str(), &events[..]).unwrap();
        ds.force_commit().unwrap();
        //let new_eventcount = ds.get_event_count(bucket.id.as_str(), None, None).unwrap();
        //info!("Eventcount: {:?} ({} new)", new_eventcount, events.len());
        datastores.push(ds);
    };
    Ok(datastores)
}

/// Returns the sync-destination bucket for a given bucket, creates it if it doesn't exist.
fn get_or_create_sync_bucket(bucket_from: &Bucket, ds_to: &Datastore) -> Bucket {
    // Ensure the bucket ID ends in "-synced"
    let new_id = format!("{}-synced", bucket_from.id.replace("-synced", ""));

    match ds_to.get_bucket(new_id.as_str()) {
        Ok(bucket) => bucket,
        Err(DatastoreError::NoSuchBucket) => {
            let mut bucket_new = bucket_from.clone();
            bucket_new.id = new_id.clone();
            // TODO: Replace sync origin with hostname/GUID and discuss how we will treat the data
            // attributes for internal use.
            bucket_new.data.insert("$aw.sync.origin".to_string(), serde_json::json!("test"));
            ds_to.create_bucket(&bucket_new).unwrap();
            ds_to.get_bucket(new_id.as_str()).unwrap()
        },
        Err(e) => panic!(e),
    }
}

/// Syncs all buckets from `ds_from` to `ds_to` with `-synced` appended to the ID of the destination bucket.
pub fn sync_datastores(ds_from: &Datastore, ds_to: &Datastore) {
    // FIXME: "-synced" should only be appended when synced to the local database, not to the
    // staging area for local buckets.
    info!("Syncing {:?} to {:?}", ds_from, ds_to);

    let buckets_from = ds_from.get_buckets().unwrap();
    for bucket_from in buckets_from.values() {
        let bucket_to = get_or_create_sync_bucket(bucket_from, ds_to);
        ds_to.force_commit().unwrap();
        let eventcount_to_old = ds_to.get_event_count(bucket_to.id.as_str(), None, None).unwrap();
        //info!("{:?}", bucket_to);

        // Sync events
        // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
        // for empty buckets (Should be None, is Some(unknown_time))
        // let resume_sync_at = bucket_to.metadata.end;
        let most_recent_events = ds_to.get_events(bucket_to.id.as_str(), None, None, Some(1)).unwrap();
        let resume_sync_at = match most_recent_events.first() {
            Some(e) => Some(e.timestamp + e.duration),
            None => None,
        };

        info!("Resumed at: {:?}", resume_sync_at);
        let mut events: Vec<Event> = ds_from.get_events(bucket_from.id.as_str(), resume_sync_at, None, None).unwrap().iter().map(|e| {
            let mut new_e = e.clone();
            new_e.id = None;
            //info!("{:?}", new_e);
            new_e
        }).collect();

        // Sort ascending
        events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        //info!("{:?}", events);
        for event in events {
            ds_to.heartbeat(bucket_to.id.as_str(), event, 0.0).unwrap();
        }

        ds_to.force_commit().unwrap();
        let eventcount_to_new = ds_to.get_event_count(bucket_to.id.as_str(), None, None).unwrap();
        info!("Synced {} new events", eventcount_to_new - eventcount_to_old);
    }
}

fn log_buckets(ds: &Datastore) {
    // Logs all buckets and some metadata for a given datastore
    let buckets = ds.get_buckets().unwrap();
    info!("Buckets in {:?}:", ds);
    for bucket in buckets.values() {
        info!(" - {}", bucket.id.as_str());
        info!("   eventcount: {:?}", ds.get_event_count(bucket.id.as_str(), None, None).unwrap());
    };
}
