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

use crate::accessmethod::AccessMethod;
use crate::report::{BucketReport, PeerReport};
use crate::util::{
    find_remotes_nonlocal_selection, list_remote_dbs, select_remote_dbs_detailed, RemoteDb,
};

pub use crate::report::{SyncMode, SyncReport};

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

/// Discover all sync peers reachable from `sync_root`, excluding `own_device_id`.
///
/// Unions the 3-level Android/new-desktop walker (`list_remote_dbs`) with the
/// 2-level legacy-desktop walker (`find_remotes_nonlocal_selection`), then
/// deduplicates by device_id keeping the largest file per device
/// (`select_remote_dbs_detailed`).  Called by `sync_run` when `path_db` is
/// `None` (daemon / root call) and by the regression tests so both exercise the
/// same production path.
pub(crate) fn discover_peers(
    sync_root: &Path,
    own_device_id: &str,
) -> Result<crate::util::RemoteSelection, Box<dyn Error>> {
    let mut all_dbs: Vec<RemoteDb> = list_remote_dbs(sync_root)?
        .into_iter()
        .filter(|db| db.device_id != own_device_id)
        .collect();
    let two_level = find_remotes_nonlocal_selection(sync_root, own_device_id, None)?;
    all_dbs.extend(two_level.selected);
    all_dbs.extend(two_level.skipped.into_iter().map(|s| s.db));
    Ok(select_remote_dbs_detailed(all_dbs))
}

/// Performs a single sync pass and returns what it did.
///
/// A `Ok` report is complete. On error, a partial report (failed peer, any
/// peers already imported) is persisted so `aw-sync status` can show the
/// failure without the process having to stay alive.
pub fn sync_run(
    client: &AwClient,
    sync_spec: &SyncSpec,
    mode: SyncMode,
) -> Result<SyncReport, Box<dyn Error>> {
    let mut report = SyncReport::new(mode);
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

    // Peer discovery: union 3-level (Android / new desktop layout) and
    // 2-level (legacy desktop) walkers so the daemon finds the same peers
    // that `pull_all` and `aw-sync status` report.
    //
    // When a specific path_db is provided (sync_wrapper::pull, which passes a
    // peer's host folder + the exact db file), stay on the 2-level path — the
    // path is already a host subdirectory, not the sync root, so list_remote_dbs
    // would walk into grandchildren and find nothing useful.
    let selection = if sync_spec.path_db.is_none() {
        discover_peers(sync_spec.path.as_path(), device_id)?
    } else {
        find_remotes_nonlocal_selection(
            sync_spec.path.as_path(),
            device_id,
            sync_spec.path_db.as_ref(),
        )?
    };
    let remote_dbfiles: Vec<_> = selection.selected.iter().map(|d| d.path.clone()).collect();

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
    // ActivityWatch/aw-server-rust#682 / #695. Do not stay silent: keep the
    // warnings on the report, not just in the log.
    if mode == SyncMode::Pull || mode == SyncMode::Both {
        report.capture_warnings(crate::util::pull_discovery_warnings(
            sync_spec.path.as_path(),
            Some(device_id),
            &remote_dbfiles,
        ));
        for skipped in &selection.skipped {
            report.peers.push(PeerReport::skipped(
                skipped.db.device_id.clone(),
                skipped.db.hostname.clone(),
                skipped.db.path.clone(),
                skipped.reason.clone(),
            ));
        }
    }

    // Peer files are opened read-only: never migrate, never flip WAL
    // (ActivityWatch/aw-server-rust#693). Version mismatch is skipped, not fatal —
    // but it must be *visible*: record it on the report so status/JNI do not
    // present an all-incompatible pass as a clean empty one.
    // A single unreadable db must not abort the pass
    // (ActivityWatch/aw-server-rust#688): record it and keep walking; only a
    // total open failure is fatal.
    // Peer datastores are only needed for pulling: a Push-only pass must not
    // open (or abort on) peer databases at all — pushing only uses the local
    // staging datastore, and unreadable/incompatible peers would otherwise
    // block or silently taint a push pass.
    let mut ds_remotes = Vec::new();
    if mode == SyncMode::Pull || mode == SyncMode::Both {
        match open_peer_datastores(&selection.selected, &mut report, true) {
            Ok(opened) => ds_remotes = opened,
            Err(e) => {
                report.finish();
                crate::report::persist_last_report_warn(&report);
                close_opened_datastores(&ds_remotes, &ds_localremote);
                return Err(e.into());
            }
        }
    }

    if !ds_remotes.is_empty() {
        info!(
            "Found {} remote datastores: {:?}",
            ds_remotes.len(),
            ds_remotes.iter().map(|(_, ds)| ds).collect::<Vec<_>>()
        );
    }

    // Pull
    if mode == SyncMode::Pull || mode == SyncMode::Both {
        info!("Pulling...");
        let remotes: Vec<(&RemoteDb, &dyn AccessMethod)> = ds_remotes
            .iter()
            .map(|(db, ds)| (db, ds as &dyn AccessMethod))
            .collect();
        if let Err(e) = pull_from_remotes(&remotes, client, sync_spec, &mut report, true) {
            report.finish();
            crate::report::persist_last_report_warn(&report);
            close_opened_datastores(&ds_remotes, &ds_localremote);
            return Err(e.into());
        }
    }

    // Push local server buckets to sync folder
    if let Some(ds_local) = &ds_localremote {
        info!("Pushing...");
        match sync_datastores(client, ds_local, true, Some(device_id), sync_spec) {
            Ok(buckets) => report.pushed = buckets,
            Err(e) => {
                report.record_push_failure(&e);
                report.finish();
                crate::report::persist_last_report_warn(&report);
                close_opened_datastores(&ds_remotes, &ds_localremote);
                return Err(e.into());
            }
        }
    }

    // Close open database connections
    close_opened_datastores(&ds_remotes, &ds_localremote);

    // Dropping also works to close the database connections, weirdly enough.
    // Probably because once the database is dropped, the thread will stop,
    // and then the Connection will be dropped, which closes the connection.
    std::mem::drop(ds_remotes);
    std::mem::drop(ds_localremote);

    // NOTE: Will fail if db connections not closed (as it will open them again)
    //list_buckets(&client, sync_spec.path.as_path());

    report.finish();
    Ok(report)
}

/// Stop datastore worker threads. Drop alone does not wait for the sqlite
/// lock; the success path already called `close()` for that reason. Error
/// returns must do the same or a long-lived daemon can leak connections
/// across failed passes.
fn close_opened_datastores(
    ds_remotes: &[(crate::util::RemoteDb, Datastore)],
    ds_localremote: &Option<Datastore>,
) {
    for (_, ds_from) in ds_remotes {
        ds_from.close();
    }
    if let Some(ds) = ds_localremote {
        ds.close();
    }
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

    let mut ds_remotes = Vec::new();
    for path in &remote_dbfiles {
        if let OpenedPeer::Ready(ds) = open_peer_datastore(path)? {
            ds_remotes.push(ds);
        }
    }

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
    let pathstr = utf8_db_path(path)?;
    Ok(Datastore::new(pathstr.to_string(), false))
}

/// Open a *peer* database for pull: read-only, no migration, no WAL sidecars.
///
/// Returns `OpenedPeer::Incompatible` when `user_version` does not match this
/// binary so the caller can skip that peer and keep walking, keeping the
/// version-mismatch reason for reporting (ActivityWatch/aw-server-rust#693).
enum OpenedPeer {
    Ready(Datastore),
    Incompatible(String),
}

fn open_peer_datastore(path: &Path) -> Result<OpenedPeer, String> {
    let pathstr = utf8_db_path(path)?;
    match Datastore::open_read_only(pathstr.to_string()) {
        Ok(ds) => Ok(OpenedPeer::Ready(ds)),
        Err(DatastoreError::OldDbVersion(msg)) => {
            warn!("Skipping peer db {}: {msg}", path.display());
            Ok(OpenedPeer::Incompatible(msg))
        }
        Err(e) => Err(format!(
            "Failed to open remote db {}: {e:?}",
            path.display()
        )),
    }
}

/// Open each discovered peer db, skipping unreadable ones.
///
/// A single unreadable peer is recorded (when `record_peers`) and skipped, not
/// fatal (ActivityWatch/aw-server-rust#688). Only a total open failure — every
/// discovered peer failed to open, none usable — is `Err`, so a broken folder
/// is not reported as a clean empty pass.
fn open_peer_datastores(
    selected: &[RemoteDb],
    report: &mut SyncReport,
    record_peers: bool,
) -> Result<Vec<(RemoteDb, Datastore)>, String> {
    let mut opened: Vec<(RemoteDb, Datastore)> = Vec::new();
    let mut failures = 0usize;
    let mut last_err: Option<String> = None;
    for db in selected {
        match open_peer_datastore(&db.path) {
            Ok(OpenedPeer::Ready(ds)) => opened.push((db.clone(), ds)),
            Ok(OpenedPeer::Incompatible(msg)) => {
                if record_peers {
                    report.peers.push(PeerReport::skipped(
                        db.device_id.clone(),
                        db.hostname.clone(),
                        db.path.clone(),
                        format!("incompatible database version: {msg}"),
                    ));
                }
            }
            Err(e) => {
                warn!("Skipping unreadable peer db {}: {e}", db.path.display());
                failures += 1;
                last_err = Some(e.clone());
                if record_peers {
                    report.peers.push(PeerReport::failed(
                        db.device_id.clone(),
                        db.hostname.clone(),
                        db.path.clone(),
                        e.clone(),
                    ));
                }
            }
        }
    }
    if opened.is_empty() && failures > 0 {
        return Err(format!(
            "all {failures} discovered peers failed to open; last error: {}",
            last_err.as_deref().unwrap_or("unknown")
        ));
    }
    Ok(opened)
}

/// Pull each remote independently so a broken peer does not abort the pass
/// (ActivityWatch/aw-server-rust#688). Partial failure is recorded (when
/// `record_peers`) and non-fatal; total failure (every remote failed, none
/// succeeded) is still Err so a down destination is not reported as success.
fn pull_from_remotes(
    remotes: &[(&RemoteDb, &dyn AccessMethod)],
    dest: &dyn AccessMethod,
    sync_spec: &SyncSpec,
    report: &mut SyncReport,
    record_peers: bool,
) -> Result<(), String> {
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut last_err: Option<String> = None;
    for (db, ds_from) in remotes {
        attempted += 1;
        match sync_datastores(*ds_from, dest, false, None, sync_spec) {
            Ok(buckets) => {
                succeeded += 1;
                if record_peers {
                    report.peers.push(PeerReport::imported(
                        db.device_id.clone(),
                        db.hostname.clone(),
                        db.path.clone(),
                        buckets,
                    ));
                }
            }
            Err(e) => {
                warn!("Skipping peer {}: {e}", db.hostname);
                last_err = Some(e.clone());
                if record_peers {
                    report.peers.push(PeerReport::failed(
                        db.device_id.clone(),
                        db.hostname.clone(),
                        db.path.clone(),
                        e.clone(),
                    ));
                }
            }
        }
    }
    if attempted > 0 && succeeded == 0 {
        return Err(format!(
            "all {attempted} peers failed; last error: {}",
            last_err.as_deref().unwrap_or("unknown")
        ));
    }
    Ok(())
}

fn utf8_db_path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("Sync database path is not valid UTF-8: {}", path.display()))
}

/// Sanitize a device hostname so it is a legal bucket hostname and a stable
/// sync-ID suffix.
///
/// Must stay byte-identical to aw-android's `sanitizeDeviceHostname`
/// (`mobile/src/main/java/net/activitywatch/android/DeviceHostname.kt`):
/// trim, lowercase, replace `[^a-z0-9_-]+` with `_`, trim `_`. Empty result
/// becomes `"unknown"`.
///
/// Divergence here forks destination buckets the day Android migrates its
/// hostname column (ActivityWatch/aw-android#272) onto a different
/// `-synced-from-` ID (ActivityWatch/activitywatch#1373).
pub fn sanitize_hostname(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return "unknown".to_string();
    }
    let lower = value.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_run = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
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

    // Look up the unsanitized ID first.  Any device that was synced before
    // aw-android added hostname sanitization (ActivityWatch/aw-android#272) may
    // have left a local bucket whose ID and hostname contain whitespace (e.g.
    // `…-synced-from-POCO F8 Ultra`) or a case-only fork (`…-synced-from-PIXEL8`).
    // Keep using that ID to avoid a full re-import (ActivityWatch/activitywatch#1373).
    match ds_to.get_bucket(new_id.as_str()) {
        Ok(bucket) => return Ok(bucket),
        Err(DatastoreError::NoSuchBucket(_)) => {}
        Err(e) => return Err(format!("Failed to get bucket '{new_id}': {e:?}")),
    }

    // Always sanitize. DeviceHostname.kt lowercases and replaces punctuation,
    // not just whitespace — `PIXEL8` vs `pixel8` is the same fork as
    // `POCO F8 Ultra` vs `poco_f8_ultra`, just without spaces. Lookup order
    // stays *raw ID → sanitized ID*; create under the sanitized ID whenever it
    // differs from raw so Android's hostname-column migration lands on an
    // existing bucket regardless of which character class differed.
    //
    // Desktop peers with dots (`erb-m2.localdomain`) will create
    // `…-synced-from-erb-m2_localdomain` for *new* imports; existing ones are
    // found via the raw lookup. That is a display wart the `(device_id, id)`
    // identity work in ActivityWatch/activitywatch#302 removes — not a reason
    // to keep the fork open.
    let sanitized_hostname = sanitize_hostname(&bucket_from.hostname);
    let sanitized_id = if let Some(ref origin) = sync_origin {
        let orig_bucketid = bucket_from
            .id
            .split("-synced-from-")
            .next()
            .unwrap_or(bucket_from.id.as_str());
        format!("{orig_bucketid}-synced-from-{}", sanitize_hostname(origin))
    } else {
        // Push path: keep the original bucket ID; only the hostname field
        // needs to be a legal create_bucket value.
        new_id.clone()
    };
    // Android maps empty/punctuation-only names to the "unknown" sentinel.
    // Creating `-synced-from-unknown` on pull would mix every such remote
    // into one destination — the same provenance hole the
    // `hostname == "unknown"` guard in `sync_datastores` exists to close.
    // Refuse; the per-bucket warn+continue then skips this bucket.
    if !is_push
        && (sanitized_hostname == "unknown" || sanitized_id.ends_with("-synced-from-unknown"))
    {
        return Err(format!(
            "Bucket '{}' hostname sanitizes to the unknown sentinel; \
             refusing to sync it without provenance",
            bucket_from.id
        ));
    }
    if sanitized_id != new_id {
        match ds_to.get_bucket(sanitized_id.as_str()) {
            Ok(bucket) => return Ok(bucket),
            Err(DatastoreError::NoSuchBucket(_)) => {}
            Err(e) => return Err(format!("Failed to get bucket '{sanitized_id}': {e:?}")),
        }
    }

    // Pre-#697 origin-based fallback (ActivityWatch/aw-server-rust#707):
    // Both exact lookups missed.  A desktop that imported the peer before
    // ActivityWatch/aw-server-rust#697 landed holds the raw hostname from that
    // day (e.g. `…-synced-from-POCO F8 Ultra`) with `$aw.sync.origin` set to
    // that same raw value.  After ActivityWatch/aw-android#273 migrates the
    // phone's staging hostname to `poco_f8_ultra`, first-hand buckets carry no
    // `$aw.sync.origin`, so both lookups above miss and the whole history gets
    // re-imported.  Scan the destination's -synced-from- buckets for one whose
    // base ID matches and whose `$aw.sync.origin` sanitizes to the same target.
    if !is_push {
        let target_base = bucket_from
            .id
            .split("-synced-from-")
            .next()
            .unwrap_or(bucket_from.id.as_str());
        let target_sanitized = sanitize_hostname(
            sync_origin
                .as_deref()
                .unwrap_or(bucket_from.hostname.as_str()),
        );
        let all_dest = ds_to
            .get_buckets()
            .map_err(|e| format!("Failed to list dest buckets for origin scan: {e:?}"))?;
        let mut candidates: Vec<Bucket> = all_dest
            .into_values()
            .filter(|b| {
                let b_base = b.id.split("-synced-from-").next().unwrap_or(&b.id);
                if b_base != target_base {
                    return false;
                }
                // $aw.sync.origin must be present and sanitize to the same value.
                b.data
                    .get("$aw.sync.origin")
                    .and_then(|v| v.as_str())
                    .map(|s| sanitize_hostname(s) == target_sanitized)
                    .unwrap_or(false)
            })
            .collect();
        match candidates.len() {
            0 => {} // fall through to create a new bucket
            1 => {
                let found = candidates.remove(0);
                info!(
                    "   ↩  Reusing pre-#697 bucket '{}' for '{}'",
                    found.id, bucket_from.id
                );
                return Ok(found);
            }
            n => {
                // Two distinct pre-#697 buckets share the same sanitized origin —
                // ambiguous.  Refuse rather than silently merging distinct histories
                // (ActivityWatch/aw-server-rust#697 :368).
                let ids: Vec<&str> = candidates.iter().map(|b| b.id.as_str()).collect();
                return Err(format!(
                    "Cannot resolve destination for '{}': {n} pre-#697 buckets share \
                     sanitized origin '{}': {ids:?}; deduplicate manually",
                    bucket_from.id, target_sanitized
                ));
            }
        }
    }

    let (final_id, final_hostname) = (sanitized_id, sanitized_hostname);

    let mut bucket_new = bucket_from.clone();
    bucket_new.id = final_id.clone();
    bucket_new.hostname = final_hostname;
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
        .map_err(|e| format!("Failed to create bucket '{final_id}': {e:?}"))?;
    ds_to
        .get_bucket(final_id.as_str())
        .map_err(|e| format!("Failed to read back bucket '{final_id}': {e:?}"))
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
pub(crate) fn is_synced_bucket(bucket: &Bucket) -> bool {
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
) -> Result<Vec<BucketReport>, String> {
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

    // Partial failure is non-fatal (one bad bucket must not skip the rest).
    // Total failure must still be Err: otherwise a destination that is down
    // reports success, which is the #682 silence reintroduced via #688's skip.
    let mut buckets = Vec::with_capacity(buckets_from.len());
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut last_err: Option<String> = None;
    for bucket_from in buckets_from {
        attempted += 1;
        let bucket_id = bucket_from.id.clone();
        let bucket_to = match get_or_create_sync_bucket(&bucket_from, ds_to, is_push) {
            Ok(b) => b,
            Err(e) => {
                warn!(" ! Skipping bucket '{}': {}", bucket_id, e);
                last_err = Some(e);
                continue;
            }
        };
        match sync_one(ds_from, ds_to, bucket_from, bucket_to, sync_spec) {
            Ok(synced) => {
                succeeded += 1;
                buckets.push(synced);
            }
            Err(e) => {
                warn!(
                    " ! Skipping sync for bucket '{bucket_id}': {e}. \
                     Destination may already contain a partial write; next pass resumes from dest newest"
                );
                last_err = Some(e);
            }
        }
    }
    if attempted > 0 && succeeded == 0 {
        return Err(format!(
            "all {attempted} buckets failed; last error: {}",
            last_err.as_deref().unwrap_or("unknown")
        ));
    }

    Ok(buckets)
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
) -> Result<BucketReport, String> {
    let eventcount_to_old = ds_to.get_event_count(bucket_to.id.as_str())?;
    info!(" ⟳  Syncing bucket '{}'", bucket_to.id);

    // Sync events
    // FIXME: This should use bucket_to.metadata.end, but it doesn't because it doesn't work
    // for empty buckets (Should be None, is Some(unknown_time))
    // let resume_sync_at = bucket_to.metadata.end;
    let most_recent_events = ds_to.get_events(bucket_to.id.as_str(), None, None, Some(1))?;
    // If the destination bucket already has events, resume from where it left off.
    // Otherwise (first sync of this bucket), fall back to sync_spec.start, if specified.
    let newest_timestamp = most_recent_events.first().map(|e| e.timestamp);
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

    // Build a fingerprint set of events already at the tail of the destination, to dedup
    // the source fetch against events that overlap the resume boundary. The source fetch
    // uses get_events(start=resume_sync_at) with overlap semantics: it returns any event
    // whose (timestamp + duration >= resume_sync_at). This includes:
    //   - duration=0 events at resume_sync_at (Symptom 1: re-imported every pass)
    //   - events at newest.timestamp with duration > 0 whose end == resume_sync_at
    //     (Symptom 2: timestamp-tie — a different event at the same timestamp)
    //   - events that started before resume_sync_at and straddle it (Symptom 3: the source
    //     query clips such an event's returned start forward to resume_sync_at, zeroing its
    //     returned duration)
    //
    // Fetch by LIMIT rather than a start-time filter: an unfiltered get_events() cannot clip
    // anything (clipping only triggers when a start/end bound cuts into an event), so these
    // destination fingerprints are always the true, unclipped values. This also removes the
    // dependency on peewee-vs-Rust-datastore asymmetry in start-boundary inclusivity that
    // motivated the old `-1ms` workaround.
    //
    // Fingerprint on (end_time, data) instead of (start_time, duration, data): an event's end
    // time is invariant under the source-side start-clipping described above (clipping only
    // moves the returned start forward and shrinks duration to compensate; the end is
    // unchanged), so it's the only representation that reliably matches a clipped chunk event
    // to its unclipped destination counterpart. Nanosecond precision avoids millisecond-window
    // collisions between distinct events, consistent with the datastore's own event identity.
    //
    // Counted (not a HashSet): the fetched page can legitimately contain more than one event
    // sharing a fingerprint (e.g. a bucket already corrupted by this bug pre-fix, or two
    // genuinely distinct events that happen to share end-time and data). A HashSet would drop
    // *every* source event with a matching fingerprint; counting only skips as many as are
    // actually confirmed present in the destination, so any additional occurrences in the
    // source are still treated as new and synced.
    //
    // Bounded by LIMIT rather than a start/end filter: any bound on this fetch would let
    // clip_to_query_range clip events straddling it (the same failure mode this fingerprint
    // scheme exists to route around — see the source-fetch comment above), and the AccessMethod
    // trait has no HTTP-safe unclipped variant to page backward with instead. A page this large
    // covers every observed real-world tie run (see #711 — up to ~550 duplicate rows at one
    // timestamp) with over an order of magnitude of margin; a bucket with a longer unresolved
    // tie than this is already corrupted well beyond what a resume-boundary dedup can repair —
    // that's the one-off cleanup tracked as a separate follow-up issue.
    const BOUNDARY_DEDUP_LOOKBACK: u64 = 2000;
    let boundary_dedup: std::collections::HashMap<(i64, String), usize> = if newest_timestamp
        .is_some()
    {
        let mut counts = std::collections::HashMap::new();
        for e in ds_to
            .get_events(
                bucket_to.id.as_str(),
                None,
                None,
                Some(BOUNDARY_DEDUP_LOOKBACK),
            )
            .map_err(|e| format!("Failed to fetch destination boundary events for dedup: {e}"))?
        {
            let fp = (
                (e.timestamp + e.duration)
                    .timestamp_nanos_opt()
                    .unwrap_or(0),
                serde_json::to_string(&e.data).unwrap_or_default(),
            );
            *counts.entry(fp).or_insert(0) += 1;
        }
        counts
    } else {
        std::collections::HashMap::new()
    };

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

            // Dedup: skip boundary events already in the destination, up to the count
            // confirmed present there. Consuming from the count (rather than a blanket
            // `contains` check) means a source page with MORE occurrences of a fingerprint
            // than exist in the destination still lets the extra ones through as genuinely
            // new — a plain set would silently drop all of them. Handles the duration=0
            // re-import, same-timestamp tie, and clipped-overlap cases (#711). Fingerprint
            // must match the (end_time, data) scheme used to build `boundary_dedup` above —
            // see the comment there for why.
            if !boundary_dedup.is_empty() {
                let mut remaining = boundary_dedup.clone();
                let before_dedup = chunk.len();
                chunk.retain(|e| {
                    let fp = (
                        (e.timestamp + e.duration)
                            .timestamp_nanos_opt()
                            .unwrap_or(0),
                        serde_json::to_string(&e.data).unwrap_or_default(),
                    );
                    match remaining.get_mut(&fp) {
                        Some(count) if *count > 0 => {
                            *count -= 1;
                            false
                        }
                        _ => true,
                    }
                });
                let skipped = before_dedup - chunk.len();
                if skipped > 0 {
                    info!("  - Skipped {} boundary duplicate(s)", skipped);
                }
            }

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

    Ok(BucketReport {
        bucket_id: bucket_to.id,
        events_new: new_events_count,
        resumed_at: resume_sync_at,
    })
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

#[cfg(test)]
mod boundary_dedup_multipage_tests {
    //! Regression coverage for the "Clipping Defeats Boundary Dedup" review
    //! finding on ActivityWatch/aw-server-rust#713.
    //!
    //! When the newest destination event has a *positive* duration,
    //! `resume_sync_at` is set to that event's end. The resumed source query
    //! re-fetches the same event clipped to the query start, i.e. with the
    //! event's original (timestamp, duration) replaced by (resume_sync_at, 0).
    //! In a *single-page* sync this clipped duplicate happens to be absorbed by
    //! `heartbeat()`'s delta=0.0 adjacency merge, masking the bug — so this
    //! test lives here (not in `tests/sync.rs`) to use the `#[cfg(test)]`
    //! `BATCH_SIZE = 5`, forcing a multi-page sync where the tail chunk is
    //! inserted via `insert_events()` instead, which exposes the duplicate
    //! directly if the dedup fingerprint fails to match.
    use super::*;

    #[test]
    fn no_duplicate_on_positive_duration_boundary_across_multiple_pages() {
        let ds_src = Datastore::new_in_memory(false);
        let ds_dest = Datastore::new_in_memory(false);

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "bucket-0",
            "type": "test",
            "hostname": "device-0",
            "client": "test"
        }))
        .unwrap();
        ds_src.create_bucket(&bucket).unwrap();
        let synced_id = "bucket-0-synced-from-device-0";
        let base_ts: DateTime<Utc> = Utc::now();

        // A single event with a positive duration — the only (and therefore
        // boundary) event in the bucket. Its end (base_ts + 182.639s) becomes
        // resume_sync_at.
        let boundary: Event = serde_json::from_value(serde_json::json!({
            "timestamp": base_ts.to_rfc3339(),
            "duration": 182.639,
            "data": {"label": "Testing"}
        }))
        .unwrap();
        ds_src.insert_events("bucket-0", &[boundary]).unwrap();
        ds_src.force_commit().unwrap();

        sync_datastores(&ds_src, &ds_dest, false, None, &SyncSpec::default()).unwrap();
        assert_eq!(
            ds_dest.get_event_count(synced_id, None, None).unwrap(),
            1,
            "first sync must import the positive-duration boundary event"
        );

        // BATCH_SIZE is 5 under #[cfg(test)]; 6 new events after the boundary
        // force a second page, whose tail includes the boundary's clipped
        // re-fetch and is written via insert_events() rather than heartbeat().
        let new_events: Vec<Event> = (0..6)
            .map(|i| {
                let ts = base_ts + Duration::seconds(200 + i * 10);
                serde_json::from_value(serde_json::json!({
                    "timestamp": ts.to_rfc3339(),
                    "duration": 0,
                    "data": {"label": format!("new-{i}")}
                }))
                .unwrap()
            })
            .collect();
        ds_src.insert_events("bucket-0", &new_events).unwrap();
        ds_src.force_commit().unwrap();

        sync_datastores(&ds_src, &ds_dest, false, None, &SyncSpec::default()).unwrap();
        assert_eq!(
            ds_dest.get_event_count(synced_id, None, None).unwrap(),
            7,
            "second sync must import the 6 new events without re-duplicating \
             the positive-duration boundary event across a page boundary"
        );
    }

    #[test]
    fn new_event_sharing_boundary_fingerprint_is_still_synced_across_multiple_pages() {
        // Regression for the "Fingerprint Drops Distinct Events" review finding on
        // ActivityWatch/aw-server-rust#713: the boundary-dedup fingerprint is
        // (end_time, data) only (duration/start are dropped because they aren't
        // clip-invariant), so a genuinely distinct source event that happens to end
        // at the same instant with the same data as an already-synced boundary event
        // must still be counted and synced — the dedup may only skip as many
        // occurrences of a fingerprint as are confirmed present in the destination,
        // not every source event that matches it.
        //
        // Uses the multi-page path (like the sibling test above) so the colliding
        // event is written via insert_events() rather than heartbeat(), whose
        // delta=0.0 adjacency merge would otherwise fold two same-data zero-gap
        // events into one and mask the distinction this test targets.
        let ds_src = Datastore::new_in_memory(false);
        let ds_dest = Datastore::new_in_memory(false);

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "bucket-0",
            "type": "test",
            "hostname": "device-0",
            "client": "test"
        }))
        .unwrap();
        ds_src.create_bucket(&bucket).unwrap();
        let synced_id = "bucket-0-synced-from-device-0";
        let base_ts: DateTime<Utc> = Utc::now();
        let boundary_end = base_ts + Duration::milliseconds((182.639 * 1000.0) as i64);

        let boundary: Event = serde_json::from_value(serde_json::json!({
            "timestamp": base_ts.to_rfc3339(),
            "duration": 182.639,
            "data": {"label": "Testing"}
        }))
        .unwrap();
        ds_src.insert_events("bucket-0", &[boundary]).unwrap();
        ds_src.force_commit().unwrap();

        sync_datastores(&ds_src, &ds_dest, false, None, &SyncSpec::default()).unwrap();
        assert_eq!(
            ds_dest.get_event_count(synced_id, None, None).unwrap(),
            1,
            "first sync must import the boundary event"
        );

        // A distinct event that starts later than the boundary but happens to end
        // at exactly the same instant with the exact same data — same fingerprint
        // as the already-synced boundary, but a genuinely different event.
        let collider: Event = serde_json::from_value(serde_json::json!({
            "timestamp": (boundary_end - Duration::seconds(50)).to_rfc3339(),
            "duration": 50.0,
            "data": {"label": "Testing"}
        }))
        .unwrap();
        // Six more events push the sync past BATCH_SIZE=5, forcing the collider
        // (chronologically before them) into a later page, written via
        // insert_events() rather than the single-page heartbeat() path.
        let new_events: Vec<Event> = (0..6)
            .map(|i| {
                let ts = base_ts + Duration::seconds(200 + i * 10);
                serde_json::from_value(serde_json::json!({
                    "timestamp": ts.to_rfc3339(),
                    "duration": 0,
                    "data": {"label": format!("new-{i}")}
                }))
                .unwrap()
            })
            .collect();
        ds_src.insert_events("bucket-0", &[collider]).unwrap();
        ds_src.insert_events("bucket-0", &new_events).unwrap();
        ds_src.force_commit().unwrap();

        sync_datastores(&ds_src, &ds_dest, false, None, &SyncSpec::default()).unwrap();
        assert_eq!(
            ds_dest.get_event_count(synced_id, None, None).unwrap(),
            8,
            "the fingerprint-colliding event must be synced alongside the 6 new \
             events, not dropped as a duplicate of the boundary"
        );

        sync_datastores(&ds_src, &ds_dest, false, None, &SyncSpec::default()).unwrap();
        assert_eq!(
            ds_dest.get_event_count(synced_id, None, None).unwrap(),
            8,
            "third sync must not re-import the boundary or the collider"
        );
    }
}

#[cfg(test)]
mod hostname_sanitize_tests {
    use super::sanitize_hostname;

    #[test]
    fn poco_f8_ultra_matches_android() {
        // The contract Erik asked for on ActivityWatch/aw-server-rust#697:
        // whitespace-only replace would produce "POCO_F8_Ultra" and fork the
        // day aw-android#272 migrates the phone's hostname column.
        assert_eq!(sanitize_hostname("POCO F8 Ultra"), "poco_f8_ultra");
        // Case-only fork: no whitespace, but Android still lowercases.
        assert_eq!(sanitize_hostname("PIXEL8"), "pixel8");
        // Dotted desktop hostname: punctuation becomes `_`.
        assert_eq!(
            sanitize_hostname("erb-m2.localdomain"),
            "erb-m2_localdomain"
        );
    }

    #[test]
    fn android_device_hostname_contract() {
        // Byte-identical to aw-android DeviceHostnameTest.kt.
        assert_eq!(sanitize_hostname("Pixel 8"), "pixel_8");
        assert_eq!(sanitize_hostname("My-Phone_1"), "my-phone_1");
        assert_eq!(sanitize_hostname("  Pixel  8  "), "pixel_8");
        assert_eq!(sanitize_hostname(""), "unknown");
        assert_eq!(sanitize_hostname("   "), "unknown");
        assert_eq!(sanitize_hostname("***"), "unknown");
        // Whitespace + punctuation only: the get_or_create pull-refuse path.
        assert_eq!(sanitize_hostname(" * "), "unknown");
        assert_eq!(sanitize_hostname(" !!! "), "unknown");
        assert_eq!(sanitize_hostname("PIXEL8"), "pixel8");
        assert_eq!(
            sanitize_hostname("erb-m2.localdomain"),
            "erb-m2_localdomain"
        );
    }
}

#[cfg(test)]
mod peer_isolation_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aw_models::Bucket;

    fn bucket(id: &str, hostname: &str) -> Bucket {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "type": "test",
            "hostname": hostname,
            "client": "test"
        }))
        .unwrap()
    }

    fn peer_db(label: &str, hostname: &str, path: PathBuf) -> RemoteDb {
        RemoteDb {
            hostname: hostname.to_string(),
            device_id: label.to_string(),
            path,
            size: 0,
        }
    }

    fn dummy_report() -> SyncReport {
        SyncReport::new(SyncMode::Pull)
    }

    /// A Datastore whose parent dir does not exist: `Datastore::new` still
    /// succeeds (open is lazy on the worker thread), but `sync_datastores`
    /// fails. Unique per call so the test is hermetic.
    fn unreadable_peer(label: &str) -> Datastore {
        let path = std::env::temp_dir().join(format!(
            "aw-sync-missing-{}-{}-{}/peer.db",
            label,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        create_datastore(&path).expect("path is valid UTF-8")
    }

    /// A broken peer listed first must not prevent a healthy sibling from
    /// importing. This is the daemon-path #688 abort: `sync_run` used `?` on
    /// each remote, so the first unreadable db skipped every peer after it.
    #[test]
    fn broken_peer_does_not_skip_healthy_sibling() {
        let healthy = Datastore::new_in_memory(false);
        healthy
            .create_bucket(&bucket("aw-watcher-window", "host-ok"))
            .unwrap();
        let dest = Datastore::new_in_memory(false);
        let broken = unreadable_peer("sibling");

        let db_a = peer_db("dev-a", "host-broken", PathBuf::from("/nonexistent-a.db"));
        let db_b = peer_db("dev-b", "host-ok", PathBuf::from("/nonexistent-b.db"));
        let remotes: Vec<(&RemoteDb, &dyn AccessMethod)> =
            vec![(&db_a, &broken), (&db_b, &healthy)];
        let mut report = dummy_report();
        pull_from_remotes(&remotes, &dest, &SyncSpec::default(), &mut report, false)
            .expect("partial failure must be Ok");

        let dest_buckets = dest.get_buckets().unwrap();
        assert!(
            dest_buckets.contains_key("aw-watcher-window-synced-from-host-ok"),
            "healthy sibling must still import, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        broken.close();
        healthy.close();
        dest.close();
    }

    #[test]
    fn all_peers_failing_is_still_err() {
        let dest = Datastore::new_in_memory(false);
        let broken_a = unreadable_peer("all-a");
        let broken_b = unreadable_peer("all-b");

        let db_a = peer_db("dev-a", "host-a", PathBuf::from("/nonexistent-a.db"));
        let db_b = peer_db("dev-b", "host-b", PathBuf::from("/nonexistent-b.db"));
        let remotes: Vec<(&RemoteDb, &dyn AccessMethod)> =
            vec![(&db_a, &broken_a), (&db_b, &broken_b)];
        let mut report = dummy_report();
        let err = pull_from_remotes(&remotes, &dest, &SyncSpec::default(), &mut report, false)
            .expect_err("total failure must be Err");
        assert!(
            err.contains("all 2 peers failed"),
            "error should report total failure, got: {err}"
        );
        broken_a.close();
        broken_b.close();
        dest.close();
    }

    #[test]
    fn no_discovered_peers_is_ok() {
        let mut report = dummy_report();
        let opened = open_peer_datastores(&[], &mut report, false).expect("zero peers is a no-op");
        assert!(opened.is_empty());
    }

    #[test]
    fn missing_peer_file_is_open_failure() {
        // Peer open is read-only and fails on a missing file.
        // create_datastore would have succeeded (lazy worker) and hidden this.
        let missing = std::env::temp_dir().join(format!(
            "aw-sync-missing-open-{}-{}/nope.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = peer_db("dev-missing", "host-missing", missing);
        let mut report = dummy_report();
        let err =
            open_peer_datastores(&[db], &mut report, false).expect_err("missing file must be Err");
        assert!(
            err.contains("all 1 discovered peers failed to open"),
            "error should report total open failure, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn all_open_failures_is_err() {
        use std::os::unix::ffi::OsStringExt;
        let bad = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/aw-sync-\xff.db".to_vec(),
        ));
        let db = peer_db("dev-bad", "host-bad", bad);
        let mut report = dummy_report();
        let err = open_peer_datastores(&[db], &mut report, false)
            .expect_err("all open failures must be Err");
        assert!(
            err.contains("all 1 discovered peers failed to open"),
            "error should report total open failure, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn mixed_open_keeps_the_readable_peer() {
        use std::os::unix::ffi::OsStringExt;
        let dir = std::env::temp_dir().join(format!(
            "aw-sync-open-mix-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let good = dir.join("peer.db");
        // open_peer_datastore is read-only: needs a current-version sqlite
        // file with schema. Datastore::new is WAL-lazy, so checkpoint after
        // close or the immutable probe sees a torn image.
        {
            let ds = create_datastore(&good).unwrap();
            ds.create_bucket(&bucket("aw-watcher-window", "host-ok"))
                .unwrap();
            ds.force_commit().unwrap();
            ds.close();
            let conn = rusqlite::Connection::open(&good).unwrap();
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
        let bad = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/aw-sync-\xff-mix.db".to_vec(),
        ));
        let db_bad = peer_db("dev-bad", "host-bad", bad);
        let db_good = peer_db("dev-good", "host-ok", good);
        let mut report = dummy_report();
        let opened = open_peer_datastores(&[db_bad, db_good], &mut report, false)
            .expect("partial open failure must be Ok");
        assert_eq!(opened.len(), 1);
        for (_, ds) in opened {
            ds.close();
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_skip_plus_open_failure_counts_only_failures() {
        // Incompatible-version skips must not inflate the all-fail count.
        let dir = std::env::temp_dir().join(format!(
            "aw-sync-open-vermix-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.db");
        {
            let conn = rusqlite::Connection::open(&old).unwrap();
            // Hermetic: pick a version guaranteed incompatible for any realistic
            // schema bump, instead of a hardcoded value that goes stale when
            // NEWEST_DB_VERSION reaches it.
            conn.pragma_update(None, "user_version", aw_datastore::NEWEST_DB_VERSION + 1000)
                .unwrap();
        }
        let missing = dir.join("missing.db");
        let db_old = peer_db("dev-old", "host-old", old);
        let db_missing = peer_db("dev-missing", "host-missing", missing);
        let mut report = dummy_report();
        let err = open_peer_datastores(&[db_old, db_missing], &mut report, false)
            .expect_err("hard open failure with no usable peer is Err");
        assert!(
            err.contains("all 1 discovered peers failed to open"),
            "count must be hard failures only, not discovered paths, got: {err}"
        );
        assert!(
            !err.contains("all 2 discovered peers failed to open"),
            "must not count the version-mismatch skip as an open failure, got: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Regression guard for ActivityWatch/aw-server-rust#709.
///
/// The daemon's peer discovery (sync_run with no path_db) must select 3-level
/// Android peers (`{hostname}/{device_id}/*.db`) in addition to legacy 2-level
/// ones.  Before the fix it used only the 2-level walker and reported
/// "Found 0 remote db files" for any Android peer.
#[cfg(test)]
mod daemon_peer_discovery_tests {
    use super::*;

    /// Uses `tempfile::tempdir()` for a directory name unique per call, not
    /// per-process: the three tests here run on parallel threads within the
    /// same process, so a pid+timestamp name (as used elsewhere in this file)
    /// can collide and make them share one directory (flaky, reproduced
    /// locally by Erik — ActivityWatch/aw-server-rust#710).
    /// `keep()` detaches the `TempDir` guard so the directory survives past
    /// this function, matching the manual `fs::remove_dir_all` cleanup each
    /// test already does.
    fn temp_dir() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    fn touch(path: &PathBuf) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn write_sized(path: &PathBuf, size: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, vec![0u8; size]).unwrap();
    }

    // Thin wrapper so tests exercise the actual production discovery path,
    // not a copy that can silently diverge.
    fn discover(sync_root: &PathBuf, own_device_id: &str) -> crate::util::RemoteSelection {
        super::discover_peers(sync_root, own_device_id).unwrap()
    }

    /// Simulates the daemon peer-discovery path (path_db = None) against a
    /// sync root that contains only a 3-level Android peer.
    #[test]
    fn daemon_discovers_three_level_android_peer() {
        let sync_root = temp_dir();
        // Android 3-level layout: {hostname}/{device_id}/sync.db
        touch(
            &sync_root
                .join("poco_f8_ultra")
                .join("41662faa-1234-5678-9abc-def012345678")
                .join("sync.db"),
        );

        let selection = discover(&sync_root, "local-device-id");

        assert_eq!(
            selection.selected.len(),
            1,
            "daemon must discover the Android 3-level peer; got: {:?}",
            selection.selected
        );
        assert_eq!(selection.selected[0].hostname, "poco_f8_ultra");
        assert_eq!(
            selection.selected[0].device_id,
            "41662faa-1234-5678-9abc-def012345678"
        );

        let _ = fs::remove_dir_all(&sync_root);
    }

    /// Verify the 2-level walker still finds legacy desktop peers so the
    /// union does not regress backward compatibility.
    #[test]
    fn daemon_discovers_two_level_legacy_peer() {
        let sync_root = temp_dir();
        touch(&sync_root.join("legacy-desktop-device").join("test.db"));

        let selection = discover(&sync_root, "local-device-id");

        assert_eq!(
            selection.selected.len(),
            1,
            "daemon must still discover the 2-level legacy peer; got: {:?}",
            selection.selected
        );
        assert_eq!(selection.selected[0].device_id, "legacy-desktop-device");

        let _ = fs::remove_dir_all(&sync_root);
    }

    /// Both a 3-level Android peer and a 2-level legacy peer must both be
    /// selected (different device IDs — no deduplication needed).
    #[test]
    fn daemon_discovers_mixed_layout_peers() {
        let sync_root = temp_dir();
        // 3-level Android peer
        touch(
            &sync_root
                .join("poco_f8_ultra")
                .join("android-dev-id")
                .join("sync.db"),
        );
        // 2-level legacy desktop peer
        touch(&sync_root.join("desktop-dev-id").join("test.db"));

        let selection = discover(&sync_root, "local-device-id");

        assert_eq!(
            selection.selected.len(),
            2,
            "daemon must discover both Android and legacy peers; got: {:?}",
            selection.selected
        );

        let _ = fs::remove_dir_all(&sync_root);
    }

    /// When the same device_id appears in both the 3-level (Android) and the
    /// 2-level (legacy) layout — e.g. a desktop that was migrated from an old
    /// sync directory — `discover_peers` must return exactly one entry for that
    /// device, keeping the larger file.
    ///
    /// The two fixtures use deliberately different sizes, with the *smaller*
    /// file's path sorting first alphabetically ("new-hostname/..." <
    /// "shared-device-id/..."). That makes the assertion exercise the
    /// largest-file tie-break in `select_remote_dbs_detailed` specifically —
    /// with two identically-sized fixtures the path-ordering tie-break alone
    /// would pick the same winner, so that variant would still pass even if
    /// the size comparison were silently dropped.
    #[test]
    fn daemon_deduplicates_same_device_in_both_layouts() {
        let sync_root = temp_dir();
        let device_id = "shared-device-id";

        // 3-level entry for the same device (e.g. after a hostname rename) —
        // smaller file; must lose despite its path sorting first.
        write_sized(
            &sync_root
                .join("new-hostname")
                .join(device_id)
                .join("sync.db"),
            8,
        );
        // 2-level legacy entry for the same device_id — the larger file,
        // must be the one selected.
        let legacy_path = sync_root.join(device_id).join("test.db");
        write_sized(&legacy_path, 64);

        let selection = discover(&sync_root, "local-device-id");

        assert_eq!(
            selection.selected.len(),
            1,
            "duplicate device_id across layouts must be collapsed to one; got selected: {:?}, skipped: {:?}",
            selection.selected,
            selection.skipped,
        );
        assert_eq!(
            selection.selected[0].device_id, device_id,
            "the surviving entry must be for the shared device_id"
        );
        assert_eq!(
            selection.selected[0].path, legacy_path,
            "the larger (legacy 2-level) file must survive, not the smaller 3-level one"
        );
        assert_eq!(
            selection.skipped.len(),
            1,
            "the other layout entry must be reported as skipped"
        );

        let _ = fs::remove_dir_all(&sync_root);
    }

    /// A leftover own-device root db (`{root}/{own_device_id}/test.db` — the
    /// #682 artefact every desktop that ran the old daemon still has, sometimes
    /// over 1 GB) must be excluded from discovery.  The 3-level peer alongside
    /// it must still be selected, and the own db must appear in neither
    /// `selected` nor `skipped`.
    #[test]
    fn daemon_excludes_own_device_root_db_alongside_three_level_peer() {
        let sync_root = temp_dir();
        let own_id = "aaaa-0000-own-device-id";

        // Old daemon's own staging db at the 2-level root path: {own_id}/test.db
        touch(&sync_root.join(own_id).join("test.db"));

        // A genuine 3-level Android peer that must survive discovery
        touch(
            &sync_root
                .join("poco_f8_ultra")
                .join("bbbb-1111-android-peer")
                .join("sync.db"),
        );

        let selection = discover(&sync_root, own_id);

        assert_eq!(
            selection.selected.len(),
            1,
            "own-device root db must not be selected; only the 3-level peer must be; got: {:?}",
            selection.selected
        );
        assert_eq!(selection.selected[0].device_id, "bbbb-1111-android-peer");

        let own_in_skipped = selection.skipped.iter().any(|s| s.db.device_id == own_id);
        assert!(
            !own_in_skipped,
            "own-device root db must not appear in skipped list either"
        );

        let _ = fs::remove_dir_all(&sync_root);
    }
}
