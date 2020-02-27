use std::collections::HashMap;

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDateTime;
use chrono::Utc;

use rusqlite::Connection;

use serde_json::value::Value;

use aw_models::Bucket;
use aw_models::BucketMetadata;
use aw_models::Event;
use aw_models::KeyValue;

use aw_transform;

use rusqlite::params;
use rusqlite::types::ToSql;

use super::DatastoreError;

fn _get_db_version(conn: &Connection) -> i32 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/*
 * ### Database version changelog ###
 * 0: Uninitialized database
 * 1: Initialized database
 * 2: Added 'data' field to 'buckets' table
 * 3: see: https://github.com/ActivityWatch/aw-server-rust/pull/52
 * 4: Added 'key_value' table for storing key - value pairs
 */
static NEWEST_DB_VERSION: i32 = 4;

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
    conn.pragma_update(None, "user_version", &1)
        .expect("Failed to update database version!");
}

fn _migrate_v1_to_v2(conn: &Connection) {
    info!("Upgrading database to v2, adding data field to buckets");
    conn.execute(
        "ALTER TABLE buckets ADD COLUMN data TEXT DEFAULT '{}';",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to upgrade database when adding data field to buckets");

    conn.pragma_update(None, "user_version", &2)
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
        Err(e) => panic!("Unexpected error: {:?}", e),
    };

    // Create new correct column
    conn.execute(
        "ALTER TABLE buckets ADD COLUMN data TEXT NOT NULL DEFAULT '{}';",
        &[] as &[&dyn ToSql],
    )
    .expect("Failed to upgrade database when adding new data field to buckets");

    conn.pragma_update(None, "user_version", &3)
        .expect("Failed to update database version!");
}

fn _migrate_v3_to_v4(conn: &Connection) {
    info!("Upgrading database to v4, adding table for key-value storage");
    conn.execute(
        "CREATE TABLE key_value (
        key TEXT PRIMARY KEY,
        value TEXT,
        last_modified NUMBER NOT NULL
    );",
        rusqlite::NO_PARAMS,
    )
    .expect("Failed to upgrade db and add key-value storage table");

    conn.pragma_update(None, "user_version", &4)
        .expect("Failed to update database version!");
}

pub struct DatastoreInstance {
    buckets_cache: HashMap<String, Bucket>,
    first_init: bool,
    pub db_version: i32,
}

impl DatastoreInstance {
    pub fn new(
        conn: &Connection,
        migrate_enabled: bool,
    ) -> Result<DatastoreInstance, DatastoreError> {
        let mut first_init = false;
        let db_version = _get_db_version(&conn);

        match migrate_enabled {
            true => first_init = _create_tables(&conn, db_version),
            false => {
                if db_version <= 0 {
                    return Err(DatastoreError::Uninitialized(
                        "Tried to open an uninitialized datastore with migration disabled"
                            .to_string(),
                    ));
                } else if db_version != NEWEST_DB_VERSION {
                    return Err(DatastoreError::OldDbVersion(format!(
                        "\
                        Tried to open an database with an incompatible database version!
                        Database has version {} while the supported version is {}",
                        db_version, NEWEST_DB_VERSION
                    )));
                }
            }
        };

        let mut ds = DatastoreInstance {
            buckets_cache: HashMap::new(),
            first_init,
            db_version,
        };
        ds.get_stored_buckets(&conn)?;
        Ok(ds)
    }

    fn get_stored_buckets(&mut self, conn: &Connection) -> Result<(), DatastoreError> {
        let mut stmt = match conn.prepare(
            "
            SELECT  buckets.id, buckets.name, buckets.type, buckets.client,
                    buckets.hostname, buckets.created,
                    min(events.starttime), max(events.endtime),
                    buckets.data
            FROM buckets
            LEFT OUTER JOIN events ON buckets.id = events.bucketrow
            GROUP BY buckets.id
            ;",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_stored_buckets SQL statement: {}",
                    err.to_string()
                )))
            }
        };
        let buckets = match stmt.query_map(&[] as &[&dyn ToSql], |row| {
            let opt_start_ns: Option<i64> = row.get(6)?;
            let opt_start = match opt_start_ns {
                Some(starttime_ns) => {
                    let seconds: i64 = (starttime_ns / 1000000000) as i64;
                    let subnanos: u32 = (starttime_ns % 1000000000) as u32;
                    Some(DateTime::<Utc>::from_utc(
                        NaiveDateTime::from_timestamp(seconds, subnanos),
                        Utc,
                    ))
                }
                None => None,
            };

            let opt_end_ns: Option<i64> = row.get(7)?;
            let opt_end = match opt_end_ns {
                Some(endtime_ns) => {
                    let seconds: i64 = (endtime_ns / 1000000000) as i64;
                    let subnanos: u32 = (endtime_ns % 1000000000) as u32;
                    Some(DateTime::<Utc>::from_utc(
                        NaiveDateTime::from_timestamp(seconds, subnanos),
                        Utc,
                    ))
                }
                None => None,
            };

            // If data column is not set (possible on old installations), use an empty map as default
            let data_str: String = row.get(8)?;
            let data_json = match serde_json::from_str(&data_str) {
                Ok(data) => data,
                Err(e) => {
                    return Err(rusqlite::Error::InvalidColumnName(format!(
                        "Failed to parse data to JSON: {:?}",
                        e
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
                    "Failed to query get_stored_buckets SQL statement: {:?}",
                    err
                )))
            }
        };
        for bucket in buckets {
            match bucket {
                Ok(b) => {
                    self.buckets_cache.insert(b.id.clone(), b.clone());
                }
                Err(e) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to parse bucket from SQLite, database is corrupt! {:?}",
                        e
                    )))
                }
            }
        }
        Ok(())
    }

    pub fn ensure_legacy_import(&mut self, conn: &Connection) -> Result<bool, ()> {
        use super::legacy_import::legacy_import;
        if !self.first_init {
            return Ok(false);
        } else {
            self.first_init = false;
            match legacy_import(self, &conn) {
                Ok(_) => {
                    info!("Successfully imported legacy database");
                    self.get_stored_buckets(&conn).unwrap();
                    return Ok(true);
                }
                Err(err) => {
                    warn!("Failed to import legacy database: {:?}", err);
                    return Err(());
                }
            };
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
        let mut stmt = match conn.prepare(
            "
                INSERT INTO buckets (name, type, client, hostname, created, data)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        ) {
            Ok(buckets) => buckets,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare create_bucket SQL statement: {}",
                    err.to_string()
                )))
            }
        };
        let data = serde_json::to_string(&bucket.data).unwrap();
        let res = stmt.execute(&[
            &bucket.id,
            &bucket._type,
            &bucket.client,
            &bucket.hostname,
            &bucket.created as &dyn ToSql,
            &data,
        ]);

        match res {
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
                    self.insert_events(conn, &bucket.id, events)?;
                    bucket.events = None;
                }
                Ok(())
            }
            // FIXME: This match is ugly, is it possible to write it in a cleaner way?
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _ } => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => {
                        Err(DatastoreError::BucketAlreadyExists)
                    }
                    _ => Err(DatastoreError::InternalError(format!(
                        "Failed to execute create_bucket SQL statement: {}",
                        err
                    ))),
                },
                _ => Err(DatastoreError::InternalError(format!(
                    "Failed to execute create_bucket SQL statement: {}",
                    err
                ))),
            },
        }
    }

    pub fn delete_bucket(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
    ) -> Result<(), DatastoreError> {
        let bucket = (self.get_bucket(&bucket_id))?;
        // Delete all events in bucket
        match conn.execute("DELETE FROM events WHERE bucketrow = ?1", &[&bucket.bid]) {
            Ok(_) => (),
            Err(err) => return Err(DatastoreError::InternalError(err.to_string())),
        }
        // Delete bucket itself
        match conn.execute("DELETE FROM buckets WHERE id = ?1", &[&bucket.bid]) {
            Ok(_) => {
                self.buckets_cache.remove(bucket_id);
                return Ok(());
            }
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _ } => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => {
                        Err(DatastoreError::BucketAlreadyExists)
                    }
                    _ => return Err(DatastoreError::InternalError(err.to_string())),
                },
                _ => return Err(DatastoreError::InternalError(err.to_string())),
            },
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cached_bucket = self.buckets_cache.get(bucket_id);
        match cached_bucket {
            Some(bucket) => Ok(bucket.clone()),
            None => Err(DatastoreError::NoSuchBucket),
        }
    }

    pub fn get_buckets(&self) -> HashMap<String, Bucket> {
        return self.buckets_cache.clone();
    }

    pub fn insert_events(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        mut events: Vec<Event>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let mut bucket = self.get_bucket(&bucket_id)?;

        let mut stmt = match conn.prepare(
            "
                INSERT OR REPLACE INTO events(bucketrow, id, starttime, endtime, data)
                VALUES (?1, ?2, ?3, ?4, ?5)",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_events SQL statement: {}",
                    err
                )))
            }
        };
        for event in &mut events {
            let starttime_nanos = event.timestamp.timestamp_nanos();
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
            let res = stmt.execute(&[
                &bucket.bid.unwrap(),
                &event.id as &dyn ToSql,
                &starttime_nanos,
                &endtime_nanos,
                &data as &dyn ToSql,
            ]);
            match res {
                Ok(_) => {
                    self.update_endtime(&mut bucket, &event);
                    let rowid = conn.last_insert_rowid();
                    event.id = Some(rowid);
                }
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to insert event: {:?}, {}",
                        event, err
                    )));
                }
            };
        }
        Ok(events)
    }

    pub fn delete_events_by_id(
        &self,
        conn: &Connection,
        bucket_id: &str,
        event_ids: Vec<i64>,
    ) -> Result<(), DatastoreError> {
        let bucket = self.get_bucket(&bucket_id)?;
        let mut stmt = match conn.prepare(
            "
                DELETE FROM events
                WHERE bucketrow = ?1 AND id = ?2",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_events SQL statement: {}",
                    err
                )))
            }
        };
        for id in event_ids {
            let res = stmt.execute(&[&bucket.bid.unwrap(), &id as &dyn ToSql]);
            match res {
                Ok(_) => {}
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!(
                        "Failed to delete event with id {} in bucket {}: {:?}",
                        id, bucket_id, err
                    )));
                }
            };
        }
        Ok(())
    }

    // TODO: Function for deleteing events by timerange with limit

    fn update_endtime(&mut self, bucket: &mut Bucket, event: &Event) {
        let mut update = false;
        /* Potentially update start */
        match bucket.metadata.start {
            None => {
                bucket.metadata.start = Some(event.timestamp.clone());
                update = true;
            }
            Some(current_start) => {
                if current_start > event.timestamp {
                    bucket.metadata.start = Some(event.timestamp.clone());
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
        event: &Event,
    ) -> Result<(), DatastoreError> {
        let mut bucket = self.get_bucket(&bucket_id)?;

        let mut stmt = match conn.prepare(
            "
                UPDATE events
                SET starttime = ?2, endtime = ?3, data = ?4
                WHERE bucketrow = ?1
                    AND endtime = (SELECT max(endtime) FROM events WHERE bucketrow = ?1)
            ",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare replace_last_event SQL statement: {}",
                    err
                )))
            }
        };
        let starttime_nanos = event.timestamp.timestamp_nanos();
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
        match stmt.execute(&[
            &bucket.bid.unwrap(),
            &starttime_nanos,
            &endtime_nanos,
            &data as &dyn ToSql,
        ]) {
            Ok(_) => self.update_endtime(&mut bucket, event),
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to execute replace_last_event SQL statement: {}",
                    err
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
        self.get_bucket(&bucket_id)?;
        if !last_heartbeat.contains_key(bucket_id) {
            last_heartbeat.insert(bucket_id.to_string(), None);
        }
        let last_event = match last_heartbeat.remove(bucket_id).unwrap() {
            // last heartbeat is in cache
            Some(last_event) => last_event,
            None => {
                // last heartbeat was not in cache, fetch from DB
                let mut last_event_vec = self.get_events(conn, &bucket_id, None, None, Some(1))?;
                match last_event_vec.pop() {
                    Some(last_event) => last_event,
                    None => {
                        // There was no last event, insert and return
                        self.insert_events(conn, &bucket_id, vec![heartbeat.clone()])?;
                        return Ok(heartbeat.clone());
                    }
                }
            }
        };
        let inserted_heartbeat = match aw_transform::heartbeat(&last_event, &heartbeat, pulsetime) {
            Some(merged_heartbeat) => {
                self.replace_last_event(conn, &bucket_id, &merged_heartbeat)?;
                merged_heartbeat
            }
            None => {
                debug!("Failed to merge heartbeat!");
                self.insert_events(conn, &bucket_id, vec![heartbeat.clone()])?;
                heartbeat
            }
        };
        last_heartbeat.insert(bucket_id.to_string(), Some(inserted_heartbeat.clone()));
        Ok(inserted_heartbeat)
    }

    pub fn get_events(
        &mut self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let bucket = self.get_bucket(&bucket_id)?;

        let mut list = Vec::new();

        let starttime_filter_ns: i64 = match starttime_opt {
            Some(dt) => dt.timestamp_nanos(),
            None => 0,
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX,
        };
        if starttime_filter_ns > endtime_filter_ns {
            warn!("Starttime in event query was lower than endtime!");
            return Ok(list);
        }
        let limit = match limit_opt {
            Some(l) => l as i64,
            None => -1,
        };

        let mut stmt = match conn.prepare(
            "
                SELECT id, starttime, endtime, data
                FROM events
                WHERE bucketrow = ?1
                    AND endtime >= ?2
                    AND starttime <= ?3
                ORDER BY starttime DESC
                LIMIT ?4
            ;",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_events SQL statement: {}",
                    err
                )))
            }
        };

        let rows = match stmt.query_map(
            &[
                &bucket.bid.unwrap(),
                &starttime_filter_ns,
                &endtime_filter_ns,
                &limit,
            ],
            |row| {
                let id = row.get(0)?;
                let mut starttime_ns: i64 = row.get(1)?;
                let mut endtime_ns: i64 = row.get(2)?;
                let data_str: String = row.get(3)?;

                if starttime_ns < starttime_filter_ns {
                    starttime_ns = starttime_filter_ns
                }
                if endtime_ns > endtime_filter_ns {
                    endtime_ns = endtime_filter_ns
                }
                let duration_ns = endtime_ns - starttime_ns;

                let time_seconds: i64 = (starttime_ns / 1000000000) as i64;
                let time_subnanos: u32 = (starttime_ns % 1000000000) as u32;
                let data: serde_json::map::Map<String, Value> =
                    serde_json::from_str(&data_str).unwrap();

                return Ok(Event {
                    id: Some(id),
                    timestamp: DateTime::<Utc>::from_utc(
                        NaiveDateTime::from_timestamp(time_seconds, time_subnanos),
                        Utc,
                    ),
                    duration: Duration::nanoseconds(duration_ns),
                    data: data,
                });
            },
        ) {
            Ok(rows) => rows,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to map get_events SQL statement: {}",
                    err
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

    pub fn get_event_count(
        &self,
        conn: &Connection,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
    ) -> Result<i64, DatastoreError> {
        let bucket = self.get_bucket(&bucket_id)?;

        let starttime_filter_ns = match starttime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => 0,
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX,
        };
        if starttime_filter_ns >= endtime_filter_ns {
            warn!("Endtime in event query was same or lower than starttime!");
            return Ok(0);
        }

        let mut stmt = match conn.prepare(
            "
            SELECT count(*) FROM events
            WHERE bucketrow = ?1
                AND (starttime >= ?2 OR endtime <= ?3)",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_event_count SQL statement: {}",
                    err
                )))
            }
        };

        let count = match stmt.query_row(
            &[
                &bucket.bid.unwrap(),
                &starttime_filter_ns,
                &endtime_filter_ns,
            ],
            |row| row.get(0),
        ) {
            Ok(count) => count,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to query get_event_count SQL statement: {}",
                    err
                )))
            }
        };

        return Ok(count);
    }

    pub fn insert_key_value(
        &self,
        conn: &Connection,
        key: &str,
        data: &str,
    ) -> Result<(), DatastoreError> {
        let mut stmt = match conn.prepare(
            "
                INSERT OR REPLACE INTO key_value(key, value, last_modified)
                VALUES (?1, ?2, ?3)",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare insert_value SQL statement: {}",
                    err
                )))
            }
        };
        let timestamp = Utc::now().timestamp();
        stmt.execute(params![key, data, &timestamp])
            .expect(&format!("Failed to insert key-value pair: {}", key));
        return Ok(());
    }

    pub fn delete_key_value(&self, conn: &Connection, key: &str) -> Result<(), DatastoreError> {
        conn.execute("DELETE FROM key_value WHERE key = ?1", &[key])
            .expect("Error deleting value from database");
        return Ok(());
    }

    pub fn get_key_value(&self, conn: &Connection, key: &str) -> Result<KeyValue, DatastoreError> {
        let mut stmt = match conn.prepare(
            "
                SELECT * FROM key_value WHERE KEY = ?1",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_value SQL statement: {}",
                    err
                )))
            }
        };

        return match stmt.query_row(&[key], |row| {
            Ok(KeyValue {
                key: row.get(0)?,
                value: row.get(1)?,
                timestamp: Some(DateTime::from_utc(
                    NaiveDateTime::from_timestamp(row.get(2)?, 0),
                    Utc,
                )),
            })
        }) {
            Ok(result) => Ok(result),
            Err(err) => match err {
                rusqlite::Error::QueryReturnedNoRows => Err(DatastoreError::NoSuchKey),
                _ => Err(DatastoreError::InternalError(format!(
                    "Get value query failed for key {}",
                    key
                ))),
            },
        };
    }

    pub fn get_keys_starting(
        &self,
        conn: &Connection,
        pattern: &str,
    ) -> Result<Vec<String>, DatastoreError> {
        let mut stmt = match conn.prepare("SELECT key FROM key_value WHERE key LIKE ?") {
            Ok(stmt) => stmt,
            Err(err) => {
                return Err(DatastoreError::InternalError(format!(
                    "Failed to prepare get_value SQL statement: {}",
                    err
                )))
            }
        };

        let mut output = Vec::<String>::new();
        // Rusqlite's get wants index and item type as parameters.
        let result = stmt.query_map(&[pattern], |row| row.get::<usize, String>(0));
        match result {
            Ok(keys) => {
                for row in keys {
                    // Unwrap to String or panic on SQL row if type is invalid. Can't happen with a
                    // properly initialized table.
                    output.push(row.unwrap());
                }
                Ok(output)
            }
            Err(err) => match err {
                rusqlite::Error::QueryReturnedNoRows => Err(DatastoreError::NoSuchKey),
                _ => Err(DatastoreError::InternalError(format!(
                    "Failed to get key_value rows starting with pattern {}",
                    pattern
                ))),
            },
        }
    }
}
