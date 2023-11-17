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

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use aw_client_rust::blocking::AwClient;
use chrono::{DateTime, Utc};

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, Event};
use clap::ValueEnum;

use crate::accessmethod::AccessMethod;

#[derive(PartialEq, Eq, Copy, Clone, ValueEnum)]
pub enum SyncMode {
    Push,
    Pull,
    Both,
}

#[derive(Debug)]
pub struct SyncSpec {
    /// Path of sync folder
    pub path: PathBuf,
    /// Path of sync db
    /// If None, will use all
    pub path_db: Option<PathBuf>,
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
            path_db: None,
            buckets: None,
            start: None,
        }
    }
}

/// Performs a single sync pass
pub fn sync_run(
    client: &AwClient,
    sync_spec: &SyncSpec,
    mode: SyncMode,
) -> Result<(), Box<dyn Error>> {
    let info = client.get_info()?;

    // FIXME: Here it is assumed that the device_id for the local server is the one used by
    // aw-server-rust, which is not necessarily true (aw-server-python has seperate device_id).
    // Therefore, this may sometimes fail to pick up the correct local datastore.
    let device_id = info.device_id.as_str();

    // FIXME: Bad device_id assumption?
    let ds_localremote = setup_local_remote(sync_spec.path.as_path(), device_id)?;
    let remote_dbfiles = crate::util::find_remotes_nonlocal(
        sync_spec.path.as_path(),
        device_id,
        sync_spec.path_db.as_ref(),
    );

    // Log if remotes found
    // TODO: Only log remotes of interest
    if !remote_dbfiles.is_empty() {
        info!(
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
        info!(
            "Found {} remote datastores: {:?}",
            ds_remotes.len(),
            ds_remotes
        );
    }

    // Pull
    if mode == SyncMode::Pull || mode == SyncMode::Both {
        info!("Pulling...");
        for ds_from in &ds_remotes {
            sync_datastores(ds_from, client, false, None, sync_spec);
        }
    }

    // Push local server buckets to sync folder
    if mode == SyncMode::Push || mode == SyncMode::Both {
        info!("Pushing...");
        sync_datastores(client, &ds_localremote, true, Some(device_id), sync_spec);
    }

    // Close open database connections
    for ds_from in &ds_remotes {
        ds_from.close();
    }
    ds_localremote.close();

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
pub fn list_buckets(client: &AwClient) -> Result<(), Box<dyn Error>> {
    let sync_directory = crate::dirs::get_sync_dir().map_err(|_| "Could not get sync dir")?;
    let sync_directory = sync_directory.as_path();
    let info = client.get_info()?;

    // FIXME: Incorrect device_id assumption?
    let device_id = info.device_id.as_str();
    let ds_localremote = setup_local_remote(sync_directory, device_id)?;

    let remote_dbfiles = crate::util::find_remotes_nonlocal(sync_directory, device_id, None);
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

    Ok(())
}

fn setup_local_remote(path: &Path, device_id: &str) -> Result<Datastore, Box<dyn Error>> {
    // FIXME: Don't run twice if already exists
    fs::create_dir_all(path)?;

    let remotedir = path.join(device_id);
    fs::create_dir_all(&remotedir)?;

    let dbfile = remotedir.join("test.db");

    // Print a message if dbfile doesn't already exist
    if !dbfile.exists() {
        info!("Creating new database file: {}", dbfile.display());
    }

    let ds_localremote = create_datastore(&dbfile);
    Ok(ds_localremote)
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
        format!("{orig_bucketid}-synced-from-{origin}")
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
            match ds_to.get_bucket(new_id.as_str()) {
                Ok(bucket) => bucket,
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(e) => panic!("{e:?}"),
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
                warn!(" ! Bucket hostname/device ID was invalid, setting to device ID/hostname");
                tup.1.hostname = src_did.unwrap().to_string();
            }
            tup.1.clone()
        })
        .collect();

    // Log warning for buckets requested but not found
    if let Some(buckets) = &sync_spec.buckets {
        for b_id in buckets {
            if !buckets_from.iter().any(|b| b.id == *b_id) {
                error!(" ! Bucket \"{}\" not found in source datastore", b_id);
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
    info!(" ⟳  Syncing bucket '{}'", bucket_to.id);

    // Sync events
    // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
    // for empty buckets (Should be None, is Some(unknown_time))
    // let resume_sync_at = bucket_to.metadata.end;
    let most_recent_events = ds_to
        .get_events(bucket_to.id.as_str(), None, None, Some(1))
        .unwrap();
    let resume_sync_at = most_recent_events.first().map(|e| e.timestamp + e.duration);

    if let Some(resume_time) = resume_sync_at {
        info!("   + Resuming at {:?}", resume_time);
    } else {
        info!("   + Starting from beginning");
    }

    // Fetch events
    // Unset ID on events, as they are not globally unique
    // TODO: Fetch at most ~5,000 events at a time (or so, to avoid timeout from huge buckets)
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
    // FIXME: What happens here if two events have the same timestamp?
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    // TODO: Do bulk insert using insert_events instead? (for performance)
    //       Client-side heartbeat queueing should keep things somewhat performant though?
    // NOTE: First event needs to be inserted with heartbeat, to ensure appropriate
    // merging/updating of pulsed events.
    let events_total = events.len();
    let mut events_sent = 0;
    let mut events_iter = events.into_iter();
    if let Some(e) = events_iter.next() {
        ds_to.heartbeat(bucket_to.id.as_str(), e, 0.0).unwrap();
        events_sent += 1;
    }

    const BATCH_SIZE: usize = 5000;
    if BATCH_SIZE == 1 {
        // TODO: Don't print progress messages if not in a suitable terminal environment (such as a
        // pipe or systemd journal)
        for event in events_iter {
            print!("{} ({}/{})\r", &event.timestamp, events_sent, events_total);
            ds_to.heartbeat(bucket_to.id.as_str(), event, 0.0).unwrap();
            events_sent += 1;
        }
    } else {
        let mut batch_events = Vec::with_capacity(BATCH_SIZE);
        for e in events_iter {
            print!("{} ({}/{})\r", e.timestamp, events_sent, events_total);
            batch_events.push(e);
            events_sent += 1;
            if batch_events.len() >= BATCH_SIZE {
                ds_to
                    .insert_events(bucket_to.id.as_str(), batch_events.clone())
                    .unwrap();
                batch_events.clear();
            }
        }

        if !batch_events.is_empty() {
            ds_to
                .insert_events(bucket_to.id.as_str(), batch_events)
                .unwrap();
        }
    }

    let eventcount_to_new = ds_to.get_event_count(bucket_to.id.as_str()).unwrap();
    let new_events_count = eventcount_to_new - eventcount_to_old;
    assert!(new_events_count >= 0);
    if new_events_count > 0 {
        info!("  = Synced {} new events", new_events_count);
    } else {
        info!("  ✓ Already up to date!");
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
