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
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use aw_client_rust::blocking::AwClient;
use chrono::{DateTime, Duration, Utc};

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, Event};

#[cfg(feature = "cli")]
use clap::ValueEnum;

use crate::accessmethod::AccessMethod;

#[derive(PartialEq, Eq, Copy, Clone)]
#[cfg_attr(feature = "cli", derive(ValueEnum))]
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
    // Only stage a local db when this pass actually pushes. Pull-only
    // `sync_run` is how `sync_wrapper::pull` walks a *peer's* host folder;
    // creating `{peer_host}/{our_device_id}/test.db` there breaks the
    // "each device only writes files it owns" invariant (see
    // ActivityWatch/aw-server-rust#682).
    let ds_localremote = maybe_setup_local_remote(sync_spec.path.as_path(), device_id, mode)?;
    let remote_dbfiles = crate::util::find_remotes_nonlocal(
        sync_spec.path.as_path(),
        device_id,
        sync_spec.path_db.as_ref(),
    )?;

    // Log if remotes found
    // TODO: Only log remotes of interest
    if !remote_dbfiles.is_empty() {
        info!(
            "Found {} remote db files: {:?}",
            remote_dbfiles.len(),
            remote_dbfiles
        );
    }

    // Finding zero peers in a configured sync dir is the interesting case —
    // ActivityWatch/aw-server-rust#684. Do not stay silent.
    if mode == SyncMode::Pull || mode == SyncMode::Both {
        for line in crate::util::pull_discovery_warnings(
            sync_spec.path.as_path(),
            device_id,
            &remote_dbfiles,
        ) {
            warn!("{line}");
        }
    }

    // TODO: Check for compatible remote db version before opening
    let mut ds_remotes = Vec::new();
    for path in &remote_dbfiles {
        match create_datastore(path) {
            Ok(ds) => ds_remotes.push(ds),
            Err(e) => {
                warn!("Failed to open remote db {}: {e}", path.display());
                return Err(e.into());
            }
        }
    }

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
            sync_datastores(ds_from, client, false, None, sync_spec)?;
        }
    }

    // Push local server buckets to sync folder
    if let Some(ds_localremote) = &ds_localremote {
        info!("Pushing...");
        sync_datastores(client, ds_localremote, true, Some(device_id), sync_spec)?;
    }

    // Close open database connections
    for ds_from in &ds_remotes {
        ds_from.close();
    }
    if let Some(ds_localremote) = &ds_localremote {
        ds_localremote.close();
    }

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

    let remote_dbfiles = crate::util::find_remotes_nonlocal(sync_directory, device_id, None)?;
    info!("Found remotes: {:?}", remote_dbfiles);

    // TODO: Check for compatible remote db version before opening
    let ds_remotes: Vec<Datastore> = remote_dbfiles
        .iter()
        .map(|p| p.as_path())
        .map(create_datastore)
        .collect::<Result<Vec<_>, _>>()?;

    log_buckets(client)?;
    log_buckets(&ds_localremote)?;
    for ds_from in &ds_remotes {
        log_buckets(ds_from)?;
    }

    Ok(())
}

fn maybe_setup_local_remote(
    path: &Path,
    device_id: &str,
    mode: SyncMode,
) -> Result<Option<Datastore>, Box<dyn Error>> {
    if mode == SyncMode::Push || mode == SyncMode::Both {
        Ok(Some(setup_local_remote(path, device_id)?))
    } else {
        // `get_sync_dir()` is path construction only. Push/Both used to create
        // the root as a side effect of staging; pull-only still needs the root
        // so `find_remotes` does not NotFound (advanced CLI / pull-only daemon
        // on a fresh machine). Do not create `{path}/{device_id}/`.
        fs::create_dir_all(path)?;
        Ok(None)
    }
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

    let ds_localremote = create_datastore(&dbfile)?;
    Ok(ds_localremote)
}

/// Open (or create) the sqlite datastore at `path`.
///
/// `Datastore::new` takes a `String`, so a non-UTF-8 path cannot be passed
/// through faithfully. Report that as an error rather than unwrapping (a panic
/// here aborts the app on Android, aw-android#220) and rather than lossily
/// converting it, which would silently open a *different* file than the caller
/// asked for.
pub fn create_datastore(path: &Path) -> Result<Datastore, String> {
    let pathstr = path
        .to_str()
        .ok_or_else(|| format!("Sync database path is not valid UTF-8: {}", path.display()))?;
    Ok(Datastore::new(pathstr.to_string(), false))
}

/// Returns the sync-destination bucket for a given bucket, creates it if it doesn't exist.
///
/// Returns an error rather than panicking on a datastore failure or on bucket
/// metadata of an unexpected shape: this runs inside a JNI call on Android,
/// where an unwind out of the `extern "C"` frame aborts the app
/// (ActivityWatch/aw-android#220).
fn get_or_create_sync_bucket(
    bucket_from: &Bucket,
    ds_to: &dyn AccessMethod,
    is_push: bool,
) -> Result<Bucket, String> {
    // On pull/import: derive the origin from $aw.sync.origin metadata (preferred) or the
    // hostname (legacy fallback for buckets that predate the metadata field).  On push-staging
    // the bucket keeps its original ID and we do NOT stamp $aw.sync.origin — staging copies
    // should not look like synced-from-remote buckets.
    let (new_id, sync_origin) = if is_push {
        (bucket_from.id.clone(), None)
    } else {
        // `split` always yields at least one item, so this cannot be None.
        let orig_bucketid = bucket_from
            .id
            .split("-synced-from-")
            .next()
            .unwrap_or(bucket_from.id.as_str());
        let origin = match bucket_from.data.get("$aw.sync.origin") {
            Some(value) => value.as_str().map(str::to_string).ok_or_else(|| {
                format!(
                    "Bucket '{}' has a non-string $aw.sync.origin: {}",
                    bucket_from.id, value
                )
            })?,
            None => bucket_from.hostname.clone(),
        };
        (
            format!("{orig_bucketid}-synced-from-{origin}"),
            Some(origin),
        )
    };

    match ds_to.get_bucket(new_id.as_str()) {
        Ok(bucket) => Ok(bucket),
        Err(DatastoreError::NoSuchBucket(_)) => {
            let mut bucket_new = bucket_from.clone();
            bucket_new.id = new_id.clone();
            // Only stamp $aw.sync.origin on pull/import.  The derived origin already handles
            // the legacy case: hostname is used when the source bucket has no metadata field.
            if let Some(origin) = sync_origin {
                bucket_new
                    .data
                    .insert("$aw.sync.origin".to_string(), serde_json::json!(origin));
            } else {
                // Push path: strip any stale $aw.sync.origin that bucket_from may carry
                // (e.g. if it was previously imported by a pull).  Staging copies must
                // never look like synced-from-remote buckets.
                bucket_new.data.remove("$aw.sync.origin");
            }
            ds_to
                .create_bucket(&bucket_new)
                .map_err(|e| format!("Failed to create bucket '{new_id}': {e:?}"))?;
            ds_to
                .get_bucket(new_id.as_str())
                .map_err(|e| format!("Failed to read back bucket '{new_id}': {e:?}"))
        }
        Err(e) => Err(format!("Failed to get bucket '{new_id}': {e:?}")),
    }
}

/// Number of events fetched per page in the chunked-fetch loop in `sync_one`.
/// Reduced in tests so multi-page paths can be exercised with a small event count.
#[cfg(not(test))]
const BATCH_SIZE: usize = 5000;
#[cfg(test)]
const BATCH_SIZE: usize = 5;

/// How far before the resume cursor to look for owner-originated edits of
/// already-synced events (ActivityWatch/aw-android#253). Bounded so a full
/// historical bucket is never loaded into memory on Android.
const EDIT_RECONCILE_LOOKBACK: Duration = Duration::days(7);

/// Whether a bucket holds data synced from another host, rather than data
/// collected on this host.
///
/// The `-synced-from-<origin>` ID suffix is the marker, matching how
/// `get_or_create_sync_bucket` builds and parses these IDs. Note that
/// `$aw.sync.origin` cannot be used here: it is written on push-staging as well
/// as on import (see the FIXME on `sync_datastores`), so it is set on a host's
/// own exported buckets too and would make every bucket look second-hand.
///
/// `-synced-from-` is a **reserved token** in aw-sync's ID grammar, not merely a
/// convention: `get_or_create_sync_bucket` splits on it to recover the original
/// ID, so a first-hand bucket whose own ID contained it would already have that
/// ID truncated on import, independently of this check. Treating it as a marker
/// therefore adds no new failure mode. Issue #649 tracks moving provenance to
/// bucket metadata, which removes the dependency on the ID string entirely.
fn is_synced_bucket(bucket: &Bucket) -> bool {
    bucket.id.contains("-synced-from-")
}

/// Syncs all buckets from `ds_from` to `ds_to` with `-synced` appended to the ID of the destination bucket.
///
/// Buckets that were themselves synced from another host are skipped in both
/// directions, so data is only ever exchanged first-hand.
///
/// is_push: a bool indicating if we're pushing local buckets to the sync dir
///          (as opposed to pulling from remotes)
/// src_did: source device ID
///
/// Returns an error instead of panicking: this is the step the abort in
/// ActivityWatch/aw-android#220 happens in (the sync directory and its
/// datastore are already created by the time the process dies), and on Android
/// it runs inside a JNI `extern "C"` frame where an unwind is an abort rather
/// than a caught exception.
pub fn sync_datastores(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    is_push: bool,
    src_did: Option<&str>,
    sync_spec: &SyncSpec,
) -> Result<(), String> {
    // FIXME: "-synced" should only be appended when synced to the local database, not to the
    // staging area for local buckets.
    info!("Syncing {:?} to {:?}", ds_from, ds_to);

    let mut buckets_from: Vec<Bucket> = ds_from
        .get_buckets()
        .map_err(|e| format!("Failed to list buckets in {ds_from:?}: {e}"))?
        .iter_mut()
        // Never sync a bucket that is itself a copy synced from another host.
        // A host must only ever offer data it collected itself. Without this,
        // HOSTA's buckets reach HOSTB, are re-exported by HOSTB's next push, and
        // come back to HOSTA as `<bucket>_HOSTA-synced-from-HOSTA` — a duplicate
        // of the local bucket, so /timeline renders every event twice.
        // See https://github.com/orgs/ActivityWatch/discussions/1373
        .filter(|tup| {
            if is_synced_bucket(tup.1) {
                debug!(" - Skipping already-synced bucket '{}'", tup.1.id);
                false
            } else {
                true
            }
        })
        // Only filter buckets if specific bucket IDs are provided
        .filter(|tup| {
            let bucket = &tup.1;
            if let Some(buckets) = &sync_spec.buckets {
                // If "*" is in the buckets list or no buckets specified, sync all buckets
                if buckets.iter().any(|b_id| b_id == "*") || buckets.is_empty() {
                    true
                } else {
                    buckets.iter().any(|b_id| b_id == &bucket.id)
                }
            } else {
                // By default, sync all buckets
                true
            }
        })
        .map(|tup| {
            // TODO: Refuse to sync buckets without hostname/device ID set, or if set to 'unknown'
            if tup.1.hostname == "unknown" {
                // Only the push path carries a source device ID to substitute.
                // On pull there is none, and continuing would give the bucket a
                // `-synced-from-unknown` destination ID shared by every remote
                // with that bucket ID, mixing events from unrelated devices.
                // Refuse the sync instead (the previous code unwrapped the None
                // here, which on Android aborts the whole app).
                let did = src_did.ok_or_else(|| {
                    format!(
                        "Bucket '{}' has an unknown hostname/device ID and there is no source \
                         device ID to substitute; refusing to sync it without provenance",
                        tup.1.id
                    )
                })?;
                warn!(" ! Bucket hostname/device ID was invalid, setting to device ID/hostname");
                tup.1.hostname = did.to_string();
            }
            Ok(tup.1.clone())
        })
        .collect::<Result<Vec<Bucket>, String>>()?;

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
        let bucket_to = get_or_create_sync_bucket(&bucket_from, ds_to, is_push)?;
        sync_one(ds_from, ds_to, bucket_from, bucket_to, sync_spec)?;
    }

    Ok(())
}

fn event_identity(event: &Event) -> (DateTime<Utc>, i64) {
    (
        event.timestamp,
        event.duration.num_nanoseconds().unwrap_or(0),
    )
}

/// Replace dest events whose timestamp+duration still exist on the source but
/// whose data changed.
///
/// WebUI/Android title edits are delete+insert at the same timestamp and
/// duration. The resume cursor starts at the destination's latest event end,
/// so those replacements are outside the incremental fetch window. Under the
/// single-writer model the source is authoritative for its own buckets, so a
/// same-identity row with different data is an owner-originated edit, not a
/// conflict. Identity includes duration so two events that share a timestamp
/// are not collapsed into one HashMap slot.
///
/// Duration-only updates of the live last event stay on the incremental
/// heartbeat path. Must run *before* that copy: a latest-event title edit is
/// also re-fetched as a start-clipped fragment, and heartbeat() refuses to
/// merge different data, which would insert a duplicate unless dest already
/// holds the new payload.
fn reconcile_updated_events(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    bucket_from: &Bucket,
    bucket_to: &Bucket,
    resume_sync_at: Option<DateTime<Utc>>,
) -> Result<(), String> {
    let Some(resume) = resume_sync_at else {
        return Ok(());
    };
    let lookback_start = resume - EDIT_RECONCILE_LOOKBACK;

    // Bound both fetches to the lookback window ending at `resume`.
    // end=None would load every newer source event when dest is far behind,
    // exhausting Android RAM and bypassing the paginated incremental copy.
    // get_events clips to the query range; dest-latest ends at `resume`, so
    // that clip is a no-op on its (timestamp, duration) identity. Title edits
    // keep duration; duration-only updates stay on the heartbeat path. Do not
    // also cap by count — a newest-first cap silently skips older in-window
    // edits.
    //
    // Datastore errors return rather than unwrap: a panic here aborts the
    // whole pass (and on Android, the JNI frame). The per-bucket skip in
    // ActivityWatch/aw-server-rust#697 then drops this bucket, not the daemon.
    let source_events = ds_from.get_events(
        bucket_from.id.as_str(),
        Some(lookback_start),
        Some(resume),
        None,
    )?;
    let dest_events = ds_to.get_events(
        bucket_to.id.as_str(),
        Some(lookback_start),
        Some(resume),
        None,
    )?;

    let mut dest_by_identity: HashMap<(DateTime<Utc>, i64), Vec<Event>> = HashMap::new();
    for event in dest_events {
        if event.timestamp >= lookback_start && event.timestamp < resume {
            dest_by_identity
                .entry(event_identity(&event))
                .or_default()
                .push(event);
        }
    }

    let mut src_by_identity: HashMap<(DateTime<Utc>, i64), Vec<Event>> = HashMap::new();
    for src in source_events {
        // Skip events that start at/after the dest cursor; the incremental
        // copy owns those. Do not use end>resume: the dest-latest event starts
        // before resume and must still be title-reconciled.
        if src.timestamp < lookback_start || src.timestamp >= resume {
            continue;
        }
        src_by_identity
            .entry(event_identity(&src))
            .or_default()
            .push(src);
    }

    for (identity, srcs) in src_by_identity {
        let mut dsts = match dest_by_identity.remove(&identity) {
            Some(dsts) if !dsts.is_empty() => dsts,
            _ => continue,
        };
        let mut to_insert = Vec::new();
        for src in srcs {
            if let Some(idx) = dsts.iter().position(|dst| dst.data == src.data) {
                // Still present on dest — keep it, including same-identity
                // siblings a later source row must not treat as stale.
                dsts.remove(idx);
            } else {
                to_insert.push(src);
            }
        }
        if to_insert.is_empty() && dsts.is_empty() {
            continue;
        }
        let ts = identity.0;
        // Insert before delete so a crash cannot drop the row. A later pass
        // sees matching data, skips insert, and still removes remaining stale ids.
        if !to_insert.is_empty() {
            let replacements: Vec<Event> = to_insert
                .into_iter()
                .map(|mut src| {
                    src.id = None;
                    src
                })
                .collect();
            ds_to.insert_events(bucket_to.id.as_str(), replacements)?;
        }
        let stale: Vec<i64> = dsts.into_iter().filter_map(|dst| dst.id).collect();
        if !stale.is_empty() {
            ds_to.delete_events_by_id(bucket_to.id.as_str(), stale)?;
        }
        info!("   ~ Reconciled edited event at {:?}", ts);
    }
    Ok(())
}

/// Syncs a single bucket from one datastore to another
fn sync_one(
    ds_from: &dyn AccessMethod,
    ds_to: &dyn AccessMethod,
    bucket_from: Bucket,
    bucket_to: Bucket,
    sync_spec: &SyncSpec,
) -> Result<(), String> {
    let eventcount_to_old = ds_to.get_event_count(bucket_to.id.as_str())?;
    info!(" ⟳  Syncing bucket '{}'", bucket_to.id);

    // Sync events
    // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
    // for empty buckets (Should be None, is Some(unknown_time))
    // let resume_sync_at = bucket_to.metadata.end;
    let most_recent_events = ds_to.get_events(bucket_to.id.as_str(), None, None, Some(1))?;
    // If the destination bucket already has events, resume from where it left off.
    // Otherwise (first sync of this bucket), fall back to sync_spec.start, if specified.
    let resume_sync_at = most_recent_events
        .first()
        .map(|e| e.timestamp + e.duration)
        .or(sync_spec.start);

    if let Some(resume_time) = resume_sync_at {
        info!("   + Resuming at {:?}", resume_time);
    } else {
        info!("   + Starting from beginning");
    }

    reconcile_updated_events(ds_from, ds_to, &bucket_from, &bucket_to, resume_sync_at)?;

    // Fetch events in bounded chunks to avoid OOM on devices with limited RAM (e.g. Android).
    // get_events returns events in descending order (newest first), so we paginate backwards
    // using the `end` parameter. Each chunk is written to `ds_to` as soon as it is fetched,
    // so peak memory is O(BATCH_SIZE), not O(total events in the bucket).
    //
    // Each chunk is reversed before writing so events are inserted oldest-first (matching the
    // insertion order in the source DB). This preserves consistent ID assignment across source
    // and destination, which the sync tests rely on.
    //
    // Heartbeat semantics at the resume boundary: we use heartbeat() for the globally-oldest
    // new event ONLY in the single-page case (pages_written == 0 when is_last_fetch fires).
    // In that case dest's "last event" is still the pre-sync resume-boundary row, so
    // heartbeat() can correctly merge an adjacent new event into it.
    //
    // In the multi-page case (pages_written > 0), newer pages have already been inserted and
    // dest's "last event" is no longer the resume boundary — heartbeat() would compare against
    // the wrong row and skip the merge anyway. Inserting the oldest event directly is correct.
    let mut fetch_end: Option<DateTime<Utc>> = None;
    let mut events_sent = 0usize;
    let mut pages_written = 0u32;

    loop {
        let raw = ds_from.get_events(
            bucket_from.id.as_str(),
            resume_sync_at,
            fetch_end,
            Some(BATCH_SIZE as u64),
        )?;

        if raw.is_empty() {
            break;
        }

        // Fewer events than requested means there's nothing older left to fetch.
        let is_last_fetch = raw.len() < BATCH_SIZE;

        let mut chunk: Vec<Event> = raw
            .into_iter()
            .map(|mut e| {
                // Unset ID on events, as they are not globally unique
                e.id = None;
                e
            })
            .collect();

        if !is_last_fetch {
            // chunk is in DESC order (newest first); chunk.last() = oldest in this (full) page.
            // Naively setting the next `end` to `oldest.timestamp - 1ns` silently drops events
            // if the page happens to end mid-run of same-timestamp events: anything else at
            // that exact timestamp would fall outside the next page's range. Guard against that
            // by dropping ALL trailing events at `boundary_ts` from this page and leaving them
            // for the next fetch (whose `end = boundary_ts` is inclusive, so it re-fetches
            // the whole tied run at once).
            //
            // Note: we must drop the boundary event itself, not just its duplicates. Keeping
            // one copy in this chunk while also setting `fetch_end = Some(boundary_ts)` (inclusive)
            // would cause that event to be fetched again next page, producing a duplicate row.
            // `raw` was non-empty and `chunk` is a 1:1 map of it, so both ends exist.
            let (Some(oldest), Some(newest)) = (chunk.last(), chunk.first()) else {
                return Err(format!(
                    "Empty event page while syncing bucket '{}'",
                    bucket_from.id
                ));
            };
            let boundary_ts = oldest.timestamp;
            if newest.timestamp != boundary_ts {
                // Safe to pop all boundary_ts events: the `if` guard ensures at least one
                // earlier event (with a different timestamp) remains in the chunk.
                while chunk.last().is_some_and(|e| e.timestamp == boundary_ts) {
                    chunk.pop();
                }
                fetch_end = Some(boundary_ts);
            } else {
                // Pathological case: every event in this full page shares the exact same
                // timestamp, so we can't tell where the tied run ends without an unbounded
                // query. This can't occur with AW's event model in practice (activity records
                // span seconds+) — accept the page as-is rather than looping forever.
                fetch_end = Some(boundary_ts - Duration::nanoseconds(1));
            }

            // Reverse to ASC order (oldest first) before inserting.
            chunk.reverse();
            events_sent += chunk.len();
            pages_written += 1;
            for batch in chunk.chunks(BATCH_SIZE) {
                print!("({}/…)\r", events_sent);
                ds_to.insert_events(bucket_to.id.as_str(), batch.to_vec())?;
            }
        } else {
            // Last (oldest) page: process oldest-first to preserve ID ordering.
            chunk.reverse(); // chunk is now ASC (oldest first)

            // Use heartbeat() for the oldest event only in the single-page case:
            // dest's "last event" is still the pre-sync resume-boundary row, so heartbeat()
            // can correctly merge an adjacent new event into it (delta=0.0 → exact adjacency).
            // In multi-page syncs, newer pages are already in dest, so heartbeat() would
            // compare against the wrong row — insert directly instead.
            if !chunk.is_empty() && pages_written == 0 {
                let oldest = chunk.remove(0);
                ds_to.heartbeat(bucket_to.id.as_str(), oldest, 0.0)?;
                events_sent += 1;
            }

            // Insert the remaining events from the last page in ASC order.
            if !chunk.is_empty() {
                events_sent += chunk.len();
                for batch in chunk.chunks(BATCH_SIZE) {
                    print!("({}/…)\r", events_sent);
                    ds_to.insert_events(bucket_to.id.as_str(), batch.to_vec())?;
                }
            }

            break;
        }
    }

    let eventcount_to_new = ds_to.get_event_count(bucket_to.id.as_str())?;
    let new_events_count = eventcount_to_new - eventcount_to_old;
    if new_events_count < 0 {
        return Err(format!(
            "Event count of bucket '{}' shrank during sync ({eventcount_to_old} -> {eventcount_to_new})",
            bucket_to.id
        ));
    }
    if new_events_count > 0 {
        info!("  = Synced {} new events", new_events_count);
    } else {
        info!("  ✓ Already up to date!");
        // Resume-from-newest is one-way: events older than dest's newest are never
        // fetched. If a short recent slice was imported first, a later pull of the
        // complete history reports "up to date" while dest is missing most of it.
        // Warn loudly so that case is diagnosable (ActivityWatch/aw-server-rust#683).
        if resume_sync_at.is_some() {
            let src_count = ds_from.get_event_count(bucket_from.id.as_str())?;
            if src_count > eventcount_to_new {
                warn!(
                    "  ! Source bucket '{}' has {src_count} events but destination '{}' has {eventcount_to_new} after a resume-from-newest pull. Older history in the source was not imported. Delete the destination bucket and re-pull to recover.",
                    bucket_from.id, bucket_to.id
                );
            }
        }
    }

    Ok(())
}

fn log_buckets(ds: &dyn AccessMethod) -> Result<(), String> {
    // Logs all buckets and some metadata for a given datastore
    let buckets = ds.get_buckets()?;
    info!("Buckets in {:?}:", ds);
    for bucket in buckets.values() {
        info!(" - {}", bucket.id.as_str());
        info!(
            "   eventcount: {:?}",
            ds.get_event_count(bucket.id.as_str())?
        );
    }
    Ok(())
}

#[cfg(test)]
mod pull_only_staging_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "aw-sync-pull-only-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn pull_does_not_create_local_staging_db() {
        let dir = temp_dir();
        let staged = maybe_setup_local_remote(&dir, "device-local", SyncMode::Pull).unwrap();
        assert!(staged.is_none());
        assert!(
            !dir.join("device-local").exists(),
            "pull-only must not create {{peer}}/{{our_device_id}}/"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pull_creates_missing_sync_root_without_local_staging() {
        let dir = temp_dir();
        fs::remove_dir_all(&dir).unwrap();
        assert!(!dir.exists());
        let staged = maybe_setup_local_remote(&dir, "device-local", SyncMode::Pull).unwrap();
        assert!(staged.is_none());
        assert!(
            dir.is_dir(),
            "pull-only must create the sync root so discovery does not NotFound"
        );
        assert!(
            !dir.join("device-local").exists(),
            "pull-only must not create {{peer}}/{{our_device_id}}/"
        );
        let remotes = crate::util::find_remotes_nonlocal(&dir, "device-local", None).unwrap();
        assert!(remotes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_creates_local_staging_dir() {
        let dir = temp_dir();
        let staged = maybe_setup_local_remote(&dir, "device-local", SyncMode::Push).unwrap();
        assert!(staged.is_some());
        // Datastore::new opens sqlite on a worker thread, so test.db may not
        // exist yet; the directory is created synchronously and is the leak
        // pull-only used to leave in a peer folder.
        assert!(dir.join("device-local").is_dir());
        if let Some(ds) = staged {
            ds.close();
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
