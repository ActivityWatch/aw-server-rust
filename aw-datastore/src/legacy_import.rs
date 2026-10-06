use crate::datastore::DatastoreInstance;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub enum LegacyDatastoreImportError {
    #[allow(dead_code)]
    SQLPrepareError(String),
    #[allow(dead_code)]
    SQLMapError(String),
}

/// Imports a legacy (Python aw-server) database into `new_ds`, if one is
/// found. Returns `Ok(true)` if an import was performed (even if it merged
/// zero new events into an already-imported bucket), `Ok(false)` if no
/// legacy database was found to import.
///
/// `legacy_db_path_override` overrides the default
/// `peewee-sqlite.v2.db` lookup location (see `--legacy-dbpath` in aw-server).
pub fn legacy_import(
    new_ds: &mut DatastoreInstance,
    new_conn: &Connection,
    legacy_db_path_override: Option<&str>,
) -> Result<bool, LegacyDatastoreImportError> {
    import::legacy_import(new_ds, new_conn, legacy_db_path_override)
}

#[cfg(not(target_os = "android"))]
mod import {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use rusqlite::types::ToSql;
    use rusqlite::Connection;

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;

    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;

    use crate::datastore::DatastoreInstance;
    use crate::DatastoreError;

    use super::LegacyDatastoreImportError;

    fn dbfile_path(override_path: Option<&str>) -> PathBuf {
        match override_path {
            Some(p) => PathBuf::from(p),
            None => default_dbfile_path(),
        }
    }

    /// Where python aw-server keeps its database: platformdirs
    /// `user_data_dir("activitywatch")`. On Windows that is
    /// `%LOCALAPPDATA%\\activitywatch\\activitywatch` (appauthor defaults to
    /// appname), not Roaming `%APPDATA%` (what `dirs::data_dir()` returns).
    fn default_dbfile_path() -> PathBuf {
        #[cfg(windows)]
        let root = dirs::data_local_dir()
            .expect("Unable to read user data dir")
            .join("activitywatch")
            .join("activitywatch");
        #[cfg(not(windows))]
        let root = dirs::data_dir()
            .expect("Unable to read user data dir")
            .join("activitywatch");
        root.join("aw-server").join("peewee-sqlite.v2.db")
    }

    /// Dedup identity tuple for an event: (timestamp, duration_ns, canonical
    /// data JSON). Mirrors the identity used by the manual bucket-import
    /// endpoint (`aw-server/src/endpoints/import.rs`), so a re-run of the
    /// legacy import is idempotent instead of panicking on
    /// `BucketAlreadyExists` or duplicating events.
    fn event_identity(event: &Event) -> Option<(DateTime<Utc>, i64, String)> {
        let duration_ns = event.duration.num_nanoseconds()?;
        let sorted: std::collections::BTreeMap<_, _> = event.data.iter().collect();
        let data_json = serde_json::to_string(&sorted).ok()?;
        Some((event.timestamp, duration_ns, data_json))
    }

    /// Filters out events that are already present in `bucket_id`, matched
    /// by `event_identity`. Only queries the time range the candidate events
    /// span, so this stays cheap on a bucket with a long history.
    fn dedup_against_existing(
        ds: &mut DatastoreInstance,
        conn: &Connection,
        bucket_id: &str,
        events: Vec<Event>,
    ) -> Result<Vec<Event>, LegacyDatastoreImportError> {
        if events.is_empty() {
            return Ok(events);
        }
        let start = events.iter().map(|e| e.timestamp).min().unwrap();
        let end = events.iter().map(|e| e.calculate_endtime()).max().unwrap();
        let existing = ds
            .get_events_unclipped(conn, bucket_id, Some(start), Some(end), None)
            .map_err(|err| {
                LegacyDatastoreImportError::SQLMapError(format!(
                    "Failed to fetch existing events for dedup in '{bucket_id}': {err:?}"
                ))
            })?;
        let existing_identities: HashSet<_> = existing.iter().filter_map(event_identity).collect();
        Ok(events
            .into_iter()
            .filter(|e| match event_identity(e) {
                Some(identity) => !existing_identities.contains(&identity),
                // Can't compute an identity (e.g. non-representable duration) — keep
                // it rather than silently drop a legacy event.
                None => true,
            })
            .collect())
    }

    fn get_legacy_buckets(conn: &Connection) -> Result<Vec<Bucket>, LegacyDatastoreImportError> {
        let mut stmt = match conn
            .prepare("SELECT key, id, type, client, hostname, created FROM bucketmodel")
        {
            Ok(stmt) => stmt,
            Err(err) => return Err(LegacyDatastoreImportError::SQLPrepareError(err.to_string())),
        };
        let bucket_rows = match stmt.query_map(&[] as &[&dyn ToSql], |row| {
            Ok(Bucket {
                bid: row.get(0)?,
                id: row.get(1)?,
                _type: row.get(2)?,
                client: row.get(3)?,
                hostname: row.get(4)?,
                device_id: "local".to_string(),
                created: row.get(5)?,
                data: json_map! {},
                events: None,
                last_updated: None,
                metadata: BucketMetadata {
                    start: None,
                    end: None,
                },
            })
        }) {
            Ok(buckets) => buckets,
            Err(err) => {
                return Err(LegacyDatastoreImportError::SQLMapError(format!(
                    "Failed to query get_legacy_buckets SQL statement: {err:?}"
                )))
            }
        };

        let mut buckets = Vec::new();
        for bucket_res in bucket_rows {
            match bucket_res {
                Ok(bucket) => buckets.push(bucket),
                Err(err) => panic!("{err:?}"),
            }
        }
        Ok(buckets)
    }

    fn get_legacy_events(
        conn: &Connection,
        bucket_id: i64,
    ) -> Result<Vec<Event>, LegacyDatastoreImportError> {
        let mut stmt = match conn.prepare(
            "
                SELECT timestamp, duration, datastr
                FROM eventmodel
                WHERE bucket_id = ?1
                ORDER BY timestamp DESC
            ;",
        ) {
            Ok(stmt) => stmt,
            Err(err) => return Err(LegacyDatastoreImportError::SQLPrepareError(err.to_string())),
        };

        let rows = match stmt.query_map([&bucket_id], |row| {
            let timestamp_str: String = row.get(0)?;
            let duration_float: f64 = row.get(1)?;
            let data_str: String = row.get(2)?;
            Ok((timestamp_str, duration_float, data_str))
        }) {
            Ok(rows) => rows,
            Err(err) => {
                return Err(LegacyDatastoreImportError::SQLMapError(format!(
                    "Failed to query get_legacy_events SQL statement: {err:?}"
                )))
            }
        };
        let mut list = Vec::new();
        for row in rows {
            match row {
                Ok((timestamp_str, duration_float, data_str)) => {
                    let timestamp_str = timestamp_str.replace(' ', "T");
                    let timestamp = match DateTime::parse_from_rfc3339(&timestamp_str) {
                        Ok(timestamp) => timestamp.with_timezone(&Utc),
                        Err(err) => panic!("Timestamp string {timestamp_str}: {err:?}"),
                    };

                    let duration_ns = match aw_models::seconds_to_nanos(duration_float) {
                        Some(ns) => ns,
                        None => {
                            warn!(
                                "Skipping event with invalid duration {} in bucket {}",
                                duration_float, bucket_id
                            );
                            continue;
                        }
                    };

                    let data: serde_json::map::Map<String, serde_json::Value> =
                        match serde_json::from_str(&data_str) {
                            Ok(data) => data,
                            Err(err) => {
                                warn!(
                                    "Unable to parse JSON data in event from bucket {}\n{}\n{}",
                                    bucket_id, err, data_str
                                );
                                continue;
                            }
                        };

                    let event = Event {
                        id: None,
                        timestamp,
                        duration: Duration::nanoseconds(duration_ns),
                        data,
                    };
                    list.push(event)
                }
                Err(err) => panic!("Corrupt event in bucket {bucket_id}: {err}"),
            };
        }
        Ok(list)
    }

    pub fn legacy_import(
        new_ds: &mut DatastoreInstance,
        new_conn: &Connection,
        legacy_db_path_override: Option<&str>,
    ) -> Result<bool, LegacyDatastoreImportError> {
        let legacy_db_path = dbfile_path(legacy_db_path_override);
        info!(
            "Checking for legacy (Python aw-server) database at {}",
            legacy_db_path.display()
        );
        if !legacy_db_path.exists() {
            info!(
                "No legacy database found at {}, skipping import",
                legacy_db_path.display()
            );
            return Ok(false);
        }
        info!(
            "Found legacy database at {}, importing",
            legacy_db_path.display()
        );
        let legacy_conn = Connection::open(&legacy_db_path).unwrap_or_else(|err| {
            panic!(
                "Unable to open legacy db file at {}: {err}",
                legacy_db_path.display()
            )
        });

        let buckets = get_legacy_buckets(&legacy_conn)?;
        info!("Legacy database has {} bucket(s) to import", buckets.len());
        for bucket in &buckets {
            let events = get_legacy_events(&legacy_conn, bucket.bid.unwrap())?;
            let num_events = events.len(); // Save len before lending events to insert_events
            match new_ds.create_bucket(new_conn, bucket.clone()) {
                Ok(_) => {
                    info!(
                        "Imported legacy bucket '{}' ({} events)",
                        bucket.id, num_events
                    );
                    if let Err(err) = new_ds.insert_events(new_conn, &bucket.id, events) {
                        panic!(
                            "Failed to insert events to bucket '{}': {:?}",
                            bucket.id, err
                        );
                    }
                }
                Err(DatastoreError::BucketAlreadyExists(_)) => {
                    // Idempotent re-import (e.g. `aw-server --import-legacy` run
                    // again): the bucket already exists from a prior import, so
                    // merge only events not already present instead of panicking.
                    let new_events = dedup_against_existing(new_ds, new_conn, &bucket.id, events)?;
                    let merged = new_events.len();
                    if !new_events.is_empty() {
                        if let Err(err) = new_ds.insert_events(new_conn, &bucket.id, new_events) {
                            panic!(
                                "Failed to merge legacy events into bucket '{}': {:?}",
                                bucket.id, err
                            );
                        }
                    }
                    info!(
                        "Bucket '{}' already existed; merged {} new event(s), skipped {} already-present event(s)",
                        bucket.id, merged, num_events - merged
                    );
                }
                Err(err) => panic!("Failed to create bucket '{}': {:?}", bucket.id, err),
            };
        }
        Ok(true)
    }

    /// Pins where python aw-server's database is looked for. Derived from the
    /// environment, not the `dirs` crate, so a dependency change that moves
    /// it (as #562 did on Windows) fails here instead of silently skipping
    /// the import for every migrating user. Must equal the Python side's
    /// pin: aw-core `tests/test_dirs_pinned.py` `test_peewee_db_path_is_pinned`
    /// (see also <https://docs.activitywatch.net/en/latest/directories.html>).
    #[test]
    fn test_legacy_dbfile_path_is_pinned() {
        #[cfg(windows)]
        let expected = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
            .join("activitywatch")
            .join("activitywatch");
        #[cfg(target_os = "macos")]
        let expected = PathBuf::from(std::env::var("HOME").unwrap())
            .join("Library/Application Support/activitywatch");
        #[cfg(target_os = "linux")]
        let expected = std::env::var("XDG_DATA_HOME")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share"))
            .join("activitywatch");
        assert_eq!(
            dbfile_path(None),
            expected.join("aw-server").join("peewee-sqlite.v2.db")
        );
    }

    /* This test is disabled because it requires manual set-up of a old aw-server database
     * Can be run with:
     * cargo test --features legacy_import,legacy_import_tests */
    #[test]
    #[cfg_attr(not(feature = "legacy_import_tests"), ignore)]
    fn test_legacy_import() {
        assert!(dbfile_path(None).exists());
        let mut new_conn =
            Connection::open_in_memory().expect("Unable to open corrupt legacy db file");
        let mut ds = DatastoreInstance::new(&mut new_conn, true).unwrap();
        assert!(
            ds.ensure_legacy_import(&new_conn, None, false).unwrap(),
            "Failed to ensure legacy import"
        );
        let buckets = ds.get_buckets();
        assert!(!buckets.is_empty());
        let mut num_events = 0;
        for (bucket_id, _bucket) in buckets {
            let events = ds
                .get_events(&new_conn, &bucket_id, None, None, Some(1000))
                .unwrap();
            num_events += events.len();
        }
        assert!(num_events > 0);
    }

    #[cfg(test)]
    mod fixture_tests {
        use super::*;

        /// Creates a legacy aw-server (Python/peewee) sqlite db at `path`
        /// with the `bucketmodel`/`eventmodel` schema `get_legacy_buckets`
        /// and `get_legacy_events` expect, plus one bucket and the given
        /// events.
        fn write_fixture_legacy_db(path: &std::path::Path, events: &[(&str, f64, &str)]) {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE bucketmodel (
                    key INTEGER PRIMARY KEY,
                    id TEXT,
                    type TEXT,
                    client TEXT,
                    hostname TEXT,
                    created TEXT
                );
                CREATE TABLE eventmodel (
                    id INTEGER PRIMARY KEY,
                    bucket_id INTEGER,
                    timestamp TEXT,
                    duration REAL,
                    datastr TEXT
                );
                ",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO bucketmodel (id, type, client, hostname, created) \
                 VALUES ('aw-watcher-afk_testhost', 'afkstatus', 'aw-watcher-afk', 'testhost', '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
            let bucket_key = conn.last_insert_rowid();
            for (timestamp, duration, data) in events {
                conn.execute(
                    "INSERT INTO eventmodel (bucket_id, timestamp, duration, datastr) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![bucket_key, timestamp, duration, data],
                )
                .unwrap();
            }
        }

        fn new_datastore() -> (Connection, DatastoreInstance) {
            let conn = Connection::open_in_memory().unwrap();
            let ds = DatastoreInstance::new(&conn, true).unwrap();
            (conn, ds)
        }

        #[test]
        fn no_legacy_db_found_is_not_an_error() {
            let tmp = tempfile::tempdir().unwrap();
            let missing_path = tmp.path().join("does-not-exist.db");
            let (conn, mut ds) = new_datastore();
            let imported =
                legacy_import(&mut ds, &conn, Some(missing_path.to_str().unwrap())).unwrap();
            assert!(!imported);
            assert!(ds.get_buckets().is_empty());
        }

        #[test]
        fn imports_buckets_and_events_from_fixture_db() {
            let tmp = tempfile::tempdir().unwrap();
            let legacy_path = tmp.path().join("legacy.db");
            write_fixture_legacy_db(
                &legacy_path,
                &[
                    ("2026-01-01 10:00:00+00:00", 5.0, r#"{"status": "afk"}"#),
                    (
                        "2026-01-01 10:01:00+00:00",
                        10.0,
                        r#"{"status": "not-afk"}"#,
                    ),
                ],
            );
            let (conn, mut ds) = new_datastore();
            let imported =
                legacy_import(&mut ds, &conn, Some(legacy_path.to_str().unwrap())).unwrap();
            assert!(imported);

            let buckets = ds.get_buckets();
            assert_eq!(buckets.len(), 1);
            let bucket_id = buckets.keys().next().unwrap().clone();
            assert_eq!(bucket_id, "aw-watcher-afk_testhost");

            let events = ds.get_events(&conn, &bucket_id, None, None, None).unwrap();
            assert_eq!(events.len(), 2);
        }

        #[test]
        fn legacy_dbpath_override_is_honored_over_default_location() {
            // Regression guard for #546: without an override, a custom
            // location (e.g. a testing fixture, or a non-default XDG data
            // dir) is silently never found.
            let tmp = tempfile::tempdir().unwrap();
            let custom_path = tmp.path().join("custom-name.db");
            write_fixture_legacy_db(&custom_path, &[("2026-01-01 10:00:00+00:00", 5.0, "{}")]);
            let (conn, mut ds) = new_datastore();
            let imported =
                legacy_import(&mut ds, &conn, Some(custom_path.to_str().unwrap())).unwrap();
            assert!(imported);
            assert_eq!(ds.get_buckets().len(), 1);
        }

        #[test]
        fn forced_reimport_into_existing_bucket_is_idempotent() {
            let tmp = tempfile::tempdir().unwrap();
            let legacy_path = tmp.path().join("legacy.db");
            write_fixture_legacy_db(
                &legacy_path,
                &[
                    ("2026-01-01 10:00:00+00:00", 5.0, r#"{"status": "afk"}"#),
                    (
                        "2026-01-01 10:01:00+00:00",
                        10.0,
                        r#"{"status": "not-afk"}"#,
                    ),
                ],
            );
            let (conn, mut ds) = new_datastore();
            assert!(legacy_import(&mut ds, &conn, Some(legacy_path.to_str().unwrap())).unwrap());

            // Simulates `aw-server --import-legacy` run again on a datastore
            // that already imported this exact legacy db: must not panic on
            // BucketAlreadyExists, and must not duplicate events.
            assert!(legacy_import(&mut ds, &conn, Some(legacy_path.to_str().unwrap())).unwrap());

            let bucket_id = ds.get_buckets().keys().next().unwrap().clone();
            let events = ds.get_events(&conn, &bucket_id, None, None, None).unwrap();
            assert_eq!(events.len(), 2, "re-import must not duplicate events");
        }

        #[test]
        fn forced_reimport_merges_only_new_events() {
            let tmp = tempfile::tempdir().unwrap();
            let legacy_path = tmp.path().join("legacy.db");
            write_fixture_legacy_db(
                &legacy_path,
                &[("2026-01-01 10:00:00+00:00", 5.0, r#"{"n": 1}"#)],
            );
            let (conn, mut ds) = new_datastore();
            assert!(legacy_import(&mut ds, &conn, Some(legacy_path.to_str().unwrap())).unwrap());

            // A later legacy export of the same bucket, with one new event
            // added alongside the one already imported.
            let legacy_path_v2 = tmp.path().join("legacy-v2.db");
            write_fixture_legacy_db(
                &legacy_path_v2,
                &[
                    ("2026-01-01 10:00:00+00:00", 5.0, r#"{"n": 1}"#),
                    ("2026-01-01 10:05:00+00:00", 5.0, r#"{"n": 2}"#),
                ],
            );
            assert!(legacy_import(&mut ds, &conn, Some(legacy_path_v2.to_str().unwrap())).unwrap());

            let bucket_id = ds.get_buckets().keys().next().unwrap().clone();
            let events = ds.get_events(&conn, &bucket_id, None, None, None).unwrap();
            assert_eq!(
                events.len(),
                2,
                "only the genuinely new event should be merged in"
            );
        }
    }
}

#[cfg(target_os = "android")]
mod import {
    use super::LegacyDatastoreImportError;
    use crate::datastore::DatastoreInstance;
    use rusqlite::Connection;

    pub fn legacy_import(
        _new_ds: &mut DatastoreInstance,
        _new_conn: &Connection,
        _legacy_db_path_override: Option<&str>,
    ) -> Result<bool, LegacyDatastoreImportError> {
        info!("Legacy import is not supported on Android, skipping");
        Ok(false)
    }
}
