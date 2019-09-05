// Basic syncing for ActivityWatch
// Based on: https://github.com/ActivityWatch/aw-server/pull/50
//
// This does not handle any direct peer interaction/connections/networking, it works as a "bring your own folder synchronizer".
//
// It manages a sync-folder by syncing the aw-server datastore with a copy/staging datastore in the folder (one for each host).
// The sync folder is then synced with remotes using Syncthing/Dropbox/whatever.

#[macro_use] extern crate log;

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json;

use aw_server::*;
use aw_server::datastore::{Datastore, DatastoreError};
use aw_server::models::{Event, Bucket};


fn main() -> std::io::Result<()> {
    // What needs to be done:
    //  - [x] Setup local sync bucket
    //  - Import local buckets and sync events from aw-server (either through API or through creating a read-only Datastore)
    //  - Import buckets and sync events from remotes

    println!("Started aw-sync-rust...");
    logging::setup_logger().expect("Failed to setup logging");

    // TODO: Get path using dirs module
    let sync_directory = Path::new("/tmp/aw-sync-rust/testing");
    fs::create_dir_all(sync_directory)?;
    info!("Created syncing directory");

    let ds_local = setup(sync_directory)?;
    info!("Set up local datastore");
    log_buckets(&ds_local)?;

    let ds_remotes = setup_test(sync_directory)?;
    info!("Set up remote datastores");
    log_buckets(ds_remotes.first().unwrap())?;

    // FIXME: These are not the datastores that should actually be synced, I'm just testing
    for ds_from in &ds_remotes {
        sync_datastores(&ds_from, &ds_local);
    }

    std::thread::sleep(std::time::Duration::from_millis(1000));

    log_buckets(&ds_local)?;

    test(&ds_local, &ds_remotes);

    info!("Finished successfully, exiting...");

    std::thread::sleep(std::time::Duration::from_millis(1000));

    Ok(())
}

fn test(ds_local: &Datastore, ds_remotes: &Vec<Datastore>) {
    // Post-sync test
    let n_remote_buckets: usize = ds_remotes.iter().map(|x| {
        x.get_buckets().unwrap().len()
    }).sum();
    assert!(ds_local.get_buckets().unwrap().len() == n_remote_buckets);

    // TODO: Check that number of events are equal across source and destination buckets
}

fn setup(sync_directory: &Path) -> std::io::Result<Datastore> {
    // Setup the local sync db

    // TODO: better filename
    let ds = Datastore::new(sync_directory.join("test-local.db").to_str().unwrap().to_string());
    Ok(ds)
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
            let timestamp: DateTime<Utc> = Utc::now();
            let event_jsonstr = format!(r#"{{
                "timestamp": "{}",
                "duration": 0,
                "data": {{"test": {} }}
            }}"#, timestamp.to_rfc3339(), i);
            let event = serde_json::from_str(&event_jsonstr).unwrap();
            event
        }).collect::<Vec<Event>>();
        ds.insert_events(bucket.id.as_str(), &events[..]).unwrap();
        info!("Eventcount: {:?}", ds.get_event_count(bucket.id.as_str(), None, None).unwrap());
        datastores.push(ds);
    };
    Ok(datastores)
}

fn sync_datastores(ds_from: &Datastore, ds_to: &Datastore) -> () {
    info!("Syncing {:?} to {:?}", ds_from, ds_to);

    let buckets_from = ds_from.get_buckets().unwrap();
    for bucket in buckets_from.values() {
        // Check if bucket exists in destination, if not then create
        let buckets_to = ds_to.get_buckets().unwrap();
        let new_id = format!("{}-synced", bucket.id);
        if !buckets_to.contains_key(new_id.as_str()) {
            let mut bucket_new = bucket.clone();
            bucket_new.id = new_id.clone();
            ds_to.create_bucket(&bucket_new).unwrap();
        }

        // Sync events
        // FIXME: Events are not being saved, does the datastore worker need more time before exit?
        let events: Vec<Event> = ds_from.get_events(bucket.id.as_str(), None, None, None).unwrap().iter().map(|e| {
            let mut new_e = e.clone();
            new_e.id = None;
            //info!("{:?}", new_e);
            new_e
        }).collect();
        info!("Syncing events: {:?}", events.len());
        ds_to.insert_events(new_id.as_str(), &events[..]).unwrap();
    }
    ()
}

fn log_buckets(ds: &Datastore) -> std::io::Result<()> {
    // Logs all buckets and some associated data for a given datastore
    let buckets = ds.get_buckets().unwrap();
    info!("Buckets in {:?}:", ds);
    for bucket in buckets.values() {
        info!(" - {}", bucket.id.as_str());
        info!("   eventcount: {:?}", ds.get_event_count(bucket.id.as_str(), None, None).unwrap());
    }

    Ok(())
}
