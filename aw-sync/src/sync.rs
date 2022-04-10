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

use std::fs;
use std::path::{Path, PathBuf};

use aw_client_rust::AwClient;
use chrono::{DateTime, Utc};

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, Event};

use crate::accessmethod::AccessMethod;

pub struct SyncSpec {
    /// Path of sync folder
    pub path: PathBuf,
    /// Bucket IDs to sync
    pub buckets: Option<Vec<String>>,
    /// Start of time range to sync
    pub start: Option<DateTime<Utc>>,
}

impl Default for SyncSpec {
    fn default() -> Self {
        // TODO: Better default path
        let path = Path::new("/tmp/aw-sync").to_path_buf();
        SyncSpec {
            path,
            buckets: None,
            start: None,
        }
    }
}

/// Performs a single sync pass
pub fn sync_run(client: AwClient, sync_spec: &SyncSpec) -> Result<(), String> {
    let ds_localremote = setup_local_remote(&client, sync_spec.path.as_path())?;

    let info = client.get_info().unwrap();
    let remote_dbfiles = find_remotes_nonlocal(sync_spec.path.as_path(), info.device_id.as_str());

    // Log if remotes found
    // TODO: Only log remotes of interest
    if !remote_dbfiles.is_empty() {
        println!(
            "Found {} remote db files: {:?}",
            remote_dbfiles.len(),
            remote_dbfiles
        );
    }

    // TODO: Check for compatible remote db version before opening
    let ds_remotes: Vec<Datastore> = remote_dbfiles
        .iter()
        .map(|p| p.as_path())
        .map(create_datastore)
        .collect();

    if !ds_remotes.is_empty() {
        println!(
            "Found {} remote datastores: {:?}",
            ds_remotes.len(),
            ds_remotes
        );
    }

    // Pull
    info!("Pulling...");
    for ds_from in &ds_remotes {
        sync_datastores(ds_from, &client, false, None, sync_spec);
    }

    // Push local server buckets to sync folder
    info!("Pushing...");
    sync_datastores(
        &client,
        &ds_localremote,
        true,
        Some(info.device_id.as_str()),
        sync_spec,
    );

    // Close open database connections
    //for ds_from in &ds_remotes {
    //    ds_from.close();
    //}
    //ds_localremote.close();

    // Dropping also works to close the database connections, weirdly enough.
    // Probably because once the database is dropped, the thread will stop,
    // and then the Connection will be dropped, which closes the connection.
    std::mem::drop(ds_remotes);
    std::mem::drop(ds_localremote);

    // NOTE: Will fail if db connections not closed (as it will open them again)
    //list_buckets(&client, sync_spec.path.as_path());

    Ok(())
}

#[allow(dead_code)]
pub fn list_buckets(client: &AwClient, sync_directory: &Path) {
    let ds_localremote = setup_local_remote(client, sync_directory).unwrap();

    let info = client.get_info().unwrap();
    let remote_dbfiles = find_remotes_nonlocal(sync_directory, info.device_id.as_str());
    info!("Found remotes: {:?}", remote_dbfiles);

    // TODO: Check for compatible remote db version before opening
    let ds_remotes: Vec<Datastore> = remote_dbfiles
        .iter()
        .map(|p| p.as_path())
        .map(create_datastore)
        .collect();

    log_buckets(client);
    log_buckets(&ds_localremote);
    for ds_from in &ds_remotes {
        log_buckets(ds_from);
    }
}

fn setup_local_remote(client: &AwClient, path: &Path) -> Result<Datastore, String> {
    // FIXME: Don't run twice if already exists
    fs::create_dir_all(path).unwrap();

    let info = client.get_info().unwrap();
    let remotedir = path.join(info.device_id.as_str());
    fs::create_dir_all(&remotedir).unwrap();

    let dbfile = remotedir
        .join("test.db")
        .into_os_string()
        .into_string()
        .unwrap();

    // Print a message if dbfile doesn't already exist
    if !Path::new(&dbfile).exists() {
        info!("Creating new database file: {}", dbfile);
    }
    let ds_localremote = Datastore::new(dbfile, false);

    Ok(ds_localremote)
}

/// Returns a list of all remote dbs
fn find_remotes(sync_directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    //info!("Using sync dir: {}", sync_directory.display());
    let dbs = fs::read_dir(sync_directory)?
        .map(|res| res.ok().unwrap().path())
        .filter(|p| p.is_dir())
        .flat_map(|d| {
            //println!("{}", d.to_str().unwrap());
            fs::read_dir(d).unwrap()
        })
        .map(|res| res.ok().unwrap().path())
        .filter(|path| path.extension().unwrap() == "db") // FIXME: Is this the correct file ext?
        .collect();
    Ok(dbs)
}

/// Returns a list of all remotes, excluding local ones
fn find_remotes_nonlocal(sync_directory: &Path, device_id: &str) -> Vec<PathBuf> {
    let remotes_all = find_remotes(sync_directory).unwrap();
    // Filter out own remote
    remotes_all
        .into_iter()
        .filter(|path| {
            !(path
                .clone()
                .into_os_string()
                .into_string()
                .unwrap()
                .contains(device_id))
        })
        .collect()
}

pub fn create_datastore(path: &Path) -> Datastore {
    let pathstr = path.as_os_str().to_str().unwrap();
    Datastore::new(pathstr.to_string(), false)
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
///
/// is_push: a bool indicating if we're pushing local buckets to the sync dir
///          (as opposed to pulling from remotes)
/// src_did: source device ID
pub fn sync_datastores(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    is_push: bool,
    src_did: Option<&str>,
    sync_spec: &SyncSpec,
) {
    // FIXME: "-synced" should only be appended when synced to the local database, not to the
    // staging area for local buckets.
    info!("Syncing {:?} to {:?}", ds_from, ds_to);

    let mut buckets_from: Vec<Bucket> = ds_from
        .get_buckets()
        .unwrap()
        .iter_mut()
        // If buckets vec isn't empty, filter out buckets not in the buckets vec
        .filter(|tup| {
            let bucket = &tup.1;
            if let Some(buckets) = &sync_spec.buckets {
                buckets.iter().any(|b_id| b_id == &bucket.id)
            } else {
                true
            }
        })
        .map(|tup| {
            // TODO: Refuse to sync buckets without hostname/device ID set, or if set to 'unknown'
            if tup.1.hostname == "unknown" {
                warn!("Bucket hostname/device ID was invalid, setting to device ID/hostname");
                tup.1.hostname = src_did.unwrap().to_string();
            }
            tup.1.clone()
        })
        .collect();

    // Log warning for buckets requested but not found
    if let Some(buckets) = &sync_spec.buckets {
        for b_id in buckets {
            if buckets_from.iter().find(|b| b.id == *b_id).is_none() {
                error!("Bucket \"{}\" not found in source datastore", b_id);
            }
        }
    }

    // Sync buckets in order of most recently updated
    buckets_from.sort_by_key(|b| b.metadata.end);

    for bucket_from in buckets_from {
        let bucket_to = get_or_create_sync_bucket(&bucket_from, ds_to, is_push);
        sync_one(ds_from, ds_to, bucket_from, bucket_to);
    }
}

/// Syncs a single bucket from one datastore to another
fn sync_one(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    bucket_from: Bucket,
    bucket_to: Bucket,
) {
    let eventcount_to_old = ds_to.get_event_count(bucket_to.id.as_str()).unwrap();
    info!("Syncing bucket '{}'...", bucket_to.id);

    // Sync events
    // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
    // for empty buckets (Should be None, is Some(unknown_time))
    // let resume_sync_at = bucket_to.metadata.end;
    let most_recent_events = ds_to
        .get_events(bucket_to.id.as_str(), None, None, Some(1))
        .unwrap();
    let resume_sync_at = most_recent_events.first().map(|e| e.timestamp + e.duration);

    if let Some(resume_time) = resume_sync_at {
        info!(" + Resuming at {:?}", resume_time);
    } else {
        info!(" + Starting from beginning");
    }
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

    // TODO: Do bulk insert using insert_events instead? (for performance)
    for event in events {
        //print!("\r{}", event.timestamp);
        ds_to.heartbeat(bucket_to.id.as_str(), event, 0.0).unwrap();
    }

    let eventcount_to_new = ds_to.get_event_count(bucket_to.id.as_str()).unwrap();
    let new_events_count = eventcount_to_new - eventcount_to_old;
    assert!(new_events_count >= 0);
    if new_events_count > 0 {
        info!(" = Synced {} new events", new_events_count);
    } else {
        info!(" = Already up to date!");
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
