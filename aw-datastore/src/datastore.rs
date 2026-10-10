use std::collections::HashMap;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

use rusqlite::Connection;

use serde_json::value::Value;

use aw_models::Bucket;
use aw_models::BucketMetadata;
use aw_models::Event;

use rusqlite::params;
use rusqlite::types::ToSql;
use rusqlite::OptionalExtension;

use super::DatastoreError;

fn _get_db_version(conn: &Connection) -> i32 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// Infer the schema version of a populated database whose `user_version` is 0.
///
/// `sqlite3 .dump` (the documented corruption-recovery path) does not carry
/// `PRAGMA user_version`, so a restored database reports 0 even though its
/// tables exist. Re-running the v0 migrations on it panics (duplicate column,
/// table already exists). Returns 0 for a genuinely empty database.
pub(crate) fn _infer_db_version(conn: &Connection) -> i32 {
    let has = |kind: &str, name: &str| -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params![kind, name],
            |_| Ok(()),
        )
        .is_ok()
    };
    if !has("table", "buckets") || !has("table", "events") {
        return 0;
    }
    let has_column = |col: &str| -> bool {
        conn.query_row(
            "SELECT 1 FROM pragma_table_info('buckets') WHERE name = ?1",
            params![col],
            |_| Ok(()),
        )
        .is_ok()
    };
    // Recognise v7 before the earlier schema's column and index checks.
    if has_column("device_id") {
        return 7;
    }
    if !has_column("data") && !has_column("data_deprecated") {
        return 1;
    }
    if !has_column("data_deprecated") {
        return 2;
    }
    if !has("table", "key_value") {
        return 3;
    }
    if has("index", "events_bucketrow_endtime_starttime_index") {
        6
    } else if has("index", "events_bucketrow_starttime_endtime_index") {
        5
    } else {
        4
    }
}

/*
 * ### Database version changelog ###
 * 0: Uninitialized database
 * 1: Initialized database
 * 2: Added 'data' field to 'buckets' table
 * 3: see: https://github.com/ActivityWatch/aw-server-rust/pull/52
 * 4: Added 'key_value' table for storing key - value pairs
 * 5: Replaced single-column events indexes with a composite index
 * 6: Added an endtime-first index for recent interval reads
 * 7: Added 'device_id' to 'buckets' (UNIQUE(device_id, name))
 */
pub const NEWEST_DB_VERSION: i32 = 7;

/// Oldest version a read-only open (aw-sync pulling a peer db) accepts.
///
/// v4, v5 and v6 have identical tables and columns; v5 and v6 only changed
/// indexes. A read-only connection cannot migrate, so queries adapt to the
/// indexes the file has (see [`events_source`]) instead of rejecting it.
///
/// Rule for future migrations: if a migration changes tables or columns that
/// reads depend on, bump this to the new version. If it only adds or drops
/// indexes, leave it alone and teach [`events_source`] about the new index.
pub const MIN_READ_COMPAT_DB_VERSION: i32 = 4;

/// The `FROM` source for event range reads, with an `INDEXED BY` hint only
/// when this database version is guaranteed to have that index.
///
/// `INDEXED BY` on a missing index is a prepare error, not a hint that
/// SQLite ignores. Writable databases are always migrated to
/// [`NEWEST_DB_VERSION`], but a read-only peer db (aw-sync pull) can be older:
/// v4 has only the single-column indexes, v5 lacks the endtime-first index.
pub(crate) fn events_source(db_version: i32, prefer_endtime: bool) -> &'static str {
    if db_version >= 6 && prefer_endtime {
        "events INDEXED BY events_bucketrow_endtime_starttime_index"
    } else if db_version >= 5 {
        "events INDEXED BY events_bucketrow_starttime_endtime_index"
    } else {
        "events"
    }
}

fn _create_tables(conn: &Connection, version: i32) -> bool {
    let mut first_init = false;

    if version < 1 {
        first_init = true;
        _migrate_v0_to_v1(conn);
    }

    if version < 2 {
        _migrate_v1_to_v2(conn);
    }

    if version < 3 {
        _migrate_v2_to_v3(conn);
    }

    if version < 4 {
        _migrate_v3_to_v4(conn);
    }

    if version < 5 {
        _migrate_v4_to_v5(conn);
    }

    if version < 6 {
        _migrate_v5_to_v6(conn);
    }

    if version < 7 {
        _migrate_v6_to_v7(conn);
    }

    first_init
}

fn _migrate_v0_to_v1(conn: &Connection) {
    /* Set up bucket table */
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS buckets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            type TEXT NOT NULL,
            client TEXT NOT NULL,
            hostname TEXT NOT NULL,
            created TEXT NOT NULL
        )",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create buckets table");

    /* Set up index for bucket table */
    conn.execute(
        "CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create buckets index");

    /* Set up events table */
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bucketrow INTEGER NOT NULL,
            starttime INTEGER NOT NULL,
            endtime INTEGER NOT NULL,
            data TEXT NOT NULL,
            FOREIGN KEY (bucketrow) REFERENCES buckets(id)
        )",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create events table");

    /* Set up index for events table */
    conn.execute(
        "CREATE INDEX IF NOT EXISTS events_bucketrow_index ON events(bucketrow)",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create events_bucketrow index");
    conn.execute(
        "CREATE INDEX IF NOT EXISTS events_starttime_index ON events(starttime)",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create events_starttime index");
    conn.execute(
        "CREATE INDEX IF NOT EXISTS events_endtime_index ON events(endtime)",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to create events_endtime index");

    /* Update database version */
    conn.pragma_update(None, "user_version", 1)
        .expect("Failed to update database version!");
}

fn _migrate_v1_to_v2(conn: &Connection) {
    info!("Upgrading database to v2, adding data field to buckets");
    conn.execute(
        "ALTER TABLE buckets ADD COLUMN data TEXT DEFAULT '{}';",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to upgrade database when adding data field to buckets");

    conn.pragma_update(None, "user_version", 2)
        .expect("Failed to update database version!");
}

fn _migrate_v2_to_v3(conn: &Connection) {
    // For details about why this migration was necessary, see: https://github.com/ActivityWatch/aw-server-rust/pull/52
    info!("Upgrading database to v3, replacing the broken data field for buckets");

    // Rename column, marking it as deprecated
    match conn.execute(
        "ALTER TABLE buckets RENAME COLUMN data TO data_deprecated;",
        &[] as &[&dyn ToSql],
    ) {
        Ok(_) => (),
        // This error is okay, it still has the intended effects
        Err(rusqlite::Error::ExecuteReturnedResults) => (),
        Err(e) => panic!("Unexpected error: {e:?}"),
    };

    // Create new correct column
    conn.execute(
        "ALTER TABLE buckets ADD COLUMN data TEXT NOT NULL DEFAULT '{}';",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to upgrade database when adding new data field to buckets");

    conn.pragma_update(None, "user_version", 3)
        .expect("Failed to update database version!");
}

fn _migrate_v3_to_v4(conn: &Connection) {
    info!("Upgrading database to v4, adding table for key-value storage");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS key_value (
        key TEXT PRIMARY KEY,
        value TEXT,
        last_modified NUMBER NOT NULL
    );",
        [],
    )
    .expect("Failed to upgrade db and add key-value storage table");

    conn.pragma_update(None, "user_version", 4)
        .expect("Failed to update database version!");
}

fn _migrate_v4_to_v5(conn: &Connection) {
    info!(
        "Upgrading database to v5, replacing single-column events indexes with a composite index"
    );
    // Every event query filters on bucketrow and a starttime/endtime range,
    // ordered by starttime. A composite index serves the seek, the range scan
    // and the ORDER BY in one pass (with endtime checked from the index
    // without fetching the row), where the single-column indexes could only
    // cover one predicate and left the rest as scan + sort. Dropping them
    // also makes inserts cheaper (one index to maintain instead of three).
    //
    // starttime is DESC so a forward scan yields the query's newest-first
    // order with equal-timestamp events in rowid (insertion) order, matching
    // the ordering callers observed before this index existed.
    //
    // The drops run before the create so the pages they free are reused to
    // build the new index within the same transaction; creating first would
    // permanently grow the database file by the new index's size.
    conn.execute_batch(
        "
        BEGIN EXCLUSIVE TRANSACTION;
        DROP INDEX IF EXISTS events_bucketrow_index;
        DROP INDEX IF EXISTS events_starttime_index;
        DROP INDEX IF EXISTS events_endtime_index;
        CREATE INDEX IF NOT EXISTS events_bucketrow_starttime_endtime_index
            ON events(bucketrow, starttime DESC, endtime);
        PRAGMA user_version = 5;
        COMMIT;
    ",
    )
    .expect("Failed to run v5 migration transaction");
}

fn _migrate_v5_to_v6(conn: &Connection) {
    conn.execute_batch(
        "BEGIN EXCLUSIVE TRANSACTION;
         CREATE INDEX IF NOT EXISTS events_bucketrow_endtime_starttime_index
             ON events(bucketrow, endtime, starttime DESC);
         PRAGMA user_version = 6;
         COMMIT;",
    )
    .expect("Failed to run v6 migration transaction");
}

fn _migrate_v6_to_v7(conn: &Connection) {
    // Add device_id column and change uniqueness from (name) to (device_id, name).
    // SQLite cannot drop a UNIQUE column constraint in-place, so we recreate the
    // table. Existing buckets are all locally-created, so they get device_id='local'.
    // Keep data_deprecated as an archive of the broken pre-v3 field. It must
    // neither be discarded by this rebuild nor promoted to current metadata.
    //
    // PRAGMA foreign_keys must be disabled outside any transaction for the table
    // recreation to succeed: events→buckets(id) is an IMMEDIATE FK, and even with
    // defer_foreign_keys=ON the constraint fires during DROP TABLE inside the batch.
    // All IDs are preserved by the copy, so re-enabling FK checks after COMMIT is safe.
    info!("Upgrading database to v7, adding device_id to buckets");
    let foreign_keys: bool = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .expect("Failed to read foreign_keys before v7 migration");
    conn.pragma_update(None, "foreign_keys", false)
        .expect("Failed to disable foreign_keys for v7 migration");
    // The transaction guard rolls back on any error before restoring the pragma
    // (SQLite ignores foreign_keys changes while a transaction is active).
    let migration = (|| -> rusqlite::Result<()> {
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Exclusive)?;
        // The caller read user_version before this lock: a concurrent opener may
        // have finished the migration (and written device IDs) while we waited.
        if _get_db_version(&transaction) >= 7 {
            return Ok(());
        }
        // DROP TABLE removes the AUTOINCREMENT high-water mark; carry it over so
        // ids of deleted buckets are never reused by an unrelated bucket.
        let sequence: Option<i64> = transaction
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'buckets'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        // Some historical/restored schemas omit the unused archive column.
        // Preserve it when present, including SQL NULL and malformed payloads.
        let has_archive: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('buckets') WHERE name = 'data_deprecated')",
            [],
            |row| row.get(0),
        )?;
        let archive = if has_archive {
            "data_deprecated"
        } else {
            "NULL"
        };
        transaction.execute_batch(&format!(
            "CREATE TABLE buckets_v7 (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             device_id TEXT NOT NULL DEFAULT 'local',
             type TEXT NOT NULL,
             client TEXT NOT NULL,
             hostname TEXT NOT NULL,
             created TEXT NOT NULL,
             data TEXT NOT NULL DEFAULT '{{}}',
             data_deprecated TEXT DEFAULT '{{}}',
             UNIQUE(device_id, name)
         );
         INSERT INTO buckets_v7 (id, name, device_id, type, client, hostname, created, data, data_deprecated)
             SELECT id, name, 'local', type, client, hostname, created, data, {archive} FROM buckets;
         DROP TABLE buckets;
         ALTER TABLE buckets_v7 RENAME TO buckets;
         CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id);
         PRAGMA user_version = 7;"
        ))?;
        if let Some(sequence) = sequence {
            let updated = transaction.execute(
                "UPDATE sqlite_sequence SET seq = max(seq, ?1) WHERE name = 'buckets'",
                [sequence],
            )?;
            if updated == 0 {
                transaction.execute(
                    "INSERT INTO sqlite_sequence (name, seq) VALUES ('buckets', ?1)",
                    [sequence],
                )?;
            }
        }
        transaction.commit()
    })();
    conn.pragma_update(None, "foreign_keys", foreign_keys)
        .expect("Failed to restore foreign_keys after v7 migration");
    migration.expect("Failed to run v7 migration transaction");
}

// Both indexes can bound only one of the two interval predicates. Use the
// bucket's time span as a cheap selectivity estimate; this affects performance,
// never which rows qualify. Limited queries retain their ordered starttime scan
// so LIMIT can stop early without sorting all matching rows.
pub(crate) fn prefer_endtime_index(
    bucket: &Bucket,
    start: i64,
    end: i64,
    limit: Option<u64>,
) -> bool {
    if limit.is_some() {
        return false;
    }
    match (
        bucket
            .metadata
            .start
            .and_then(|dt| dt.timestamp_nanos_opt()),
        bucket.metadata.end.and_then(|dt| dt.timestamp_nanos_opt()),
    ) {
        (Some(first), Some(last)) => {
            let first = i128::from(first);
            let last = i128::from(last);
            // Clamp unbounded ranges to the bucket span, so a full-bucket
            // export keeps its ordered scan. Require a substantial advantage
            // to offset sorting the endtime index's matching rows.
            let start_scan = (i128::from(end).min(last) - first).max(0);
            let end_scan = (last - i128::from(start).max(first)).max(0);
            end_scan * 4 < start_scan
        }
        _ => false,
    }
}

pub struct DatastoreInstance {
    buckets_cache: HashMap<String, Bucket>,
    /// `(count, max id, names)` of the buckets table when `buckets_cache` was
    /// loaded; lets a reader detect creates, deletes and renames by another
    /// connection with one cheap query.
    buckets_signature: (i64, i64, String),
    first_init: bool,
    pub db_version: i32,
}

/// Nanoseconds since the epoch for a time filter, clamped to `i64` for dates chrono can't
/// express in nanoseconds (before 1677 or after 2262), so such a bound means "no limit"
/// instead of panicking the datastore worker. Bounds that can't match anything are
/// caught first by [`filter_excludes_everything`].
fn filter_nanos(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_nanos_opt().unwrap_or(if dt.timestamp() < 0 {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// Whether a time filter can't match any stored event: a start after 2262 or an end
/// before 1677, beyond the range event times are stored in.
fn filter_excludes_everything(
    starttime_opt: Option<DateTime<Utc>>,
    endtime_opt: Option<DateTime<Utc>>,
) -> bool {
    let out_of_range = |dt: &DateTime<Utc>| dt.timestamp_nanos_opt().is_none();
    starttime_opt.is_some_and(|dt| out_of_range(&dt) && dt.timestamp() > 0)
        || endtime_opt.is_some_and(|dt| out_of_range(&dt) && dt.timestamp() < 0)
}

fn _datetime_from_nanos(ns: i64) -> DateTime<Utc> {
    // Euclidean division so a negative (pre-epoch) timestamp still yields a
    // subnanos remainder in [0, 1_000_000_000) instead of a negative value
    // that wraps to a bogus u32 and makes `from_timestamp` return None.
    let seconds = ns.div_euclid(1_000_000_000);
    let subnanos = ns.rem_euclid(1_000_000_000) as u32;
    DateTime::from_timestamp(seconds, subnanos).unwrap()
}

/// Parse an event from a row selected as `id, starttime, endtime, data`.
///
/// When `clip` is set to `(starttime_filter_ns, endtime_filter_ns)`, the event is
/// clamped to that query range.
pub(crate) fn parse_event_row(
    row: &rusqlite::Row,
    clip: Option<(i64, i64)>,
) -> rusqlite::Result<Event> {
    let id = row.get(0)?;
    let mut starttime_ns: i64 = row.get(1)?;
    let mut endtime_ns: i64 = row.get(2)?;
    let data_str: String = row.get(3)?;

    if let Some((starttime_filter_ns, endtime_filter_ns)) = clip {
        if starttime_ns < starttime_filter_ns {
            starttime_ns = starttime_filter_ns
        }
        if endtime_ns > endtime_filter_ns {
            endtime_ns = endtime_filter_ns
        }
    }
    let duration_ns = endtime_ns - starttime_ns;

    let time_seconds: i64 = starttime_ns / 1_000_000_000;
    let time_subnanos: u32 = (starttime_ns % 1_000_000_000) as u32;
    let data: serde_json::map::Map<String, Value> =
        serde_json::from_str(&data_str).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
        })?;

    Ok(Event {
        id: Some(id),
        timestamp: DateTime::from_timestamp(time_seconds, time_subnanos).unwrap(),
        duration: Duration::nanoseconds(duration_ns),
        data,
    })
}

/// Legacy-bucket events that overlap no other event in either bucket, so they can
/// be moved into the destination bucket without creating overlapping records.
///
/// Two events overlap when `a.starttime < b.endtime AND b.starttime < a.endtime`
/// (strict, matching the datastore's overlap semantics elsewhere). Events are
/// read once ordered by `starttime`. A sweep keeps still-open events in a
/// min-heap keyed by `endtime`. A positive-duration event overlaps every
/// still-open event, so those are marked in O(1) via an epoch counter rather
/// than scanning the heap (which would be quadratic for stacked histories).
/// Zero-duration events are the only case that still scans, and they never stay
/// in the open set. The pass is O(n log n).
fn movable_legacy_event_ids(
    conn: &Connection,
    legacy_bid: Option<i64>,
    destination_bid: Option<i64>,
) -> rusqlite::Result<Vec<i64>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    struct Span {
        id: i64,
        starttime: i64,
        endtime: i64,
        legacy: bool,
    }

    let mut stmt = conn.prepare(
        "SELECT id, starttime, endtime, bucketrow FROM events
         WHERE bucketrow IN (?1, ?2)
         ORDER BY starttime ASC, id ASC",
    )?;
    let spans = stmt
        .query_map(params![legacy_bid, destination_bid], |row| {
            let bucketrow: Option<i64> = row.get(3)?;
            Ok(Span {
                id: row.get(0)?,
                starttime: row.get(1)?,
                endtime: row.get(2)?,
                legacy: bucketrow == legacy_bid,
            })
        })?
        .collect::<rusqlite::Result<Vec<Span>>>()?;

    let mut overlaps = vec![false; spans.len()];
    // Open events: (endtime, index, epoch-at-push). Epoch marks the whole
    // concurrent set in O(1) when a later positive-duration event overlaps it.
    let mut open: BinaryHeap<Reverse<(i64, usize, u32)>> = BinaryHeap::new();
    let mut epoch: u32 = 0;
    for (i, span) in spans.iter().enumerate() {
        // Events that ended at or before this start can never overlap this or
        // any later event (later events start no earlier than this one).
        while let Some(Reverse((endtime, j, pushed_epoch))) = open.peek().copied() {
            if endtime <= span.starttime {
                open.pop();
                if pushed_epoch < epoch {
                    overlaps[j] = true;
                }
            } else {
                break;
            }
        }
        if !open.is_empty() {
            if span.endtime > span.starttime {
                // Positive duration: every remaining open event `j` has
                // `j.starttime <= span.starttime < span.endtime` and
                // `span.starttime < j.endtime`, so they all overlap.
                overlaps[i] = true;
                epoch += 1;
            } else {
                // Zero-duration: overlaps open events that started strictly
                // earlier, but not same-start positive-duration events
                // (`j.starttime < span.endtime` fails when starttimes match).
                let mut overlapped = false;
                for Reverse((_, j, _)) in open.iter() {
                    if spans[*j].starttime < span.starttime {
                        overlaps[*j] = true;
                        overlapped = true;
                    }
                }
                if overlapped {
                    overlaps[i] = true;
                }
            }
        }
        open.push(Reverse((span.endtime, i, epoch)));
    }
    while let Some(Reverse((_, j, pushed_epoch))) = open.pop() {
        if pushed_epoch < epoch {
            overlaps[j] = true;
        }
    }

    Ok(spans
        .iter()
        .zip(overlaps.iter())
        .filter(|(span, overlapping)| span.legacy && !**overlapping)
        .map(|(span, _)| span.id)
        .collect())
}

/// Reassign the given events to `bucket_bid`, in batches so the statement stays
/// well under SQLite's bound-parameter limit.
fn move_events_to_bucket(
    conn: &Connection,
    bucket_bid: Option<i64>,
    event_ids: &[i64],
) -> rusqlite::Result<()> {
    const BATCH: usize = 500;
    for chunk in event_ids.chunks(BATCH) {
        let placeholders = (0..chunk.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE events SET bucketrow = ?1 WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut values: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() + 1);
        values.push(&bucket_bid);
        for id in chunk {
            values.push(id);
        }
        stmt.execute(values.as_slice())?;
    }
    Ok(())
}

impl DatastoreInstance {
    pub fn new(
        conn: &Connection,
        migrate_enabled: bool,
    ) -> Result<DatastoreInstance, DatastoreError> {
        let mut first_init = false;
        let mut db_version = _get_db_version(conn);

        // `sqlite3 .dump` (the documented corruption-recovery path) drops
        // `PRAGMA user_version`, so a restored file reports 0 while its schema
        // is at some real version. Infer it and use that: a read-only open
        // (aw-sync pulling a peer db) must not reject the file as unsupported,
        // and a writable open must not re-run v0 migrations on it. Only a
        // writable open persists the inferred version; a read-only one cannot.
        if db_version == 0 {
            let inferred = _infer_db_version(conn);
            if inferred > 0 {
                if migrate_enabled {
                    warn!(
                        "Database has user_version 0 but a populated schema (restored from a \
                         dump?), treating it as v{inferred}"
                    );
                    conn.pragma_update(None, "user_version", inferred)
                        .expect("Failed to update database version!");
                }
                db_version = inferred;
            }
        }

        if migrate_enabled {
            first_init = _create_tables(conn, db_version);
            // Queries pick index hints from this, so it must describe the
            // migrated file, not the version it had before this open.
            db_version = _get_db_version(conn);
        } else if db_version < 0 {
            return Err(DatastoreError::Uninitialized(
                "Tried to open an uninitialized datastore with migration disabled".to_string(),
            ));
        } else if !(MIN_READ_COMPAT_DB_VERSION..=NEWEST_DB_VERSION).contains(&db_version) {
            return Err(DatastoreError::OldDbVersion(format!(
                "\
                Tried to open an database with an incompatible database version!
                Database has version {db_version} while the supported versions are \
                {MIN_READ_COMPAT_DB_VERSION}..={NEWEST_DB_VERSION}"
            )));
        }

        let mut ds = DatastoreInstance {
            buckets_cache: HashMap::new(),
            buckets_signature: (0, 0, String::new()),
            first_init,
            db_version,
        };
        ds.get_stored_buckets(conn)?;
        Ok(ds)
    }

    /// Re-read the bucket list from the database. A reader instance
    /// ([`crate::DatastoreMethod::FileReader`]) keeps a cache that goes stale
    /// when the writer creates, deletes, renames or imports buckets.
    pub fn reload_buckets(&mut self, conn: &Connection) -> Result<(), DatastoreError> {
        self.get_stored_buckets(conn)
    }

    fn read_buckets_signature(conn: &Connection) -> Result<(i64, i64, String), DatastoreError> {
        conn.query_row(
            "SELECT count(*), coalesce(max(id), 0), coalesce(group_concat(name, char(10)), '') \
             FROM (SELECT id, name FROM buckets ORDER BY id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| DatastoreError::InternalError(format!("Failed to read bucket signature: {e}")))
    }

    /// Whether buckets were created, deleted or renamed since the cache was
    /// loaded (ids are AUTOINCREMENT, so a delete-and-recreate changes
    /// `max(id)`; the name list catches renames).
    pub fn buckets_changed(&self, conn: &Connection) -> Result<bool, DatastoreError> {
        Ok(Self::read_buckets_signature(conn)? != self.buckets_signature)
    }

    fn get_stored_buckets(&mut self, conn: &Connection) -> Result<(), DatastoreError> {
        // Read before the list so a concurrent change lands in the next check.
        let signature = Self::read_buckets_signature(conn)?;
        // Read-only opens of peer DBs (aw-sync) may be at v4-v6, which pre-date the
        // device_id column. Use a literal 'local' for those; v7+ reads the real column.
        let device_id_expr = if self.db_version >= 7 {
            "buckets.device_id"
        } else {
            "'local'"
        };
        let sql = format!(
            "
            SELECT  buckets.id, buckets.name, buckets.type, buckets.client,
                    buckets.hostname, buckets.created,
                    min(events.starttime), max(events.endtime),
                    buckets.data, {device_id_expr}
            FROM buckets
            LEFT OUTER JOIN events ON buckets.id = events.bucketrow
            GROUP BY buckets.id
            ;"
        );
        let mut stmt = match conn.prepare_cached(&sql) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_stored_buckets SQL statement: {err}"
                )))
            }
        };
        let buckets = match stmt.query_map(&[] as &[&dyn ToSql], |row| {
            let opt_start_ns: Option<i64> = row.get(6)?;
            let opt_start = match opt_start_ns {
                Some(starttime_ns) => {
                    let seconds: i64 = starttime_ns / 1_000_000_000;
                    let subnanos: u32 = (starttime_ns % 1_000_000_000) as u32;
                    Some(DateTime::from_timestamp(seconds, subnanos).unwrap())
                }
                None => None,
            };

            let opt_end_ns: Option<i64> = row.get(7)?;
            let opt_end = match opt_end_ns {
                Some(endtime_ns) => {
                    let seconds: i64 = endtime_ns / 1_000_000_000;
                    let subnanos: u32 = (endtime_ns % 1_000_000_000) as u32;
                    Some(DateTime::from_timestamp(seconds, subnanos).unwrap())
                }
                None => None,
            };

            // If data column is not set (possible on old installations), use an empty map as default
            let data_str: String = row.get(8)?;
            let data_json = match serde_json::from_str(&data_str) {
                Ok(data) => data,
                Err(e) => {
                    return Err(rusqlite::Error::InvalidColumnName(format!(
                        "Failed to parse data to JSON: {e:?}"
                    )))
                }
            };

            Ok(Bucket {
                bid: row.get(0)?,
                id: row.get(1)?,
                _type: row.get(2)?,
                client: row.get(3)?,
                hostname: row.get(4)?,
                created: row.get(5)?,
                device_id: row.get(9)?,
                data: data_json,
                metadata: BucketMetadata {
                    start: opt_start,
                    end: opt_end,
                },
                events: None,
                last_updated: None,
            })
        }) {
            Ok(buckets) => buckets,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to query get_stored_buckets SQL statement: {err:?}"
                )))
            }
        };
        let mut new_cache = HashMap::new();
        for bucket in buckets {
            match bucket {
                Ok(b) => {
                    if new_cache.contains_key(&b.id) {
                        return Err(DatastoreError::InternalError(format!(
                            "Cannot load ambiguous bucket name {:?}: device-aware addressing is not implemented",
                            b.id
                        )));
                    }
                    new_cache.insert(b.id.clone(), b);
                }
                Err(e) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to parse bucket from SQLite, database is corrupt! {e:?}"
                    )))
                }
            }
        }
        self.buckets_cache = new_cache;
        self.buckets_signature = signature;
        Ok(())
    }

    /// Imports a legacy (Python aw-server) database, if one is found.
    ///
    /// By default this only runs once, the first time the datastore is
    /// created (`first_init`) — if aw-server-rust had already run before,
    /// nothing happens and nothing is logged beyond an INFO line explaining
    /// why the import was skipped (ActivityWatch/aw-server-rust#546). Pass
    /// `force = true` (`aw-server --import-legacy`) to run the import
    /// regardless of `first_init`; re-running is idempotent, since matching
    /// events are deduped against what's already in the bucket.
    ///
    /// `legacy_db_path_override` overrides the default
    /// `peewee-sqlite.v2.db` lookup location (`aw-server --legacy-dbpath`).
    #[allow(clippy::result_unit_err)]
    pub fn ensure_legacy_import(
        &mut self,
        conn: &Connection,
        legacy_db_path_override: Option<&str>,
        force: bool,
    ) -> Result<bool, ()> {
        use super::legacy_import::legacy_import;
        if !self.first_init && !force {
            info!(
                "Datastore was already initialized, skipping legacy import \
                 (use `aw-server --import-legacy` to run it explicitly)"
            );
            Ok(false)
        } else {
            self.first_init = false;
            match legacy_import(self, conn, legacy_db_path_override) {
                Ok(imported) => {
                    if imported {
                        info!("Successfully imported legacy database");
                        self.get_stored_buckets(conn).unwrap();
                    }
                    Ok(imported)
                }
                Err(err) => {
                    warn!("Failed to import legacy database: {:?}", err);
                    Err(())
                }
            }
        }
    }

    pub fn create_bucket(
        &mut self,
        conn: &Connection,
        mut bucket: Bucket,
    ) -> Result<(), DatastoreError> {
        bucket.created = match bucket.created {
            Some(created) => Some(created),
            None => Some(Utc::now()),
        };
        // Stamp device_id = "local" for locally-created buckets.
        // Peer buckets from aw-sync will carry their own device_id via import.
        if bucket.device_id.is_empty() {
            bucket.device_id = "local".to_string();
        }
        // The v7 schema prepares for device-scoped names, but the public API
        // and cache still address buckets by name. Until the resolver exists,
        // reject every duplicate name before it can hide history or redirect writes.
        let mut stmt = match conn.prepare_cached(
            "
                INSERT INTO buckets (name, device_id, type, client, hostname, created, data)
                SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
                WHERE NOT EXISTS (SELECT 1 FROM buckets WHERE name = ?1)",
        ) {
            Ok(buckets) => buckets,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare create_bucket SQL statement: {err}"
                )))
            }
        };
        let data = serde_json::to_string(&bucket.data).unwrap();
        let res = stmt.execute([
            &bucket.id,
            &bucket.device_id,
            &bucket._type,
            &bucket.client,
            &bucket.hostname,
            &bucket.created as &dyn ToSql,
            &data,
        ]);

        match res {
            // The cache may be stale when another connection created this name.
            // Checking in the INSERT makes rejection atomic across writers;
            // a skipped insert must not reuse last_insert_rowid or update the cache.
            Ok(0) => Err(DatastoreError::BucketAlreadyExists(bucket.id.to_string())),
            Ok(_) => {
                info!("Created bucket {}", bucket.id);
                // Get and set rowid
                let rowid: i64 = conn.last_insert_rowid();
                bucket.bid = Some(rowid);
                // Take out events from struct before caching
                let events = bucket.events;
                bucket.events = None;
                // Cache bucket
                self.buckets_cache.insert(bucket.id.clone(), bucket.clone());
                // Insert events
                if let Some(events) = events {
                    self.insert_events(conn, &bucket.id, events.take_inner())?;
                    bucket.events = None;
                }
                Ok(())
            }
            Err(rusqlite::Error::SqliteFailure(sqlerr, _))
                if sqlerr.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(DatastoreError::BucketAlreadyExists(bucket.id.to_string()))
            }
            Err(err) => Err(DatastoreError::InternalError(format!(
                "Failed to execute create_bucket SQL statement: {err}"
            ))),
        }
    }

    pub fn delete_bucket(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
    ) -> Result<(), DatastoreError> {
        let bucket = (self.get_bucket(bucket_id))?;
        // Delete all events in bucket
        match conn.execute("DELETE FROM events WHERE bucketrow = ?1", [&bucket.bid]) {
            Ok(_) => (),
            Err(err) => return Err(DatastoreError::InternalError(err.to_string())),
        }
        // Delete bucket itself
        match conn.execute("DELETE FROM buckets WHERE id = ?1", [&bucket.bid]) {
            Ok(_) => {
                self.buckets_cache.remove(bucket_id);
                Ok(())
            }
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _ } => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => {
                        Err(DatastoreError::BucketAlreadyExists(bucket_id.to_string()))
                    }
                    _ => Err(DatastoreError::InternalError(err.to_string())),
                },
                _ => Err(DatastoreError::InternalError(err.to_string())),
            },
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cached_bucket = self.buckets_cache.get(bucket_id);
        match cached_bucket {
            Some(bucket) => Ok(bucket.clone()),
            None => Err(DatastoreError::NoSuchBucket(bucket_id.to_string())),
        }
    }

    pub fn get_buckets(&self) -> HashMap<String, Bucket> {
        self.buckets_cache.clone()
    }

    /// Serialize an export one event at a time using the caller's connection.
    /// Returns the bucket ID for a single-bucket export (for download naming).
    pub fn write_export(
        &self,
        conn: &Connection,
        bucket_id: Option<&str>,
        writer: impl std::io::Write,
    ) -> Result<Option<String>, DatastoreError> {
        crate::export::write_export(
            conn,
            self.db_version,
            &self.buckets_cache,
            bucket_id,
            writer,
        )?;
        Ok(bucket_id.map(str::to_owned).or_else(|| {
            (self.buckets_cache.len() == 1)
                .then(|| self.buckets_cache.keys().next().unwrap().clone())
        }))
    }

    /// Stream one bucket's events as RFC-4180 CSV, one row at a time.
    ///
    /// Uses the same filters, clipping, and corrupt-row policy as `get_events`.
    pub fn write_events_csv(
        &self,
        conn: &Connection,
        bucket_id: &str,
        start: Option<chrono::DateTime<chrono::Utc>>,
        end: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<u64>,
        writer: impl std::io::Write,
    ) -> Result<(), DatastoreError> {
        crate::export::write_events_csv(
            conn,
            self.db_version,
            &self.buckets_cache,
            bucket_id,
            start,
            end,
            limit,
            writer,
        )
    }

    pub fn insert_events(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        mut events: Vec<Event>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let mut bucket = self.get_bucket(bucket_id)?;

        let mut stmt = match conn.prepare_cached(
            "
                INSERT OR REPLACE INTO events(bucketrow, id, starttime, endtime, data)
                VALUES (?1, ?2, ?3, ?4, ?5)",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_events SQL statement: {err}"
                )))
            }
        };
        for event in &mut events {
            let starttime_nanos = event.timestamp.timestamp_nanos_opt().unwrap();
            let duration_nanos = match event.duration.num_nanoseconds() {
                Some(nanos) => nanos,
                None => {
                    return Err(DatastoreError::InternalError(
                        "Failed to convert duration to nanoseconds".to_string(),
                    ))
                }
            };
            let endtime_nanos = starttime_nanos + duration_nanos;
            let data = serde_json::to_string(&event.data).unwrap();
            let res = stmt.execute([
                &bucket.bid.unwrap(),
                &event.id as &dyn ToSql,
                &starttime_nanos,
                &endtime_nanos,
                &data as &dyn ToSql,
            ]);
            match res {
                Ok(_) => {
                    self.update_endtime(&mut bucket, event);
                    let rowid = conn.last_insert_rowid();
                    event.id = Some(rowid);
                }
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to insert event: {event:?}, {err}"
                    )));
                }
            };
        }
        Ok(events)
    }

    pub fn delete_events_by_id(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        event_ids: Vec<i64>,
    ) -> Result<(), DatastoreError> {
        let mut bucket = self.get_bucket(bucket_id)?;
        let mut stmt = match conn.prepare_cached(
            "
                DELETE FROM events
                WHERE bucketrow = ?1 AND id = ?2",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_events SQL statement: {err}"
                )))
            }
        };
        for id in event_ids {
            let res = stmt.execute([&bucket.bid.unwrap(), &id as &dyn ToSql]);
            match res {
                Ok(_) => {}
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to delete event with id {id} in bucket {bucket_id}: {err:?}"
                    )));
                }
            };
        }
        // Deletion can shrink (or empty) the bucket's time span. prefer_endtime_index
        // estimates selectivity from these cached bounds, so refresh them here -
        // otherwise a bucket trimmed down after this call keeps the stale, wider
        // span and can misjudge which index actually wins.
        self.refresh_bucket_bounds(conn, &mut bucket)?;
        Ok(())
    }

    /// Recompute a bucket's cached start/end bounds from the events table and
    /// update the bucket cache. Used after deletions, which unlike inserts can
    /// shrink the bucket's span rather than only extend it.
    fn refresh_bucket_bounds(
        &mut self,
        conn: &Connection,
        bucket: &mut Bucket,
    ) -> Result<(), DatastoreError> {
        let mut stmt = match conn
            .prepare_cached("SELECT min(starttime), max(endtime) FROM events WHERE bucketrow = ?1")
        {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare refresh_bucket_bounds SQL statement: {err}"
                )))
            }
        };
        let (start_ns, end_ns): (Option<i64>, Option<i64>) = match stmt
            .query_row([&bucket.bid.unwrap() as &dyn ToSql], |row| {
                Ok((row.get(0)?, row.get(1)?))
            }) {
            Ok(bounds) => bounds,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to query refresh_bucket_bounds SQL statement: {err:?}"
                )))
            }
        };
        bucket.metadata.start = start_ns.map(_datetime_from_nanos);
        bucket.metadata.end = end_ns.map(_datetime_from_nanos);
        self.buckets_cache.insert(bucket.id.clone(), bucket.clone());
        Ok(())
    }

    // TODO: Function for deleting events by timerange with limit

    fn update_endtime(&mut self, bucket: &mut Bucket, event: &Event) {
        let mut update = false;
        /* Potentially update start */
        match bucket.metadata.start {
            None => {
                bucket.metadata.start = Some(event.timestamp);
                update = true;
            }
            Some(current_start) => {
                if current_start > event.timestamp {
                    bucket.metadata.start = Some(event.timestamp);
                    update = true;
                }
            }
        }
        /* Potentially update end */
        let event_endtime = event.calculate_endtime();
        match bucket.metadata.end {
            None => {
                bucket.metadata.end = Some(event_endtime);
                update = true;
            }
            Some(current_end) => {
                if current_end < event_endtime {
                    bucket.metadata.end = Some(event_endtime);
                    update = true;
                }
            }
        }
        /* Update buchets_cache if start or end has been updated */
        if update {
            self.buckets_cache.insert(bucket.id.clone(), bucket.clone());
        }
    }

    pub fn replace_last_event(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        event_id: i64,
        event: &Event,
    ) -> Result<(), DatastoreError> {
        let mut bucket = self.get_bucket(bucket_id)?;

        // Use event ID directly instead of max(endtime) to avoid mismatch with get_events ordering
        let mut stmt = match conn.prepare_cached(
            "
                UPDATE events
                SET starttime = ?2, endtime = ?3, data = ?4
                WHERE bucketrow = ?1 AND id = ?5
            ",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare replace_last_event SQL statement: {err}"
                )))
            }
        };
        let starttime_nanos = event.timestamp.timestamp_nanos_opt().unwrap();
        let duration_nanos = match event.duration.num_nanoseconds() {
            Some(nanos) => nanos,
            None => {
                return Err(DatastoreError::InternalError(
                    "Failed to convert duration to nanoseconds".to_string(),
                ))
            }
        };
        let endtime_nanos = starttime_nanos + duration_nanos;
        let data = serde_json::to_string(&event.data).unwrap();
        match stmt.execute([
            &bucket.bid.unwrap(),
            &starttime_nanos,
            &endtime_nanos,
            &data as &dyn ToSql,
            &event_id,
        ]) {
            Ok(0) => {
                return Err(DatastoreError::InternalError(format!(
                    "replace_last_event matched 0 rows for event_id {event_id} - cache/DB inconsistency"
                )))
            }
            Ok(_) => self.update_endtime(&mut bucket, event),
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to execute replace_last_event SQL statement: {err}"
                )))
            }
        };
        Ok(())
    }

    pub fn heartbeat(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        heartbeat: Event,
        pulsetime: f64,
        last_heartbeat: &mut HashMap<String, Option<Event>>,
    ) -> Result<Event, DatastoreError> {
        self.get_bucket(bucket_id)?;
        if !last_heartbeat.contains_key(bucket_id) {
            last_heartbeat.insert(bucket_id.to_string(), None);
        }
        let last_event = match last_heartbeat.remove(bucket_id).unwrap() {
            // last heartbeat is in cache
            Some(last_event) => last_event,
            None => {
                // last heartbeat was not in cache, fetch from DB
                let mut last_event_vec = self.get_events(conn, bucket_id, None, None, Some(1))?;
                match last_event_vec.pop() {
                    Some(last_event) => last_event,
                    None => {
                        // There was no last event, insert and return
                        let mut inserted = self.insert_events(conn, bucket_id, vec![heartbeat])?;
                        return Ok(inserted.pop().unwrap());
                    }
                }
            }
        };
        let inserted_heartbeat = match aw_transform::heartbeat(&last_event, &heartbeat, pulsetime) {
            Some(mut merged_heartbeat) => {
                debug!("Merged heartbeat successfully");
                // Use the event ID from last_event to ensure we update the correct row
                let event_id = last_event.id.ok_or_else(|| {
                    DatastoreError::InternalError("last_event has no ID".to_string())
                })?;
                self.replace_last_event(conn, bucket_id, event_id, &merged_heartbeat)?;
                // Preserve the event ID on the cached heartbeat so subsequent
                // heartbeats can look it up for replace_last_event
                merged_heartbeat.id = Some(event_id);
                merged_heartbeat
            }
            None => {
                debug!("Failed to merge heartbeat");
                // insert_events sets the ID on the events in the vec, so use the
                // returned event (with ID) instead of the original heartbeat
                let mut inserted = self.insert_events(conn, bucket_id, vec![heartbeat])?;
                inserted.pop().unwrap()
            }
        };
        last_heartbeat.insert(bucket_id.to_string(), Some(inserted_heartbeat.clone()));
        Ok(inserted_heartbeat)
    }

    pub fn get_event(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        event_id: i64,
    ) -> Result<Event, DatastoreError> {
        let bucket = self.get_bucket(bucket_id)?;

        let mut stmt = match conn.prepare_cached(
            "
                SELECT id, starttime, endtime, data
                FROM events
                WHERE bucketrow = ?1
                    AND id = ?2
                LIMIT 1
            ;",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_event SQL statement: {err}"
                )))
            }
        };

        let row = match stmt.query_row([&bucket.bid.unwrap(), &event_id], |row| {
            parse_event_row(row, None)
        }) {
            Ok(rows) => rows,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(DatastoreError::NoSuchEvent(bucket_id.to_string(), event_id))
            }
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to map get_event SQL statement: {err}"
                )))
            }
        };

        Ok(row)
    }

    fn get_events_inner(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
        clip_to_query_range: bool,
    ) -> Result<Vec<Event>, DatastoreError> {
        let bucket = self.get_bucket(bucket_id)?;

        let mut list = Vec::new();

        if filter_excludes_everything(starttime_opt, endtime_opt) {
            return Ok(list);
        }
        let starttime_filter_ns: i64 = starttime_opt.map_or(0, filter_nanos);
        let endtime_filter_ns: i64 = endtime_opt.map_or(i64::MAX, filter_nanos);
        if starttime_filter_ns > endtime_filter_ns {
            warn!("Starttime in event query was lower than endtime!");
            return Ok(list);
        }
        let limit = match limit_opt {
            Some(l) => l as i64,
            None => -1,
        };

        let source = events_source(
            self.db_version,
            prefer_endtime_index(&bucket, starttime_filter_ns, endtime_filter_ns, limit_opt),
        );
        let sql = format!(
            "SELECT id, starttime, endtime, data
             FROM {source}
             WHERE bucketrow = ?1 AND endtime >= ?2 AND starttime <= ?3
             ORDER BY starttime DESC, endtime ASC, id ASC LIMIT ?4"
        );
        let mut stmt = match conn.prepare_cached(&sql) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_events SQL statement: {err}"
                )))
            }
        };

        let rows = match stmt.query_map(
            [
                &bucket.bid.unwrap(),
                &starttime_filter_ns,
                &endtime_filter_ns,
                &limit,
            ],
            |row| {
                parse_event_row(
                    row,
                    clip_to_query_range.then_some((starttime_filter_ns, endtime_filter_ns)),
                )
            },
        ) {
            Ok(rows) => rows,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to map get_events SQL statement: {err}"
                )))
            }
        };
        for row in rows {
            match row {
                Ok(event) => list.push(event),
                Err(err) => warn!("Corrupt event in bucket {}: {}", bucket_id, err),
            };
        }

        Ok(list)
    }

    pub fn get_events(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        self.get_events_inner(conn, bucket_id, starttime_opt, endtime_opt, limit_opt, true)
    }

    pub fn get_events_unclipped(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        self.get_events_inner(
            conn,
            bucket_id,
            starttime_opt,
            endtime_opt,
            limit_opt,
            false,
        )
    }

    pub fn get_event_count(
        &self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
    ) -> Result<i64, DatastoreError> {
        let bucket = self.get_bucket(bucket_id)?;

        if filter_excludes_everything(starttime_opt, endtime_opt) {
            return Ok(0);
        }
        let starttime_filter_ns: i64 = starttime_opt.map_or(0, filter_nanos);
        let endtime_filter_ns: i64 = endtime_opt.map_or(i64::MAX, filter_nanos);
        // Same bound check as get_events_inner, so a zero-length range counts the
        // events that get_events returns for it.
        if starttime_filter_ns > endtime_filter_ns {
            warn!("Endtime in event count query was lower than starttime!");
            return Ok(0);
        }

        let source = events_source(
            self.db_version,
            prefer_endtime_index(&bucket, starttime_filter_ns, endtime_filter_ns, None),
        );
        let sql = format!(
            "SELECT count(*) FROM {source}
             WHERE bucketrow = ?1 AND endtime >= ?2 AND starttime <= ?3"
        );
        let mut stmt = match conn.prepare_cached(&sql) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_event_count SQL statement: {err}",
                )))
            }
        };

        let count = match stmt.query_row(
            [
                &bucket.bid.unwrap(),
                &starttime_filter_ns,
                &endtime_filter_ns,
            ],
            |row| row.get(0),
        ) {
            Ok(count) => count,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to query get_event_count SQL statement: {err}"
                )))
            }
        };

        Ok(count)
    }

    pub fn insert_key_value(
        &self,
        conn: &Connection,
        key: &str,
        data: &str,
    ) -> Result<(), DatastoreError> {
        let mut stmt = match conn.prepare_cached(
            "
                INSERT OR REPLACE INTO key_value(key, value, last_modified)
                VALUES (?1, ?2, ?3)",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_value SQL statement: {err}"
                )))
            }
        };
        let timestamp = Utc::now().timestamp();
        #[allow(clippy::expect_fun_call)]
        stmt.execute(params![key, data, &timestamp])
            .expect(&format!("Failed to insert key-value pair: {key}"));
        Ok(())
    }

    pub fn delete_key_value(&self, conn: &Connection, key: &str) -> Result<(), DatastoreError> {
        conn.execute("DELETE FROM key_value WHERE key = ?1", [key])
            .expect("Error deleting value from database");
        Ok(())
    }

    pub fn get_key_value(&self, conn: &Connection, key: &str) -> Result<String, DatastoreError> {
        let mut stmt = match conn.prepare_cached(
            "
                SELECT * FROM key_value WHERE KEY = ?1",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_value SQL statement: {err}"
                )))
            }
        };

        match stmt.query_row([key], |row| row.get(1)) {
            Ok(result) => Ok(result),
            Err(err) => match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    Err(DatastoreError::NoSuchKey(key.to_string()))
                }
                _ => Err(DatastoreError::InternalError(format!(
                    "Get value query failed for key {key}"
                ))),
            },
        }
    }

    pub fn get_key_values(
        &self,
        conn: &Connection,
        pattern: &str,
    ) -> Result<HashMap<String, String>, DatastoreError> {
        let mut stmt =
            match conn.prepare_cached("SELECT key, value FROM key_value WHERE key LIKE ?") {
                Ok(stmt) => stmt,
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to prepare get_value SQL statement: {err}"
                    )))
                }
            };

        let mut output = HashMap::<String, String>::new();
        // Rusqlite's get wants index and item type as parameters.
        let result = stmt.query_map([pattern], |row| {
            Ok((row.get::<usize, String>(0)?, row.get::<usize, String>(1)?))
        });
        match result {
            Ok(settings) => {
                for row in settings {
                    // Unwrap to String or panic on SQL row if type is invalid. Can't happen with a
                    // properly initialized table.
                    let (key, value) = row.unwrap();
                    // Only return keys starting with "settings.".
                    if !key.starts_with("settings.") {
                        continue;
                    }
                    output.insert(key, value);
                }
                Ok(output)
            }
            Err(err) => match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(output),
                _ => Err(DatastoreError::InternalError(
                    "Failed to get settings".to_string(),
                )),
            },
        }
    }

    /// Renames a bucket from `old_id` to `new_id`.
    /// Events are left untouched because they reference the integer row ID, not the name.
    /// Returns `NoSuchBucket` if `old_id` does not exist, or `BucketAlreadyExists` if
    /// `new_id` is already taken.
    pub fn rename_bucket(
        &mut self,
        conn: &Connection,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), DatastoreError> {
        if !self.buckets_cache.contains_key(old_id) {
            return Err(DatastoreError::NoSuchBucket(old_id.to_string()));
        }
        if self.buckets_cache.contains_key(new_id) {
            return Err(DatastoreError::BucketAlreadyExists(new_id.to_string()));
        }

        // The destination check lives in the UPDATE: another connection may have
        // created `new_id` since this cache was loaded, and the (device_id, name)
        // constraint alone would let a differing device through.
        match conn.execute(
            "UPDATE buckets SET name = ?1 WHERE name = ?2
             AND NOT EXISTS (SELECT 1 FROM buckets WHERE name = ?1)",
            [new_id, old_id],
        ) {
            Ok(0) => {
                let old_exists: bool = conn
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM buckets WHERE name = ?1)",
                        [old_id],
                        |row| row.get(0),
                    )
                    .map_err(|err| {
                        DatastoreError::InternalError(format!(
                            "Failed to rename bucket '{}' to '{}': {err}",
                            old_id, new_id
                        ))
                    })?;
                if old_exists {
                    Err(DatastoreError::BucketAlreadyExists(new_id.to_string()))
                } else {
                    Err(DatastoreError::NoSuchBucket(old_id.to_string()))
                }
            }
            Ok(_) => {
                info!("Renamed bucket '{}' to '{}'", old_id, new_id);
                // Update the in-memory cache: remove the old entry and re-insert under the new id.
                if let Some(mut bucket) = self.buckets_cache.remove(old_id) {
                    bucket.id = new_id.to_string();
                    self.buckets_cache.insert(new_id.to_string(), bucket);
                }
                Ok(())
            }
            Err(err) => Err(DatastoreError::InternalError(format!(
                "Failed to rename bucket '{}' to '{}': {err}",
                old_id, new_id
            ))),
        }
    }

    /// Migrates all buckets whose hostname is "unknown" or "Unknown" to `new_hostname`.
    /// Events are left untouched; only the bucket metadata is updated.
    /// Returns the number of buckets that were updated.
    pub fn migrate_hostname(
        &mut self,
        conn: &Connection,
        new_hostname: &str,
    ) -> Result<usize, DatastoreError> {
        info!(
            "Migrating hostname from 'unknown'/'Unknown' to '{}'",
            new_hostname
        );

        let updated = match conn.execute(
            "UPDATE buckets SET hostname = ?1 WHERE hostname = 'unknown' OR hostname = 'Unknown'",
            [new_hostname],
        ) {
            Ok(n) => n,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to migrate hostname: {err}"
                )))
            }
        };

        if updated > 0 {
            info!("Migrated hostname for {} bucket(s)", updated);
            // Refresh the in-memory cache so callers see the new hostnames immediately.
            self.get_stored_buckets(conn)?;
        } else {
            info!("No buckets with hostname 'unknown'/'Unknown' found; nothing to migrate");
        }

        Ok(updated)
    }

    /// Migrates all buckets whose name starts with `aw-watcher-android-test` to use
    /// `aw-watcher-android` instead. This covers the old production bucket naming
    /// convention (e.g. `aw-watcher-android-test_phone` → `aw-watcher-android_phone`).
    ///
    /// If the destination already exists, disjoint legacy events are moved into it.
    /// Events that overlap a destination or another legacy event stay in the legacy
    /// bucket so the migration cannot create overlapping activity records. The legacy
    /// bucket is deleted only after it is empty.
    /// Returns the number of legacy buckets that were fully renamed or merged.
    pub fn migrate_test_bucket_names(
        &mut self,
        conn: &Connection,
    ) -> Result<usize, DatastoreError> {
        const OLD_PREFIX: &str = "aw-watcher-android-test";
        const NEW_PREFIX: &str = "aw-watcher-android";

        info!("Migrating '{OLD_PREFIX}' bucket names to '{NEW_PREFIX}'");
        let legacy_ids: Vec<String> = self
            .buckets_cache
            .keys()
            .filter(|id| id.starts_with(OLD_PREFIX))
            .cloned()
            .collect();
        let mut migrated = 0;
        let mut cache_dirty = false;

        for old_id in legacy_ids {
            let new_id = old_id.replacen(OLD_PREFIX, NEW_PREFIX, 1);
            if let Some(new_bucket) = self.buckets_cache.get(&new_id).cloned() {
                let old_bucket = self
                    .buckets_cache
                    .get(&old_id)
                    .cloned()
                    .ok_or_else(|| DatastoreError::NoSuchBucket(old_id.clone()))?;

                // Move only events that do not overlap the destination or another
                // legacy event. A single overlapping cutover heartbeat must not strand
                // years of disjoint history in the legacy bucket (ActivityWatch/aw-android#243).
                //
                // Overlaps are found with one sorted scan over both buckets instead of a
                // correlated `NOT EXISTS` subquery per legacy event: that subquery had no
                // lower bound on `starttime`, so it scanned every earlier event again for
                // each row (O(n^2)). With a couple of years of Android history it kept the
                // single datastore worker busy for hours, which blanked the web UI and
                // produced ANRs in every main-thread datastore call (aw-android#261).
                let movable = movable_legacy_event_ids(conn, old_bucket.bid, new_bucket.bid)
                    .map_err(|err| {
                        DatastoreError::InternalError(format!(
                            "Failed to find mergeable events in '{}': {err}",
                            old_id
                        ))
                    })?;
                move_events_to_bucket(conn, new_bucket.bid, &movable).map_err(|err| {
                    DatastoreError::InternalError(format!(
                        "Failed to merge bucket '{}' into '{}': {err}",
                        old_id, new_id
                    ))
                })?;
                cache_dirty = true;

                let remaining: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE bucketrow = ?1",
                        [old_bucket.bid],
                        |row| row.get(0),
                    )
                    .map_err(|err| {
                        DatastoreError::InternalError(format!(
                            "Failed to count leftover events in '{}': {err}",
                            old_id
                        ))
                    })?;
                if remaining == 0 {
                    conn.execute("DELETE FROM buckets WHERE id = ?1", [old_bucket.bid])
                        .map_err(|err| {
                            DatastoreError::InternalError(format!(
                                "Failed to remove merged bucket '{}': {err}",
                                old_id
                            ))
                        })?;
                    info!("Merged legacy bucket '{}' into '{}'", old_id, new_id);
                    migrated += 1;
                } else {
                    warn!(
                        "Partially merged '{}' into '{}'; {} overlapping event(s) remain in the legacy bucket",
                        old_id, new_id, remaining
                    );
                }
            } else {
                conn.execute(
                    "UPDATE buckets SET name = ?1 WHERE name = ?2",
                    [&new_id, &old_id],
                )
                .map_err(|err| {
                    DatastoreError::InternalError(format!(
                        "Failed to rename bucket '{}' to '{}': {err}",
                        old_id, new_id
                    ))
                })?;
                info!("Renamed legacy bucket '{}' to '{}'", old_id, new_id);
                migrated += 1;
                cache_dirty = true;
            }
        }

        if cache_dirty {
            self.get_stored_buckets(conn)?;
        }
        Ok(migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_v7_migration_restores_foreign_keys_and_rolls_back() {
        let conn = Connection::open_in_memory().unwrap();
        _migrate_v0_to_v1(&conn);
        _migrate_v1_to_v2(&conn);
        _migrate_v2_to_v3(&conn);
        _migrate_v3_to_v4(&conn);
        _migrate_v4_to_v5(&conn);
        _migrate_v5_to_v6(&conn);
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        // Force failure after the replacement table is created, before COMMIT.
        conn.execute_batch("ALTER TABLE buckets RENAME COLUMN data TO missing_data;")
            .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            _migrate_v6_to_v7(&conn);
        }));
        assert!(result.is_err());
        assert!(conn.is_autocommit(), "failed migration must roll back");
        let foreign_keys: bool = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert!(
            foreign_keys,
            "failure must not leave FK enforcement disabled"
        );
        assert_eq!(_get_db_version(&conn), 6);
        let replacements: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'buckets_v7'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(replacements, 0);
        assert!(conn
            .execute(
                "INSERT INTO events (bucketrow, starttime, endtime, data) VALUES (999, 0, 1, '{}')",
                [],
            )
            .is_err());
        // The same borrowed connection can retry once the cause is repaired.
        conn.execute_batch("ALTER TABLE buckets RENAME COLUMN missing_data TO data;")
            .unwrap();
        _migrate_v6_to_v7(&conn);
        assert_eq!(_get_db_version(&conn), 7);
    }

    fn v6_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        _migrate_v0_to_v1(&conn);
        _migrate_v1_to_v2(&conn);
        _migrate_v2_to_v3(&conn);
        _migrate_v3_to_v4(&conn);
        _migrate_v4_to_v5(&conn);
        _migrate_v5_to_v6(&conn);
        conn
    }

    fn insert_v6_bucket(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO buckets (name, type, client, hostname, created, data)
             VALUES (?1, 't', 'c', 'h', '2026-01-01T00:00:00+00:00', '{}')",
            [name],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn v7_migration_preserves_archival_and_current_data_separately() {
        let conn = v6_connection();
        for (name, archived) in [("archived", Some("broken legacy bytes")), ("null", None)] {
            insert_v6_bucket(&conn, name);
            conn.execute(
                "UPDATE buckets SET data = '{\"current\":true}', data_deprecated = ?1 WHERE name = ?2",
                params![archived, name],
            )
            .unwrap();
        }
        _migrate_v6_to_v7(&conn);
        for (name, archived) in [("archived", Some("broken legacy bytes")), ("null", None)] {
            let (data, deprecated): (String, Option<String>) = conn
                .query_row(
                    "SELECT data, data_deprecated FROM buckets WHERE name = ?1",
                    [name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(data, r#"{"current":true}"#);
            assert_eq!(deprecated.as_deref(), archived);
        }
    }

    #[test]
    fn v7_migration_keeps_autoincrement_high_water_mark() {
        let conn = v6_connection();
        insert_v6_bucket(&conn, "a");
        let deleted = insert_v6_bucket(&conn, "b");
        conn.execute("DELETE FROM buckets WHERE id = ?1", [deleted])
            .unwrap();
        _migrate_v6_to_v7(&conn);
        let next = conn
            .execute(
                "INSERT INTO buckets (name, type, client, hostname, created) VALUES ('c', 't', 'c', 'h', 'now')",
                [],
            )
            .map(|_| conn.last_insert_rowid())
            .unwrap();
        assert_eq!(next, deleted + 1, "deleted bucket ids must not be reused");
    }

    #[test]
    fn v7_migration_keeps_sequence_when_table_is_empty() {
        let conn = v6_connection();
        let only = insert_v6_bucket(&conn, "a");
        conn.execute("DELETE FROM buckets", []).unwrap();
        _migrate_v6_to_v7(&conn);
        let next = conn
            .execute(
                "INSERT INTO buckets (name, type, client, hostname, created) VALUES ('c', 't', 'c', 'h', 'now')",
                [],
            )
            .map(|_| conn.last_insert_rowid())
            .unwrap();
        assert_eq!(next, only + 1);
    }

    #[test]
    fn v7_migration_rechecks_version_after_taking_the_lock() {
        let conn = v6_connection();
        insert_v6_bucket(&conn, "a");
        _migrate_v6_to_v7(&conn);
        conn.execute("UPDATE buckets SET device_id = 'peer-abc'", [])
            .unwrap();
        // A second opener read v6 before the first finished; once it holds the
        // lock the migration must be a no-op, not rebuild with 'local'.
        _migrate_v6_to_v7(&conn);
        let device: String = conn
            .query_row("SELECT device_id FROM buckets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(device, "peer-abc");
        assert_eq!(_get_db_version(&conn), 7);
    }

    /// Minimal legacy (Python aw-server/peewee) sqlite db fixture: one
    /// bucket, one event. Schema mirrors what `legacy_import` expects.
    fn write_fixture_legacy_db(path: &std::path::Path) {
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
        conn.execute(
            "INSERT INTO eventmodel (bucket_id, timestamp, duration, datastr) VALUES (?1, ?2, ?3, ?4)",
            params![bucket_key, "2026-01-01 10:00:00+00:00", 5.0, "{}"],
        )
        .unwrap();
    }

    #[test]
    fn ensure_legacy_import_skips_after_first_init_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy_path = tmp.path().join("legacy.db");
        write_fixture_legacy_db(&legacy_path);

        let conn = Connection::open_in_memory().unwrap();
        let mut ds = DatastoreInstance::new(&conn, true).unwrap();

        // First call: first_init is true (freshly created datastore), so the
        // import runs automatically.
        assert!(ds
            .ensure_legacy_import(&conn, Some(legacy_path.to_str().unwrap()), false)
            .unwrap());
        assert_eq!(ds.get_buckets().len(), 1);

        // Second call without force: first_init is now false (this is
        // exactly ActivityWatch/aw-server-rust#546 — a restart of an
        // already-initialized datastore). Must skip, not re-run or panic.
        assert!(!ds
            .ensure_legacy_import(&conn, Some(legacy_path.to_str().unwrap()), false)
            .unwrap());

        // With force=true (`aw-server --import-legacy`): runs again. The
        // fixture is unchanged, so this also proves the merge path is
        // idempotent instead of panicking on BucketAlreadyExists.
        assert!(ds
            .ensure_legacy_import(&conn, Some(legacy_path.to_str().unwrap()), true)
            .unwrap());
        let bucket_id = ds.get_buckets().keys().next().unwrap().clone();
        let events = ds.get_events(&conn, &bucket_id, None, None, None).unwrap();
        assert_eq!(
            events.len(),
            1,
            "forced re-import must not duplicate events"
        );
    }
}

#[cfg(test)]
mod inference_tests {
    use super::*;

    /// `_infer_db_version` must name the exact version, not just a version the
    /// migrations would happen to repair: a v4 schema inferred as v5 would skip
    /// the v4->v5 migration. `_create_tables(conn, 0)` builds the newest schema.
    #[test]
    fn infer_db_version_identifies_current_schema() {
        let conn = Connection::open_in_memory().unwrap();
        _create_tables(&conn, 0);
        assert_eq!(_infer_db_version(&conn), NEWEST_DB_VERSION);
    }

    #[test]
    fn infer_db_version_zero_for_empty_db() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(_infer_db_version(&conn), 0);
    }
}
