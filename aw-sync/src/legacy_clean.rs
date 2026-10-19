//! `aw-sync status --clean-legacy`: prune re-exported `-synced-from-` buckets
//! from this device's own staging databases (ActivityWatch/aw-server-rust#689
//! item 2).
//!
//! Before #648 the push path re-exported buckets that had themselves been
//! imported from a peer. Push no longer does that and pull skips them
//! (`is_synced_bucket`), so they are dead weight in a file that every peer
//! replicates. Only databases [`scan_sync_dir`] classifies as
//! [`SyncEntryKind::OwnStaging`] are ever opened for writing; a peer's file is
//! never touched, and nothing runs unless the user asks for it.
//!
//! Staging databases themselves are not deleted. The default daemon still
//! stages at `{device_id}/test.db` and `aw-sync sync` at
//! `{hostname}/{device_id}/test.db`, so either one may be the live copy.
//! Retiring them belongs with the v2 layout (ActivityWatch/activitywatch#1445).

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use aw_client_rust::blocking::AwClient;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::sync::is_synced_bucket_id;
use crate::util::{format_bytes, inspect_sync_db, scan_sync_dir, SyncEntryKind};

/// One own staging database and what cleanup would do to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneTarget {
    pub db_path: PathBuf,
    pub db_size: u64,
    /// `(bucket id, event count)`, in the order sqlite returned them.
    pub buckets: Vec<(String, i64)>,
    /// An earlier prune committed its deletes but its `VACUUM` did not run.
    pub vacuum_pending: bool,
    /// Bytes on sqlite's freelist that `VACUUM` would give back.
    pub reclaimable: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneOutcome {
    pub buckets: usize,
    pub events: usize,
    pub size_before: u64,
    pub size_after: u64,
}

/// `key_value` row a prune writes in the same transaction as its deletes and
/// removes once `VACUUM` succeeds. While it is set the db stays a target, so a
/// `VACUUM` that failed (e.g. a daemon pass held the lock) is retried by the
/// next run, without vacuuming dbs that cleanup never touched.
const VACUUM_PENDING_KEY: &str = "aw-sync.clean-legacy.vacuum-pending";

/// Own staging databases that hold `-synced-from-` buckets or have a `VACUUM`
/// pending from an earlier prune.
///
/// Returns the targets plus one message per own staging database that was
/// skipped (could not be inspected, or resolves through a symlink); those are
/// left alone and make the cleanup incomplete.
pub fn find_prune_targets(
    sync_dir: &Path,
    local_device_id: &str,
) -> io::Result<(Vec<PruneTarget>, Vec<String>)> {
    let mut targets = Vec::new();
    let mut errors = Vec::new();
    for entry in scan_sync_dir(sync_dir, Some(local_device_id))? {
        if entry.kind != SyncEntryKind::OwnStaging {
            continue;
        }
        let Some(db_path) = entry.db_path else {
            continue;
        };
        if let Err(e) = ensure_no_symlink(sync_dir, &db_path) {
            errors.push(e);
            continue;
        }
        let inspected = inspect_sync_db(&db_path).and_then(|info| {
            let pending = vacuum_pending(&db_path)?;
            Ok((info, pending, reclaimable_bytes(&db_path)?))
        });
        match inspected {
            Ok((info, vacuum_pending, reclaimable)) => {
                let buckets: Vec<(String, i64)> = info
                    .buckets
                    .into_iter()
                    .filter(|b| is_synced_bucket_id(&b.id))
                    .map(|b| (b.id, b.event_count))
                    .collect();
                let db_size = entry.db_size.unwrap_or(0);
                if !buckets.is_empty() || vacuum_pending {
                    targets.push(PruneTarget {
                        db_path,
                        db_size,
                        buckets,
                        vacuum_pending,
                        reclaimable,
                    });
                }
            }
            Err(e) => errors.push(e),
        }
    }
    Ok((targets, errors))
}

/// Refuse a path that leaves `sync_dir` through a symlink.
///
/// Own vs peer is decided from the path's `device_id` component, but sqlite
/// opens whatever the path resolves to. A symlinked `{own_id}/test.db` that
/// points at a peer's file would otherwise get that peer's data deleted.
fn ensure_no_symlink(sync_dir: &Path, db_path: &Path) -> Result<(), String> {
    let rel = db_path.strip_prefix(sync_dir).map_err(|_| {
        format!(
            "{} is outside the sync dir {}",
            db_path.display(),
            sync_dir.display()
        )
    })?;
    let canon = |p: &Path| fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    let expected = canon(sync_dir)?.join(rel);
    let real = canon(db_path)?;
    if real != expected {
        return Err(format!(
            "{} resolves to {} through a symlink; refusing to modify it",
            db_path.display(),
            real.display()
        ));
    }
    Ok(())
}

fn vacuum_pending(db_path: &Path) -> Result<bool, String> {
    let err = |e: rusqlite::Error| format!("{}: {e}", db_path.display());
    let conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    match conn
        .query_row(
            "SELECT 1 FROM key_value WHERE key = ?1",
            [VACUUM_PENDING_KEY],
            |_| Ok(()),
        )
        .optional()
    {
        Ok(found) => Ok(found.is_some()),
        // Pre-v4 schema without key_value: no prune has ever run on it.
        Err(e) if e.to_string().contains("no such table") => Ok(false),
        Err(e) => Err(err(e)),
    }
}

fn reclaimable_bytes(db_path: &Path) -> Result<u64, String> {
    let err = |e: rusqlite::Error| format!("{}: {e}", db_path.display());
    let conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let free: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .map_err(err)?;
    let page: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(err)?;
    Ok((free.max(0) as u64).saturating_mul(page.max(0) as u64))
}

/// Delete `bucket_ids` and their events from an own staging database, then
/// `VACUUM` so the file peers replicate actually shrinks.
///
/// Refuses any id without the `-synced-from-` marker, so a caller bug cannot
/// delete first-hand data, and any path that resolves through a symlink.
/// Ids that are already gone are skipped, and a `VACUUM` left pending by an
/// earlier run is retried, so a re-run after an interrupted prune finishes it.
pub fn prune_synced_buckets(
    sync_dir: &Path,
    db_path: &Path,
    bucket_ids: &[String],
) -> Result<PruneOutcome, String> {
    if let Some(id) = bucket_ids.iter().find(|id| !is_synced_bucket_id(id)) {
        return Err(format!("refusing to prune first-hand bucket '{id}'"));
    }
    ensure_no_symlink(sync_dir, db_path)?;
    let size_before = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let open_err = |e: rusqlite::Error| format!("{}: {e}", db_path.display());

    // No SQLITE_OPEN_CREATE: a staging db that vanished since the scan must
    // not be recreated empty inside the synced folder.
    let mut conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(open_err)?;
    // A daemon pass may be pushing into this file right now.
    conn.busy_timeout(Duration::from_secs(30))
        .map_err(open_err)?;

    let mut outcome = PruneOutcome {
        buckets: 0,
        events: 0,
        size_before,
        size_after: size_before,
    };
    let tx = conn.transaction().map_err(open_err)?;
    for id in bucket_ids {
        let bucketrow: Option<i64> = tx
            .query_row("SELECT id FROM buckets WHERE name = ?1", [id], |r| r.get(0))
            .optional()
            .map_err(open_err)?;
        let Some(bucketrow) = bucketrow else {
            continue;
        };
        outcome.events += tx
            .execute("DELETE FROM events WHERE bucketrow = ?1", [bucketrow])
            .map_err(open_err)?;
        tx.execute("DELETE FROM buckets WHERE id = ?1", [bucketrow])
            .map_err(open_err)?;
        outcome.buckets += 1;
    }
    if outcome.buckets > 0 {
        tx.execute(
            "INSERT OR REPLACE INTO key_value(key, value, last_modified) VALUES (?1, '1', ?2)",
            params![VACUUM_PENDING_KEY, chrono::Utc::now().timestamp()],
        )
        .map_err(open_err)?;
    }
    tx.commit().map_err(open_err)?;

    if outcome.buckets > 0 || vacuum_pending(db_path)? {
        conn.execute_batch("VACUUM").map_err(|e| {
            format!(
                "{}: VACUUM failed ({e}); deleted buckets stay deleted but the file will not \
                 shrink until it runs — re-run --clean-legacy (stop aw-sync first if it is busy)",
                db_path.display()
            )
        })?;
        conn.execute("DELETE FROM key_value WHERE key = ?1", [VACUUM_PENDING_KEY])
            .map_err(open_err)?;
    }
    // Closing the last connection checkpoints the WAL back into the main
    // file, so what the syncer copies next is self-contained.
    conn.close().map_err(|(_, e)| open_err(e))?;

    outcome.size_after = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    Ok(outcome)
}

pub fn run_clean_legacy(client: &AwClient, dry_run: bool) -> Result<(), Box<dyn Error>> {
    // Own vs peer is decided by device_id; guessing would risk writing to a
    // file another device owns.
    let device_id = client
        .get_info()
        .map_err(|e| {
            format!("--clean-legacy needs the local server to identify own staging dbs: {e}")
        })?
        .device_id;
    let sync_dir = crate::dirs::get_sync_dir()?;
    let mut stdout = io::stdout().lock();
    clean_legacy(&sync_dir, &device_id, dry_run, &mut stdout)
}

fn clean_legacy(
    sync_dir: &Path,
    device_id: &str,
    dry_run: bool,
    out: &mut dyn Write,
) -> Result<(), Box<dyn Error>> {
    writeln!(out)?;
    writeln!(out, "Legacy cleanup (ActivityWatch/aw-server-rust#689)")?;
    let (targets, skipped) = find_prune_targets(sync_dir, device_id)?;
    for e in &skipped {
        writeln!(out, "  ! skipped: {e}")?;
    }
    // A skipped own db may still hold re-exported buckets, so the run is
    // incomplete however the rest goes.
    let incomplete = || -> Box<dyn Error> {
        format!(
            "cleanup incomplete: {} own staging db(s) skipped",
            skipped.len()
        )
        .into()
    };

    if targets.is_empty() {
        writeln!(
            out,
            "  no other own staging db holds re-exported …-synced-from-… buckets"
        )?;
        return if skipped.is_empty() {
            Ok(())
        } else {
            Err(incomplete())
        };
    }

    let mut total_buckets = 0;
    let mut total_events = 0i64;
    for t in &targets {
        writeln!(
            out,
            "  {} (own staging, {})",
            t.db_path.display(),
            format_bytes(t.db_size)
        )?;
        for (id, events) in &t.buckets {
            writeln!(out, "    {id}  {events} events")?;
            total_events += events;
        }
        if t.buckets.is_empty() {
            writeln!(
                out,
                "    no re-exported buckets left, but {} is reclaimable by VACUUM (an earlier \
                 run did not finish)",
                format_bytes(t.reclaimable)
            )?;
        }
        total_buckets += t.buckets.len();
    }
    writeln!(
        out,
        "  {total_buckets} re-exported buckets, {total_events} events in {} db(s).",
        targets.len()
    )?;

    if dry_run {
        writeln!(
            out,
            "  dry run: nothing changed. Re-run without --dry-run to delete them. The \
             deletion replicates to every device sharing this folder; those devices already \
             ignore these buckets on pull."
        )?;
        return if skipped.is_empty() {
            Ok(())
        } else {
            Err(incomplete())
        };
    }

    let mut failed = 0;
    for t in &targets {
        let ids: Vec<String> = t.buckets.iter().map(|(id, _)| id.clone()).collect();
        match prune_synced_buckets(sync_dir, &t.db_path, &ids) {
            Ok(o) => writeln!(
                out,
                "  pruned {} buckets ({} events) from {}: {} → {}",
                o.buckets,
                o.events,
                t.db_path.display(),
                format_bytes(o.size_before),
                format_bytes(o.size_after)
            )?,
            Err(e) => {
                failed += 1;
                writeln!(out, "  ! {e}")?;
            }
        }
    }
    if failed > 0 {
        Err(format!(
            "{failed} staging db(s) could not be pruned, {} skipped",
            skipped.len()
        ))?;
    }
    if !skipped.is_empty() {
        return Err(incomplete());
    }
    Ok(())
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use crate::sync::create_datastore;
    use aw_models::{Bucket, Event};
    use chrono::{Duration as ChronoDuration, Utc};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    const LOCAL: &str = "d7bc68e7-aaaa-bbbb-cccc-dddddddddddd";
    const PEER: &str = "41662faa-aaaa-bbbb-cccc-dddddddddddd";

    fn temp_sync_dir() -> PathBuf {
        // Tests run in parallel; the counter keeps two same-nanosecond calls
        // from sharing (and clobbering) one directory.
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "aw-sync-clean-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn bucket(id: &str) -> Bucket {
        Bucket {
            bid: None,
            id: id.to_string(),
            _type: "currentwindow".to_string(),
            client: "test".to_string(),
            hostname: "host".to_string(),
            created: None,
            data: Default::default(),
            metadata: Default::default(),
            events: None,
            last_updated: None,
        }
    }

    /// A staging db with one bucket per id and `n` events in each.
    fn staging_db(path: &Path, bucket_ids: &[&str], n: i64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ds = create_datastore(path).unwrap();
        let start = Utc::now() - ChronoDuration::days(1);
        for id in bucket_ids {
            ds.create_bucket(&bucket(id)).unwrap();
            let events: Vec<Event> = (0..n)
                .map(|i| Event {
                    id: None,
                    timestamp: start + ChronoDuration::seconds(i),
                    duration: ChronoDuration::seconds(1),
                    data: Default::default(),
                })
                .collect();
            ds.insert_events(id, &events).unwrap();
        }
        ds.force_commit().unwrap();
        ds.close();
        let conn = Connection::open(path).unwrap();
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    fn bucket_ids(path: &Path) -> Vec<String> {
        let mut ids: Vec<String> = inspect_sync_db(path)
            .unwrap()
            .buckets
            .into_iter()
            .map(|b| b.id)
            .collect();
        ids.sort();
        ids
    }

    const OWN: &str = "aw-watcher-window_erb-m2";
    const REEXPORT: &str = "aw-watcher-window_erb-main3-synced-from-erb-main3";

    #[test]
    fn prunes_own_staging_in_both_layouts_and_never_touches_peers() {
        let root = temp_sync_dir();
        let own_two_level = root.join(LOCAL).join("test.db");
        let own_three_level = root.join("erb-m2").join(LOCAL).join("test.db");
        let peer = root.join("erb-main3").join(PEER).join("test.db");
        staging_db(&own_two_level, &[OWN, REEXPORT], 20);
        staging_db(&own_three_level, &[OWN, REEXPORT], 5);
        staging_db(&peer, &["aw-watcher-afk_x-synced-from-x"], 5);

        let (targets, errors) = find_prune_targets(&root, LOCAL).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let mut paths: Vec<_> = targets.iter().map(|t| t.db_path.clone()).collect();
        paths.sort();
        let mut expected = vec![own_two_level.clone(), own_three_level.clone()];
        expected.sort();
        assert_eq!(paths, expected, "peer db must never be a target");
        let two = targets.iter().find(|t| t.db_path == own_two_level).unwrap();
        assert_eq!(two.buckets, vec![(REEXPORT.to_string(), 20)]);

        let mut out = Vec::new();
        clean_legacy(&root, LOCAL, false, &mut out).unwrap();

        assert_eq!(bucket_ids(&own_two_level), vec![OWN.to_string()]);
        assert_eq!(bucket_ids(&own_three_level), vec![OWN.to_string()]);
        assert_eq!(
            bucket_ids(&peer),
            vec!["aw-watcher-afk_x-synced-from-x".to_string()]
        );
        let own_info = inspect_sync_db(&own_two_level).unwrap();
        assert_eq!(own_info.event_count, 20, "first-hand events survive");

        // Idempotent: a second run finds nothing.
        let (targets, _) = find_prune_targets(&root, LOCAL).unwrap();
        assert!(targets.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        staging_db(&own, &[OWN, REEXPORT], 3);
        let before = fs::read(&own).unwrap();

        let mut out = Vec::new();
        clean_legacy(&root, LOCAL, true, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();

        assert!(out.contains(REEXPORT), "{out}");
        assert!(out.contains("dry run"), "{out}");
        assert_eq!(fs::read(&own).unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_first_hand_bucket_ids() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        staging_db(&own, &[OWN], 1);

        let err = prune_synced_buckets(&root, &own, &[OWN.to_string()]).unwrap_err();
        assert!(err.contains("first-hand"), "{err}");
        assert_eq!(bucket_ids(&own), vec![OWN.to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_bucket_is_skipped_and_db_is_not_created() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        staging_db(&own, &[OWN], 1);

        let o = prune_synced_buckets(&root, &own, &[REEXPORT.to_string()]).unwrap();
        assert_eq!((o.buckets, o.events), (0, 0));

        let gone = root.join("nope").join("test.db");
        assert!(prune_synced_buckets(&root, &gone, &[REEXPORT.to_string()]).is_err());
        assert!(!gone.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupted_vacuum_is_retried_on_next_run() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        staging_db(&own, &[OWN, REEXPORT], 2000);
        // Simulate a prune whose deletes committed but whose VACUUM failed.
        {
            let conn = Connection::open(&own).unwrap();
            conn.execute_batch(&format!(
                "DELETE FROM events WHERE bucketrow = (SELECT id FROM buckets WHERE name = '{REEXPORT}');
                 DELETE FROM buckets WHERE name = '{REEXPORT}';
                 INSERT INTO key_value(key, value, last_modified) VALUES ('{VACUUM_PENDING_KEY}', '1', 0);"
            ))
            .unwrap();
        }
        assert!(reclaimable_bytes(&own).unwrap() > 0);

        let (targets, _) = find_prune_targets(&root, LOCAL).unwrap();
        assert_eq!(targets.len(), 1, "oversized db must stay a target");
        assert!(targets[0].buckets.is_empty());

        let mut out = Vec::new();
        clean_legacy(&root, LOCAL, false, &mut out).unwrap();
        assert_eq!(reclaimable_bytes(&own).unwrap(), 0);
        assert!(!vacuum_pending(&own).unwrap());
        let (targets, _) = find_prune_targets(&root, LOCAL).unwrap();
        assert!(targets.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn free_pages_without_pending_marker_are_left_alone() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        staging_db(&own, &[OWN], 2000);
        // Ordinary deletes (not by cleanup) leave a large freelist.
        {
            let conn = Connection::open(&own).unwrap();
            conn.execute_batch("DELETE FROM events").unwrap();
        }
        assert!(reclaimable_bytes(&own).unwrap() > 0);

        let (targets, skipped) = find_prune_targets(&root, LOCAL).unwrap();
        assert!(targets.is_empty() && skipped.is_empty(), "{targets:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn own_path_symlinked_to_peer_db_is_refused() {
        let root = temp_sync_dir();
        let peer = root.join("erb-main3").join(PEER).join("test.db");
        staging_db(&peer, &[REEXPORT], 3);
        let own = root.join(LOCAL).join("test.db");
        fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&peer, &own).unwrap();
        let before = fs::read(&peer).unwrap();

        let (targets, skipped) = find_prune_targets(&root, LOCAL).unwrap();
        assert!(targets.is_empty(), "{targets:?}");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("symlink"), "{skipped:?}");

        let mut out = Vec::new();
        assert!(clean_legacy(&root, LOCAL, false, &mut out).is_err());
        assert!(prune_synced_buckets(&root, &own, &[REEXPORT.to_string()]).is_err());
        assert_eq!(
            fs::read(&peer).unwrap(),
            before,
            "peer db must be untouched"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn uninspectable_own_db_makes_cleanup_fail() {
        let root = temp_sync_dir();
        let own = root.join(LOCAL).join("test.db");
        fs::create_dir_all(own.parent().unwrap()).unwrap();
        fs::write(&own, b"not a sqlite database").unwrap();

        let mut out = Vec::new();
        let err = clean_legacy(&root, LOCAL, true, &mut out).unwrap_err();
        assert!(err.to_string().contains("incomplete"), "{err}");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("skipped"), "{out}");
        let _ = fs::remove_dir_all(&root);
    }
}
