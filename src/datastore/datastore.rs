use std::fmt;
use std::thread;
use std::collections::HashMap;

use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;
use chrono::Duration;

use rusqlite::Connection;
use rusqlite::DropBehavior;
use rusqlite::TransactionBehavior;

use serde_json::value::Value;

use crossbeam_requests;

use crate::models::Bucket;
use crate::models::BucketMetadata;
use crate::models::Event;
use crate::transform;

use rusqlite::types::ToSql;

/* TODO
 * - Replace some unwraps with MpscError
 * - Improve DatastoreError
 */

#[derive(Debug,Clone)]
pub enum Responses {
    Empty(),
    Bucket(Bucket),
    BucketMap(HashMap<String, Bucket>),
    Event(Event),
    EventList(Vec<Event>),
    Count(i64)
}

pub enum DatastoreMethod {
    Memory(),
    File(String),
}

#[derive(Debug,Clone)]
pub enum DatastoreError {
    NoSuchBucket,
    BucketAlreadyExists,
    MpscError,
    InternalError(String),
}

#[derive(Debug,Clone)]
pub enum Commands {
    CreateBucket(Bucket),
    DeleteBucket(String),
    GetBucket(String),
    GetBuckets(),
    InsertEvents(String, Vec<Event>),
    Heartbeat(String, Event, f64),
    GetEvents(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<u64>),
    GetEventCount(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>),
}

type RequestSender = crossbeam_requests::RequestSender<Commands, Result<Responses, DatastoreError>>;
type RequestReceiver = crossbeam_requests::RequestReceiver<Commands, Result<Responses, DatastoreError>>;

#[derive(Clone)]
pub struct Datastore {
    requester: RequestSender,
}

impl fmt::Debug for Datastore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Datastore()")
    }
}

struct DatastoreWorker {
    responder: RequestReceiver,
    quit: bool
}

/*
 * ### Database version changelog ###
 * 0: Uninitialized database
 * 1: Initialized database
 * 2: Added 'data' field to 'buckets' table
 */
fn _create_tables(conn: &Connection) {
    /* get DB version */
    let version : i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))
.unwrap();
    info!("DB version: {}", version);

    if version < 1 {
        _migrate_v0_to_v1(conn);
    }

    if version < 2 {
        _migrate_v1_to_v2(conn);
    }

    if version < 3 {
        _migrate_v2_to_v3(conn);
    }
}

fn _migrate_v0_to_v1(conn: &Connection) {
    /* Set up bucket table */
    conn.execute("
        CREATE TABLE IF NOT EXISTS buckets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            type TEXT NOT NULL,
            client TEXT NOT NULL,
            hostname TEXT NOT NULL,
            created TEXT NOT NULL
        )", &[] as &[&dyn ToSql]).expect("Failed to create buckets table");

    /* Set up index for bucket table */
    conn.execute("CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)", &[] as &[&dyn ToSql])
        .expect("Failed to create buckets index");

    /* Set up events table */
    conn.execute("
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bucketrow INTEGER NOT NULL,
            starttime INTEGER NOT NULL,
            endtime INTEGER NOT NULL,
            data TEXT NOT NULL,
            FOREIGN KEY (bucketrow) REFERENCES buckets(id)
        )", &[] as &[&dyn ToSql]).expect("Failed to create events table");

    /* Set up index for events table */
    conn.execute("CREATE INDEX IF NOT EXISTS events_bucketrow_index ON events(bucketrow)", &[] as &[&dyn ToSql])
        .expect("Failed to create events_bucketrow index");
    conn.execute("CREATE INDEX IF NOT EXISTS events_starttime_index ON events(starttime)", &[] as &[&dyn ToSql])
        .expect("Failed to create events_starttime index");
    conn.execute("CREATE INDEX IF NOT EXISTS events_endtime_index ON events(endtime)", &[] as &[&dyn ToSql])
        .expect("Failed to create events_endtime index");

    /* Update database version */
    conn.pragma_update(None, "user_version", &1).expect("Failed to update database version!");
}

fn _migrate_v1_to_v2(conn: &Connection) {
    info!("Upgrading database to v2, adding data field to buckets");
    conn.execute("ALTER TABLE buckets ADD COLUMN data TEXT DEFAULT '{}';", &[] as &[&dyn ToSql])
        .expect("Failed to upgrade database when adding data field to buckets");

    conn.pragma_update(None, "user_version", &2).expect("Failed to update database version!");
}

fn _migrate_v2_to_v3(conn: &Connection) {
    // For details about why this migration was necessary, see: https://github.com/ActivityWatch/aw-server-rust/pull/52
    info!("Upgrading database to v3, replacing the broken data field for buckets");

    // Rename column, marking it as deprecated
    match conn.execute("ALTER TABLE buckets RENAME COLUMN data TO data_deprecated;", &[] as &[&dyn ToSql]) {
        Ok(_) => (),
        // This error is okay, it still has the intended effects
        Err(rusqlite::Error::ExecuteReturnedResults) => (),
        Err(e) => panic!("Unexpected error: {:?}", e)
    };

    // Create new correct column
    conn.execute("ALTER TABLE buckets ADD COLUMN data TEXT NOT NULL DEFAULT '{}';", &[] as &[&dyn ToSql])
        .expect("Failed to upgrade database when adding new data field to buckets");

    conn.pragma_update(None, "user_version", &3).expect("Failed to update database version!");
}

struct DatastoreInstance {
    commit: bool,
    uncommited_events: u64,
    buckets_cache: HashMap<String, Bucket>,
}

impl DatastoreWorker {
    pub fn new(responder: crossbeam_requests::RequestReceiver<Commands, Result<Responses, DatastoreError>>) -> Self {
        DatastoreWorker {
            responder: responder,
            quit: false
        }
    }

    fn work_loop(&mut self, method: DatastoreMethod) -> () {
        let mut conn = match method {
            DatastoreMethod::Memory() => Connection::open_in_memory()
                .expect("Failed to create in-memory datastore"),
            DatastoreMethod::File(path) => Connection::open(path)
                .expect("Failed to create datastore")
        };
        _create_tables(&conn);
        let mut ds = DatastoreInstance {
            commit: false,
            uncommited_events: 0,
            buckets_cache: HashMap::new()
        };
        let mut last_heartbeat = HashMap::new();
        match ds.get_stored_buckets(&conn) {
            Ok(_) => (),
            Err(err) => panic!("Failed to initially load buckets from datastore! {:?}", err)
        }
        loop {
            let mut transaction = match conn.transaction_with_behavior(TransactionBehavior::Exclusive) {
                Ok(transaction) => transaction,
                Err(err) => panic!("Unable to take exclusive lock on SQLite database! {}", err)
            };
            transaction.set_drop_behavior(DropBehavior::Commit);
            loop {
                let (request, response_sender) = match self.responder.poll() {
                    Ok((req, res_sender)) => (req, res_sender),
                    Err(_) => { // All references to responder is gone, quit
                        info!("DB worker quitting");
                        self.quit = true;
                        break;
                    }
                };
                let response = match request {
                    Commands::CreateBucket(bucket) => {
                        match ds.create_bucket(&transaction, bucket) {
                            Ok(_) => Ok(Responses::Empty()),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::DeleteBucket(bucketname) => {
                        match ds.delete_bucket(&transaction, &bucketname) {
                            Ok(_) => Ok(Responses::Empty()),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetBucket(bucketname) => {
                        match ds.get_bucket(&bucketname) {
                            Ok(b) => Ok(Responses::Bucket(b)),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetBuckets() => {
                        Ok(Responses::BucketMap(ds.get_buckets()))
                    },
                    Commands::InsertEvents(bucketname, events) => {
                        match ds.insert_events(&transaction, &bucketname, events) {
                            Ok(events) => {
                                last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                                Ok(Responses::EventList(events))
                            },
                            Err(e) => Err(e)
                        }
                    },
                    Commands::Heartbeat(bucketname, event, pulsetime) => {
                        match ds.heartbeat(&transaction, &bucketname, event, pulsetime, &mut last_heartbeat) {
                            Ok(e) => Ok(Responses::Event(e)),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetEvents(bucketname, starttime_opt, endtime_opt, limit_opt) => {
                        match ds.get_events(&transaction, &bucketname, starttime_opt, endtime_opt, limit_opt) {
                            Ok(el) => Ok(Responses::EventList(el)),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetEventCount(bucketname, starttime_opt, endtime_opt) => {
                        match ds.get_event_count(&transaction, &bucketname, starttime_opt, endtime_opt) {
                            Ok(n) => Ok(Responses::Count(n)),
                            Err(e) => Err(e)
                        }
                    },
                };
                response_sender.respond(response);
                if ds.commit || ds.uncommited_events > 100 { break };
            }
            debug!("Commiting DB! Force commit {}, {} uncommited events", ds.commit, ds.uncommited_events);
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {}", err)
            }
            ds.commit = false;
            ds.uncommited_events = 0;
            if self.quit { break };
        }
        info!("DB Worker thread finished");
    }
}

impl DatastoreInstance {
    fn get_stored_buckets(&mut self, conn: &Connection) -> Result <(), DatastoreError> {
        let mut stmt = match conn.prepare("
            SELECT  buckets.id, buckets.name, buckets.type, buckets.client,
                    buckets.hostname, buckets.created,
                    min(events.starttime), max(events.endtime),
                    buckets.data
            FROM buckets
            LEFT OUTER JOIN events ON buckets.id = events.bucketrow
            GROUP BY buckets.id
            ;") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare get_stored_buckets SQL statement: {}", err.to_string())))
        };
        let buckets = match stmt.query_map(&[] as &[&dyn ToSql], |row| {
            let opt_start_ns : Option<i64> = row.get(6)?;
            let opt_start = match opt_start_ns {
                Some(starttime_ns) => {
                    let seconds : i64 = (starttime_ns/1000000000) as i64;
                    let subnanos : u32 = (starttime_ns%1000000000) as u32;
                    Some(DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(seconds, subnanos), Utc))
                },
                None => None
            };

            let opt_end_ns : Option<i64> = row.get(7)?;
            let opt_end = match opt_end_ns {
                Some(endtime_ns) => {
                    let seconds : i64 = (endtime_ns/1000000000) as i64;
                    let subnanos : u32 = (endtime_ns%1000000000) as u32;
                    Some(DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(seconds, subnanos), Utc))
                },
                None => None
            };

            // If data column is not set (possible on old installations), use an empty map as default
            let data_str : String = row.get(8)?;
            let data_json = match serde_json::from_str(&data_str) {
                Ok(data) => data,
                Err(e) => return Err(rusqlite::Error::InvalidColumnName(format!("Failed to parse data to JSON: {:?}", e)))
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
            })
        }) {
            Ok(buckets) => buckets,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to query get_stored_buckets SQL statement: {:?}", err)))
        };
        for bucket in buckets {
            match bucket {
                Ok(b) => {
                    self.buckets_cache.insert(b.id.clone(), b.clone());
                },
                Err(e) => {
                    return Err(DatastoreError::InternalError(format!("Failed to parse bucket from SQLite, database is corrupt! {:?}", e)))
                }
            }
        };
        Ok(())
    }

    fn create_bucket(&mut self, conn: &Connection, mut bucket: Bucket) -> Result<(), DatastoreError> {
        bucket.created = match bucket.created {
            Some(created) => Some(created),
            None => Some(Utc::now())
        };
        let mut stmt = match conn.prepare("
                INSERT INTO buckets (name, type, client, hostname, created, data)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)") {
            Ok(buckets) => buckets,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare create_bucket SQL statement: {}", err.to_string()))),
        };
        let data = serde_json::to_string(&bucket.data).unwrap();
        let res = stmt.execute(&[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created as &dyn ToSql, &data]);

        match res {
            Ok(_) => {
                info!("Created bucket {}", bucket.id);
                // Get and set rowid
                let rowid : i64 = conn.last_insert_rowid();
                bucket.bid = Some(rowid);
                // Take out events from struct before caching
                let events = bucket.events;
                bucket.events = None;
                // Cache bucket
                self.buckets_cache.insert(bucket.id.clone(), bucket.clone());
                self.commit = true;
                // Insert events
                if let Some(events) = events {
                    self.insert_events(conn, &bucket.id, events)?;
                    bucket.events = None;
                }
                Ok(())
            },
            // FIXME: This match is ugly, is it possible to write it in a cleaner way?
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => Err(DatastoreError::InternalError(format!("Failed to execute create_bucket SQL statement: {}", err)))
                },
                _ => Err(DatastoreError::InternalError(format!("Failed to execute create_bucket SQL statement: {}", err)))
            }
        }
    }

    fn delete_bucket(&mut self, conn: &Connection, bucket_id: &str) -> Result<(), DatastoreError>{
        let bucket = (self.get_bucket(&bucket_id))?;
        // Delete all events in bucket
        match conn.execute("DELETE FROM events WHERE bucketrow = ?1", &[&bucket.bid]) {
            Ok(_) => (),
            Err(err) => return Err(DatastoreError::InternalError(err.to_string()))
        }
        // Delete bucket itself
        match conn.execute("DELETE FROM buckets WHERE id = ?1", &[&bucket.bid]) {
            Ok(_) => {
                self.buckets_cache.remove(bucket_id);
                self.commit = true;
                return Ok(());
            },
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => return Err(DatastoreError::InternalError(err.to_string()))
                },
                _ => return Err(DatastoreError::InternalError(err.to_string()))
            }
        }
    }

    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cached_bucket = self.buckets_cache.get(bucket_id);
        match cached_bucket {
            Some(bucket) => Ok(bucket.clone()),
            None => Err(DatastoreError::NoSuchBucket)
        }
    }

    fn get_buckets(&self) -> HashMap<String, Bucket> {
        return self.buckets_cache.clone();
    }


    pub fn insert_events(&mut self, conn: &Connection, bucket_id: &str, mut events: Vec<Event>) -> Result<Vec<Event>, DatastoreError> {
        let mut bucket = self.get_bucket(&bucket_id)?;

        let mut stmt = match conn.prepare("
                INSERT OR REPLACE INTO events(bucketrow, id, starttime, endtime, data)
                VALUES (?1, ?2, ?3, ?4, ?5)") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare insert_events SQL statement: {}", err)))
        };
        for event in &mut events {
            let starttime_nanos = event.timestamp.timestamp_nanos();
            let duration_nanos = match event.duration.num_nanoseconds() {
                Some(nanos) => nanos,
                None => return Err(DatastoreError::InternalError("Failed to convert duration to nanoseconds".to_string()))
            };
            let endtime_nanos = starttime_nanos + duration_nanos;
            let data = serde_json::to_string(&event.data).unwrap();
            let res = stmt.execute(&[&bucket.bid.unwrap(), &event.id as &dyn ToSql, &starttime_nanos, &endtime_nanos, &data as &dyn ToSql]);
            match res {
                Ok(_) => {
                    self.update_endtime(&mut bucket, &event);
                    let rowid = conn.last_insert_rowid();
                    event.id = Some(rowid);
                },
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!("Failed to insert event: {:?}, {}", event, err)));
                }
            };
        }
        Ok(events)
    }

    fn update_endtime(&mut self, bucket: &mut Bucket, event: &Event) {
        let mut update = false;
        /* Potentially update start */
        match bucket.metadata.start {
            None => {
                bucket.metadata.start = Some(event.timestamp.clone());
                update = true;
            },
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
            },
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

    pub fn replace_last_event(&mut self, conn: &Connection, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        let mut bucket = self.get_bucket(&bucket_id)?;

        let mut stmt = match conn.prepare("
                UPDATE events
                SET starttime = ?2, endtime = ?3, data = ?4
                WHERE bucketrow = ?1
                    AND endtime = (SELECT max(endtime) FROM events WHERE bucketrow = ?1)
            ") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare replace_last_event SQL statement: {}", err)))
        };
        let starttime_nanos = event.timestamp.timestamp_nanos();
        let duration_nanos = match event.duration.num_nanoseconds() {
            Some(nanos) => nanos,
            None => return Err(DatastoreError::InternalError("Failed to convert duration to nanoseconds".to_string()))
        };
        let endtime_nanos = starttime_nanos + duration_nanos;
        let data = serde_json::to_string(&event.data).unwrap();
        match stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &data as &dyn ToSql]) {
            Ok(_) => self.update_endtime(&mut bucket, event),
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to execute replace_last_event SQL statement: {}", err)))
        };
        self.uncommited_events += 1;
        Ok(())
    }

    pub fn heartbeat(&mut self, conn: &Connection, bucket_id: &str, heartbeat: Event, pulsetime: f64, last_heartbeat: &mut HashMap<String, Option<Event>>) -> Result<Event, DatastoreError> {
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
        let inserted_heartbeat = match transform::heartbeat(&last_event, &heartbeat, pulsetime) {
            Some(merged_heartbeat) => {
                self.replace_last_event(conn, &bucket_id, &merged_heartbeat)?;
                merged_heartbeat
            },
            None => {
                debug!("Failed to merge heartbeat!");
                self.insert_events(conn, &bucket_id, vec![heartbeat.clone()])?;
                heartbeat
            }
        };
        last_heartbeat.insert(bucket_id.to_string(), Some(inserted_heartbeat.clone()));
        Ok(inserted_heartbeat)
    }

    pub fn get_events(&mut self, conn: &Connection, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        let bucket = self.get_bucket(&bucket_id)?;

        let mut list = Vec::new();

        let starttime_filter_ns : i64 = match starttime_opt {
            Some(dt) => dt.timestamp_nanos(),
            None => 0
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX
        };
        if starttime_filter_ns > endtime_filter_ns {
            warn!("Starttime in event query was lower than endtime!");
            return Ok(list);
        }
        let limit = match limit_opt {
            Some(l) => l as i64,
            None => -1
        };

        let mut stmt = match conn.prepare("
                SELECT id, starttime, endtime, data
                FROM events
                WHERE bucketrow = ?1
                    AND endtime >= ?2
                    AND starttime <= ?3
                ORDER BY starttime DESC
                LIMIT ?4
            ;") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare get_events SQL statement: {}", err)))
        };

        let rows = match stmt.query_map(&[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns, &limit], |row| {
            let id = row.get(0)?;
            let mut starttime_ns : i64 = row.get(1)?;
            let mut endtime_ns : i64 = row.get(2)?;
            let data_str : String = row.get(3)?;

            if starttime_ns < starttime_filter_ns { starttime_ns = starttime_filter_ns }
            if endtime_ns > endtime_filter_ns { endtime_ns = endtime_filter_ns }
            let duration_ns = endtime_ns - starttime_ns;

            let time_seconds : i64 = (starttime_ns/1000000000) as i64;
            let time_subnanos : u32 = (starttime_ns%1000000000) as u32;
            let data : serde_json::map::Map<String, Value> = serde_json::from_str(&data_str).unwrap();

            return Ok(Event {
                id: Some(id),
                timestamp: DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(time_seconds, time_subnanos), Utc),
                duration: Duration::nanoseconds(duration_ns),
                data: data,
            })
        }) {
            Ok(rows) => rows,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to map get_events SQL statement: {}", err)))
        };
        for row in rows {
            match row {
                Ok(event) => list.push(event),
                Err(err) => warn!("Corrupt event in bucket {}: {}", bucket_id, err)
            };
        };
        Ok(list)
    }

    pub fn get_event_count(&self, conn: &Connection, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, DatastoreError> {
        let bucket = self.get_bucket(&bucket_id)?;

        let starttime_filter_ns = match starttime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => 0
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX
        };
        if starttime_filter_ns >= endtime_filter_ns {
            warn!("Endtime in event query was same or lower than starttime!");
            return Ok(0);
        }

        let mut stmt = match conn.prepare("
            SELECT count(*) FROM events
            WHERE bucketrow = ?1
                AND (starttime >= ?2 OR endtime <= ?3)") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare get_event_count SQL statement: {}", err)))
        };

        let count = match stmt.query_row(&[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns],
            |row| row.get(0)
        ) {
            Ok(count) => count,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to query get_event_count SQL statement: {}", err)))
        };

        return Ok(count);
    }
}

impl Datastore {

    pub fn new(dbpath: String) -> Self {
        let method = DatastoreMethod::File(dbpath);
        Datastore::_new_internal(method)
    }

    pub fn new_in_memory() -> Self {
        let method = DatastoreMethod::Memory();
        Datastore::_new_internal(method)
    }

    fn _new_internal(method: DatastoreMethod) -> Self {
        let (requester, responder) = crossbeam_requests::channel::<Commands, Result<Responses, DatastoreError>>();
        let _thread = thread::spawn(move || {
            let mut di = DatastoreWorker::new(responder);
            di.work_loop(method);
        });
        Datastore {
            requester,
        }
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        let cmd = Commands::CreateBucket(bucket.clone());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(_) => Ok(()),
            Err(e) => Err(e)
        }
    }

    pub fn delete_bucket(&self, bucket_id: &str) -> Result<(), DatastoreError>{
        let cmd = Commands::DeleteBucket(bucket_id.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cmd = Commands::GetBucket(bucket_id.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::Bucket(b) => Ok(b),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        let cmd = Commands::GetBuckets();
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::BucketMap(bm) => Ok(bm),
                e => Err(DatastoreError::InternalError(format!("Invalid response: {:?}", e)))
            },
            Err(e) => Err(e)
        }
    }

    pub fn insert_events(&self, bucket_id: &str, events: &[Event]) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Commands::InsertEvents(bucket_id.to_string(), events.to_vec());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::EventList(events) => Ok(events),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn heartbeat(&self, bucket_id: &str, heartbeat: Event, pulsetime: f64) -> Result<Event, DatastoreError> {
        let cmd = Commands::Heartbeat(bucket_id.to_string(), heartbeat, pulsetime);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::Event(e) => return Ok(e),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_events(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Commands::GetEvents(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone(), limit_opt.clone());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::EventList(el) => Ok(el),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_event_count(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, DatastoreError> {
        let cmd = Commands::GetEventCount(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::Count(n) => Ok(n),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }
}
