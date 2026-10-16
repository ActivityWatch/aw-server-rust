//! Startup-only recovery of histories preserved by directory migration.
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use aw_models::Event;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use crate::{DatastoreError, DatastoreInstance};

fn error(e: impl std::fmt::Debug) -> String {
    format!("Database recovery failed: {e:?}")
}

fn identity(event: &Event) -> Result<(chrono::DateTime<chrono::Utc>, i64, String), String> {
    let duration = event
        .duration
        .num_nanoseconds()
        .ok_or("duration out of range")?;
    let sorted: BTreeMap<_, _> = event.data.iter().collect();
    Ok((
        event.timestamp,
        duration,
        serde_json::to_string(&sorted).map_err(error)?,
    ))
}

/// SQLite's online backup includes committed WAL frames. Never copy just the
/// main file, and never use immutable=1 for a potentially WAL-backed source.
pub fn consistent_backup(source: &Connection, destination: &Path) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(error)?;
    let mut backup = Connection::open(destination).map_err(error)?;
    rusqlite::backup::Backup::new(source, &mut backup)
        .map_err(error)?
        .run_to_completion(128, std::time::Duration::from_millis(10), None)
        .map_err(error)?;
    let result: String = backup
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(error)?;
    if result != "ok" {
        return Err(format!("Backup quick_check failed: {result}"));
    }
    drop(backup);
    std::fs::OpenOptions::new()
        .write(true)
        .open(destination)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    #[cfg(unix)]
    std::fs::File::open(destination.parent().ok_or("backup has no parent")?)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryCounts {
    pub buckets_created: usize,
    pub events_inserted: usize,
    pub duplicates_skipped: usize,
}

/// Union one preserved Rust datastore into the selected database before any
/// workers start. Completion and all inserted events commit in one transaction.
/// Original source files are read-only and remain available for manual recovery.
pub fn recover_database(
    target: &Path,
    source: &Path,
    backup: &Path,
    recovery_id: &str,
) -> Result<RecoveryCounts, String> {
    let mut conn =
        Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(error)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(error)?;
    let key = format!("windows_recovery:{recovery_id}");
    let completed = |c: &Connection| -> Result<bool, String> {
        // Versions before v4 have no key_value table (and hence cannot
        // contain a completion marker). Do not migrate just to probe it:
        // the rollback snapshot must preserve the pre-upgrade schema.
        let has_key_value: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='key_value')",
                [],
                |r| r.get(0),
            )
            .map_err(error)?;
        if !has_key_value {
            return Ok(false);
        }
        c.query_row("SELECT value FROM key_value WHERE key=?1", [&key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map(|v| v.is_some())
        .map_err(error)
    };
    if completed(&conn)? {
        return Ok(RecoveryCounts::default());
    }
    // A unique backup per attempt means an interrupted backup is never reused.
    consistent_backup(&conn, backup)?;
    let source_conn =
        Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(error)?;
    source_conn
        .busy_timeout(std::time::Duration::from_secs(30))
        .map_err(error)?;
    // A private temporary SQLite database is deleted when this connection
    // closes. Upgrade this online snapshot, never the preserved original.
    // An empty filename uses SQLite's temporary-file storage, so a large
    // history need not fit in RAM. Online backup includes committed WAL.
    let mut source_snapshot = Connection::open("").map_err(error)?;
    rusqlite::backup::Backup::new(&source_conn, &mut source_snapshot)
        .map_err(error)?
        .run_to_completion(128, std::time::Duration::from_millis(10), None)
        .map_err(error)?;
    drop(source_conn);
    // An uninspectable source (invalid bucket JSON, unsupported schema) is a
    // content problem in the preserved original, not an operational failure:
    // defer it for manual recovery instead of failing startup, even when the
    // selected database is healthy. Operational write failures below still
    // propagate. `database_rank` cannot catch this because it only inspects
    // SQLite structure and event timestamps, not bucket JSON.
    let mut source_ds = match DatastoreInstance::new(&source_snapshot, true) {
        Ok(ds) => ds,
        Err(e) => {
            warn!(
                "History recovery source {source:?} is uninspectable ({e:?}); leaving it untouched for manual recovery"
            );
            return Ok(RecoveryCounts::default());
        }
    };
    let source_tx = source_snapshot.unchecked_transaction().map_err(error)?;
    // Index migrations use their own BEGIN/COMMIT. Run them only after the
    // rollback backup, and before starting our atomic union transaction.
    DatastoreInstance::new(&conn, true).map_err(error)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    if completed(&tx)? {
        return Ok(RecoveryCounts::default());
    }
    // An old server can still be writing this file. Build the bucket cache
    // only under the write lock so a concurrent delete/recreate cannot leave
    // stale bucket row IDs and orphan recovered events.
    let mut ds = DatastoreInstance::new(&tx, false).map_err(error)?;
    let mut counts = RecoveryCounts::default();
    for (name, mut bucket) in source_ds.get_buckets() {
        if ds.get_bucket(&name).is_err() {
            bucket.bid = None;
            bucket.events = None;
            ds.create_bucket(&tx, bucket).map_err(error)?;
            counts.buckets_created += 1;
        }
        let existing = ds
            .get_events_unclipped(&tx, &name, None, None, None)
            .map_err(error)?;
        let mut seen: HashSet<_> = existing.iter().map(identity).collect::<Result<_, _>>()?;
        // Unreadable event content in the preserved original is a content
        // problem, not an operational failure: defer the whole source for
        // manual recovery instead of crashing startup, even when the selected
        // database is healthy. Returning before the transaction commits leaves
        // no completion marker, so a truncated union is never claimed as done
        // and the source stays available for manual recovery. `database_rank`
        // inspects only SQLite structure and timestamps, and the constructor
        // guard above only catches bucket-read failures, so this is the one
        // place corrupt event rows are detected. Operational failures (backup,
        // target writes, commit) still propagate.
        let events = match source_ds
            .get_events_unclipped_strict(&source_tx, &name, None, None, None)
        {
            Ok(events) => events,
            Err(DatastoreError::CorruptEvent(e)) => {
                warn!(
                    "History recovery source {source:?} has unreadable event content ({e}); leaving it untouched for manual recovery"
                );
                return Ok(RecoveryCounts::default());
            }
            Err(e) => return Err(error(e)),
        };
        let mut insert = Vec::new();
        for mut event in events {
            if seen.insert(identity(&event)?) {
                event.id = None;
                insert.push(event);
            } else {
                counts.duplicates_skipped += 1;
            }
        }
        counts.events_inserted += insert.len();
        ds.insert_events(&tx, &name, insert).map_err(error)?;
    }
    tx.execute(
        "INSERT INTO key_value(key, value, last_modified) VALUES (?1, 'done', ?2)",
        rusqlite::params![key, chrono::Utc::now().timestamp()],
    )
    .map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatastoreInstance;
    use aw_models::{Bucket, Event};
    use chrono::{Duration, TimeZone, Utc};
    use rusqlite::Connection;

    fn event(data: &str) -> Event {
        Event {
            id: Some(1),
            timestamp: Utc.timestamp_opt(1700000000, 123).unwrap(),
            duration: Duration::nanoseconds(1234567890),
            data: serde_json::from_str(data).unwrap(),
        }
    }

    fn populate(path: &Path, rows: &[(&str, Vec<Event>)]) {
        let conn = Connection::open(path).unwrap();
        let mut ds = DatastoreInstance::new(&conn, true).unwrap();
        for (name, events) in rows {
            let bucket = Bucket {
                id: (*name).to_owned(),
                device_id: "local".to_owned(),
                _type: "test".to_owned(),
                client: "test".to_owned(),
                hostname: "test".to_owned(),
                bid: None,
                created: None,
                data: Default::default(),
                metadata: Default::default(),
                events: None,
                last_updated: None,
            };
            ds.create_bucket(&conn, bucket).unwrap();
            let events: Vec<_> = events
                .iter()
                .cloned()
                .map(|mut e| {
                    e.id = None;
                    e
                })
                .collect();
            ds.insert_events(&conn, name, events).unwrap();
        }
    }

    fn count(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn durable_python_request_runs_actual_import_once_after_restart() {
        use crate::{Datastore, LegacyImportOptions};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let legacy = root.path().join("legacy.db");
        populate(&target, &[]);
        {
            let conn = Connection::open(&target).unwrap();
            conn.execute("INSERT INTO key_value(key, value, last_modified) VALUES ('windows_python_recovery', 'pending', 0)", []).unwrap();
        } // simulated process death after migration, before opening the worker
        let python = Connection::open(&legacy).unwrap();
        python.execute_batch("CREATE TABLE bucketmodel (key INTEGER PRIMARY KEY, id TEXT, type TEXT, client TEXT, hostname TEXT, created TEXT);
            CREATE TABLE eventmodel (id INTEGER PRIMARY KEY, bucket_id INTEGER, timestamp TEXT, duration REAL, datastr TEXT);
            INSERT INTO bucketmodel VALUES (1, 'python', 'test', 'test', 'test', '2026-01-01T00:00:00+00:00');
            INSERT INTO eventmodel VALUES (1, 1, '2026-01-01 10:00:00+00:00', 5.0, '{}');").unwrap();
        let opts = LegacyImportOptions {
            enabled: true,
            force: false,
            db_path_override: Some(legacy.to_str().unwrap().into()),
        };
        {
            let ds = Datastore::new_with_legacy_import_opts(
                target.to_str().unwrap().into(),
                opts.clone(),
            );
            assert_eq!(ds.get_event_count("python", None, None).unwrap(), 1);
            assert_eq!(ds.get_key_value("windows_python_recovery").unwrap(), "done");
        }
        python
            .execute_batch(
                "INSERT INTO eventmodel VALUES (2, 1, '2026-01-02 10:00:00+00:00', 5.0, '{}');",
            )
            .unwrap();
        let ds = Datastore::new_with_legacy_import_opts(target.to_str().unwrap().into(), opts);
        assert_eq!(
            ds.get_event_count("python", None, None).unwrap(),
            1,
            "completed migration must not import Python again"
        );
    }

    #[test]
    fn failed_recovery_marker_write_keeps_request_pending_and_restart_retries() {
        use crate::{Datastore, LegacyImportOptions};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let legacy = root.path().join("legacy.db");
        populate(&target, &[]);
        {
            let conn = Connection::open(&target).unwrap();
            conn.execute("INSERT INTO key_value(key, value, last_modified) VALUES ('windows_python_recovery', 'pending', 0)", []).unwrap();
            // The import succeeds but writing the completion marker fails
            // (I/O error, busy database, ...). The worker must survive it.
            conn.execute_batch("CREATE TRIGGER fail_marker BEFORE INSERT ON key_value WHEN NEW.key = 'windows_python_recovery' AND NEW.value = 'done' BEGIN SELECT RAISE(ABORT, 'simulated marker write failure'); END;").unwrap();
        }
        let python = Connection::open(&legacy).unwrap();
        python.execute_batch("CREATE TABLE bucketmodel (key INTEGER PRIMARY KEY, id TEXT, type TEXT, client TEXT, hostname TEXT, created TEXT);
            CREATE TABLE eventmodel (id INTEGER PRIMARY KEY, bucket_id INTEGER, timestamp TEXT, duration REAL, datastr TEXT);
            INSERT INTO bucketmodel VALUES (1, 'python', 'test', 'test', 'test', '2026-01-01T00:00:00+00:00');
            INSERT INTO eventmodel VALUES (1, 1, '2026-01-01 10:00:00+00:00', 5.0, '{}');").unwrap();
        let opts = LegacyImportOptions {
            enabled: true,
            force: false,
            db_path_override: Some(legacy.to_str().unwrap().into()),
        };
        {
            let ds = Datastore::new_with_legacy_import_opts(
                target.to_str().unwrap().into(),
                opts.clone(),
            );
            // The datastore stays usable, the import was rolled back with the
            // marker, and the durable request is still pending for a retry.
            assert_eq!(
                ds.get_key_value("windows_python_recovery").unwrap(),
                "pending"
            );
            assert!(
                ds.get_event_count("python", None, None).is_err(),
                "rolled-back import must not leave a phantom bucket"
            );
        }
        Connection::open(&target)
            .unwrap()
            .execute_batch("DROP TRIGGER fail_marker")
            .unwrap();
        let ds = Datastore::new_with_legacy_import_opts(target.to_str().unwrap().into(), opts);
        assert_eq!(ds.get_event_count("python", None, None).unwrap(), 1);
        assert_eq!(ds.get_key_value("windows_python_recovery").unwrap(), "done");
    }

    #[test]
    fn failed_insert_rolls_back_and_restart_retries() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        populate(&target, &[("shared", vec![event(r#"{"a":1}"#)])]);
        populate(
            &source,
            &[("shared", vec![event(r#"{"a":2}"#), event(r#"{"a":3}"#)])],
        );
        let conn = Connection::open(&target).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_recovery BEFORE INSERT ON events WHEN NEW.data LIKE '%3%' BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;").unwrap();
        assert!(recover_database(
            &target,
            &source,
            &root.path().join("failed.db"),
            "migration-2"
        )
        .is_err());
        assert_eq!(count(&target), 1);
        conn.execute_batch("DROP TRIGGER fail_recovery").unwrap();
        let result = recover_database(
            &target,
            &source,
            &root.path().join("retry.db"),
            "migration-2",
        )
        .unwrap();
        assert_eq!(result.events_inserted, 2);
        assert_eq!(count(&target), 3);
    }

    #[test]
    fn backup_and_union_include_uncheckpointed_wal() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        populate(&target, &[("shared", vec![event(r#"{"a":1}"#)])]);
        populate(&source, &[("shared", vec![])]);
        let writer = Connection::open(&source).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        let mut source_ds = DatastoreInstance::new(&writer, false).unwrap();
        let mut e = event(r#"{"wal":true}"#);
        e.id = None;
        source_ds.insert_events(&writer, "shared", vec![e]).unwrap();
        let target_writer = Connection::open(&target).unwrap();
        target_writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; UPDATE events SET data='{\"a\":2}';").unwrap();
        let backup = root.path().join("backup.db");
        assert_eq!(
            recover_database(&target, &source, &backup, "wal")
                .unwrap()
                .events_inserted,
            1
        );
        assert_eq!(count(&target), 2);
        let data: String = Connection::open(&backup)
            .unwrap()
            .query_row("SELECT data FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(data, r#"{"a":2}"#);
        assert_eq!(count(&source), 1);
    }

    // v3 has the current bucket/event columns, but no key_value table and
    // only the three original single-column event indexes.
    fn downgrade_to_v3(conn: &Connection) {
        conn.execute_batch(
            "DROP TABLE key_value;
             DROP INDEX events_bucketrow_starttime_endtime_index;
             DROP INDEX events_bucketrow_endtime_starttime_index;
             CREATE INDEX events_bucketrow_index ON events(bucketrow);
             CREATE INDEX events_starttime_index ON events(starttime);
             CREATE INDEX events_endtime_index ON events(endtime);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    fn version(conn: &Connection) -> i32 {
        conn.pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn v3_histories_recover_without_upgrading_source_or_rollback_backup() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        let backup = root.path().join("backup.db");
        populate(&target, &[("shared", vec![event(r#"{"target":true}"#)])]);
        populate(&source, &[("shared", vec![event(r#"{"source":true}"#)])]);
        let target_writer = Connection::open(&target).unwrap();
        let source_writer = Connection::open(&source).unwrap();
        downgrade_to_v3(&target_writer);
        downgrade_to_v3(&source_writer);
        // Data updates are committed only in WAL. Keeping these writers
        // open prevents a close-time checkpoint from hiding
        // accidental main-file-only copies.
        for conn in [&target_writer, &source_writer] {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
        }
        target_writer
            .execute("UPDATE events SET data=?1", [r#"{"target_wal":true}"#])
            .unwrap();
        source_writer
            .execute("UPDATE events SET data=?1", [r#"{"source_wal":true}"#])
            .unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        let source_wal = source.with_extension("db-wal");
        let source_wal_bytes = std::fs::read(&source_wal).unwrap();

        let counts = recover_database(&target, &source, &backup, "v3").unwrap();
        assert_eq!(counts.events_inserted, 1);
        assert_eq!(count(&target), 2);
        assert_eq!(version(&target_writer), crate::datastore::NEWEST_DB_VERSION);
        let values: Vec<String> = target_writer
            .prepare("SELECT data FROM events ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            values,
            vec![r#"{"target_wal":true}"#, r#"{"source_wal":true}"#]
        );
        assert_eq!(version(&source_writer), 3);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&source_wal).unwrap(), source_wal_bytes);
        let rollback = Connection::open(&backup).unwrap();
        assert_eq!(version(&rollback), 3);
        assert_eq!(count(&backup), 1);
        let data: String = rollback
            .query_row("SELECT data FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(data, r#"{"target_wal":true}"#);
        assert!(rollback.prepare("SELECT * FROM key_value").is_err());
        assert_eq!(
            recover_database(&target, &source, &backup, "v3").unwrap(),
            RecoveryCounts::default()
        );
    }

    /// Recovery must copy every stored row, including events stamped before
    /// 1970 (whole and fractional seconds), not just the API's default range.
    #[test]
    fn union_includes_pre_epoch_events() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        let at = |ns: i64, data: &str| Event {
            id: None,
            timestamp: Utc.timestamp_nanos(ns),
            duration: Duration::seconds(1),
            data: serde_json::from_str(data).unwrap(),
        };
        populate(&target, &[("shared", vec![event(r#"{"a":1}"#)])]);
        populate(
            &source,
            &[(
                "shared",
                vec![
                    at(-2_000_000_000, r#"{"pre":"whole"}"#),
                    at(-1_500_000_000, r#"{"pre":"fraction"}"#),
                    at(-10_000_000_000, r#"{"pre":"ended"}"#),
                ],
            )],
        );
        let result = recover_database(
            &target,
            &source,
            &root.path().join("backup.db"),
            "migration-pre-epoch",
        )
        .unwrap();
        assert_eq!(result.events_inserted, 3);
        assert_eq!(count(&target), 4);
        let conn = Connection::open(&target).unwrap();
        let mut ds = DatastoreInstance::new(&conn, false).unwrap();
        let mut starts: Vec<_> = ds
            .get_events_unclipped(&conn, "shared", None, None, None)
            .unwrap()
            .into_iter()
            .map(|e| e.timestamp.timestamp_nanos_opt().unwrap())
            .collect();
        starts.sort();
        assert_eq!(
            starts,
            vec![
                -10_000_000_000,
                -2_000_000_000,
                -1_500_000_000,
                1_700_000_000_000_000_123
            ]
        );
        assert_eq!(ds.get_event_count(&conn, "shared", None, None).unwrap(), 4);
    }

    #[test]
    fn union_preserves_id_collisions_dedups_per_bucket_and_replays() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        let backup = root.path().join("backup.db");
        populate(&target, &[("shared", vec![event(r#"{"a":1,"b":2}"#)])]);
        populate(
            &source,
            &[
                (
                    "shared",
                    vec![
                        event(r#"{"b":2,"a":1}"#),
                        event(r#"{"a":3}"#),
                        event(r#"{"a":3}"#),
                    ],
                ),
                ("new", vec![event(r#"{"a":1,"b":2}"#)]),
            ],
        );
        let result = recover_database(&target, &source, &backup, "migration-1").unwrap();
        assert_eq!(result.events_inserted, 2);
        assert_eq!(result.duplicates_skipped, 2);
        assert_eq!(result.buckets_created, 1);
        assert_eq!(count(&target), 3);
        assert_eq!(count(&backup), 1);
        assert_eq!(count(&source), 4);
        assert_eq!(
            recover_database(&target, &source, &backup, "migration-1").unwrap(),
            RecoveryCounts::default()
        );
        assert_eq!(count(&target), 3);
    }

    #[test]
    fn corrupt_source_events_are_deferred_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        let backup = root.path().join("backup.db");
        populate(&target, &[("healthy", vec![event(r#"{"a":1}"#)])]);
        populate(&source, &[("shared", vec![event(r#"{"a":1}"#)])]);
        // One event row is unreadable. A tolerant reader would return the
        // readable subset and commit `done`, silently truncating history; the
        // strict recovery reader must not claim success. But corrupt content
        // in the preserved archive must not crash startup either, even though
        // the selected database is healthy: defer the source for manual
        // recovery and leave no completion marker so it is retried.
        Connection::open(&source)
            .unwrap()
            .execute("UPDATE events SET data='not-json'", [])
            .unwrap();
        let result = recover_database(&target, &source, &backup, "migration-corrupt").unwrap();
        assert_eq!(result, RecoveryCounts::default());
        // Rolled back: nothing inserted and no completion marker.
        assert_eq!(count(&target), 1);
        let conn = Connection::open(&target).unwrap();
        let done: i64 = conn
            .query_row(
                "SELECT count(*) FROM key_value WHERE key=?1",
                ["windows_recovery:migration-corrupt"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done, 0);
    }

    #[test]
    fn uninspectable_source_is_deferred_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.db");
        let source = root.path().join("source.db");
        let backup = root.path().join("backup.db");
        populate(&target, &[]);
        populate(&source, &[("shared", vec![event(r#"{"a":1}"#)])]);
        // Invalid bucket JSON passes database_rank (structure/timestamps only)
        // but makes the preserved source uninspectable.
        Connection::open(&source)
            .unwrap()
            .execute("UPDATE buckets SET data='not-json'", [])
            .unwrap();
        let result =
            recover_database(&target, &source, &backup, "migration-uninspectable").unwrap();
        assert_eq!(result, RecoveryCounts::default());
        // The healthy target is untouched and startup is not blocked.
        assert_eq!(count(&target), 0);
    }
}
