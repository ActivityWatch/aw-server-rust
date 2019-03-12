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

use mpsc_requests;

use models::Bucket;
use models::Event;
use transform;

use rusqlite::types::ToSql;

/*
 * TODO:
 * - Needs refactoring?
 */

#[derive(Debug,Clone)]
pub enum Responses {
    Empty(),
    Bucket(Bucket),
    BucketMap(HashMap<String, Bucket>),
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
    RequestLockTimeout,
    InternalError(String),
}

#[derive(Debug,Clone)]
pub enum Commands {
    CreateBucket(Bucket),
    DeleteBucket(String),
    GetBucket(String),
    GetBuckets(),
    InsertEvents(String, Vec<Event>),
    ReplaceLastEvent(String, Event),
    Heartbeat(String, Event, f64),
    GetEvents(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<u64>),
    GetEventCount(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>),
}

type Requester = mpsc_requests::Requester<Commands, Result<Responses, DatastoreError>>;
type Responder = mpsc_requests::Responder<Commands, Result<Responses, DatastoreError>>;

#[derive(Clone)]
pub struct Datastore {
    requester: Requester,
}

impl fmt::Debug for Datastore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Datastore()")
    }
}

struct DatastoreWorker {
    responder: Responder,
    quit: bool
}

fn _create_tables(conn: &Connection) {
    /* Set up bucket table and index */
    conn.execute("
        CREATE TABLE IF NOT EXISTS buckets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            type TEXT NOT NULL,
            client TEXT NOT NULL,
            hostname TEXT NOT NULL,
            created TEXT NOT NULL
        )", &[] as &[&ToSql]).expect("Failed to create buckets tabke");
    conn.execute("CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)", &[] as &[&ToSql])
        .expect("Failed to create buckets index");

    conn.execute("
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bucketrow INTEGER NOT NULL,
            starttime INTEGER NOT NULL,
            endtime INTEGER NOT NULL,
            data TEXT NOT NULL,
            FOREIGN KEY (bucketrow) REFERENCES buckets(id)
        )", &[] as &[&ToSql]).expect("Failed to create events table");
    conn.execute("CREATE INDEX IF NOT EXISTS events_bucketrow_index ON events(bucketrow)", &[] as &[&ToSql])
        .expect("Failed to create events_bucketrow index");
    conn.execute("CREATE INDEX IF NOT EXISTS events_starttime_index ON events(starttime)", &[] as &[&ToSql])
        .expect("Failed to create events_starttime index");
    conn.execute("CREATE INDEX IF NOT EXISTS events_endtime_index ON events(endtime)", &[] as &[&ToSql])
        .expect("Failed to create events_endtime index");
}

struct DatastoreInstance {
    commit: bool,
    uncommited_events: u64,
    buckets_cache: HashMap<String, Bucket>,
}

impl DatastoreWorker {
    pub fn new(responder: mpsc_requests::Responder<Commands, Result<Responses, DatastoreError>>) -> Self {
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
                let mut request = match self.responder.poll() {
                    Ok(r) => r,
                    Err(_) => { // All references to responder is gone, quit
                        info!("DB worker quitting");
                        self.quit = true;
                        break;
                    }
                };
                let response = match request.body() {
                    Commands::CreateBucket(bucket) => {
                        match ds.create_bucket(&transaction, bucket) {
                            Ok(_) => Ok(Responses::Empty()),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::DeleteBucket(bucketname) => {
                        match ds.delete_bucket(&transaction, bucketname) {
                            Ok(_) => Ok(Responses::Empty()),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetBucket(bucketname) => {
                        match ds.get_bucket(bucketname) {
                            Ok(b) => Ok(Responses::Bucket(b)),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetBuckets() => {
                        Ok(Responses::BucketMap(ds.get_buckets()))
                    },
                    Commands::InsertEvents(bucketname, events) => {
                        match ds.insert_events(&transaction, bucketname, events) {
                            Ok(_) => {
                                last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                                Ok(Responses::Empty())
                            },
                            Err(e) => Err(e)
                        }
                    },
                    Commands::ReplaceLastEvent(bucketname, event) => {
                        match ds.replace_last_event(&transaction, bucketname, event) {
                            Ok(_) => {
                                last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                                Ok(Responses::Empty())
                            },
                            Err(e) => Err(e)
                        }
                    },
                    Commands::Heartbeat(bucketname, event, pulsetime) => {
                        match ds.heartbeat(&transaction, bucketname, event.clone(), *pulsetime, &mut last_heartbeat) {
                            Ok(_) => Ok(Responses::Empty()),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetEvents(bucketname, starttime_opt, endtime_opt, limit_opt) => {
                        match ds.get_events(&transaction, bucketname, *starttime_opt, *endtime_opt, *limit_opt) {
                            Ok(el) => Ok(Responses::EventList(el)),
                            Err(e) => Err(e)
                        }
                    },
                    Commands::GetEventCount(bucketname, starttime_opt, endtime_opt) => {
                        match ds.get_event_count(&transaction, bucketname, *starttime_opt, *endtime_opt) {
                            Ok(n) => Ok(Responses::Count(n)),
                            Err(e) => Err(e)
                        }
                    },
                };
                request.respond(response);
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
    }
}

impl DatastoreInstance {

    fn get_stored_buckets(&mut self, conn: &Connection) -> Result <(), DatastoreError> {
        let mut stmt = match conn.prepare("SELECT id, name, type, client, hostname, created FROM buckets") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare get_stored_buckets SQL statement: {}", err.to_string())))
        };
        let buckets = match stmt.query_map(&[] as &[&ToSql], |row| {
            Ok(Bucket {
                bid: row.get(0)?,
                id: row.get(1)?,
                _type: row.get(2)?,
                client: row.get(3)?,
                hostname: row.get(4)?,
                created: row.get(5)?,
                events: None,
            })
        }) {
            Ok(buckets) => buckets,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to query get_stored_buckets SQL statement: {}", err)))
        };
        for bucket in buckets {
            match bucket {
                Ok(b) => {
                    self.buckets_cache.insert(b.id.clone(), b.clone());
                },
                Err(e) => {
                    error!("Failed to parse bucket from SQLite, database is corrupt! {}", e);
                }
            }
        };
        Ok(())
    }

    fn create_bucket(&mut self, conn: &Connection, bucket: &Bucket) -> Result<(), DatastoreError> {
        let created = match bucket.created {
            Some(created) => created,
            None => Utc::now()
        };
        let mut stmt = match conn.prepare("
                INSERT INTO buckets (name, type, client, hostname, created)
                VALUES (?1, ?2, ?3, ?4, ?5)") {
            Ok(buckets) => buckets,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare create_bucket SQL statement: {}", err.to_string()))),
        };
        let res = stmt.execute(&[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &created as &ToSql]);

        match res {
            Ok(_) => {
                let rowid = conn.last_insert_rowid();
                let mut inserted_bucket = bucket.clone();
                inserted_bucket.bid = Some(rowid);

                info!("Created bucket {}", inserted_bucket.id);
                self.buckets_cache.insert(bucket.id.clone(), inserted_bucket);
                self.commit = true;
            },
            // FIXME: This match is ugly, is it possible to write it in a cleaner way?
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => { return Err(DatastoreError::BucketAlreadyExists); },
                    _ => return Err(DatastoreError::InternalError(format!("Failed to execute create_bucket SQL statement: {}", err)))
                },
                _ => return Err(DatastoreError::InternalError(format!("Failed to execute create_bucket SQL statement: {}", err)))
            }
        };
        if let Some(ref events) = bucket.events {
            self.insert_events(conn, &bucket.id, events)?;
        }
        return Ok(());
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


    pub fn insert_events(&mut self, conn: &Connection, bucket_id: &str, events: &Vec<Event>) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut stmt = match conn.prepare("
                INSERT INTO events(bucketrow, starttime, endtime, data)
                VALUES (?1, ?2, ?3, ?4)") {
            Ok(stmt) => stmt,
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to prepare insert_events SQL statement: {}", err)))
        };
        for event in events {
            let starttime_nanos = event.timestamp.timestamp_nanos();
            let duration_nanos = match event.duration.num_nanoseconds() {
                Some(nanos) => nanos,
                None => return Err(DatastoreError::InternalError("Failed to convert duration to nanoseconds".to_string()))
            };
            let endtime_nanos = starttime_nanos + duration_nanos;
            let res = stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data as &ToSql]);
            match res {
                Ok(_) => self.uncommited_events += 1,
                Err(err) => {
                    return Err(DatastoreError::InternalError(format!("Failed to insert event: {:?}, {}", event, err)));
                }
            }
        }
        Ok(())
    }

    pub fn replace_last_event(&mut self, conn: &Connection, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

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
        match stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data as &ToSql]) {
            Ok(_) => (),
            Err(err) => return Err(DatastoreError::InternalError(format!("Failed to execute replace_last_event SQL statement: {}", err)))
        }
        self.uncommited_events += 1;
        Ok(())
    }

    pub fn heartbeat(&mut self, conn: &Connection, bucket_id: &str, heartbeat: Event, pulsetime: f64, last_heartbeat: &mut HashMap<String, Option<Event>>) -> Result<(), DatastoreError> {
        try!(self.get_bucket(&bucket_id));
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
                        self.insert_events(conn, &bucket_id, &vec![heartbeat.clone()])?;
                        return Ok(());
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
                self.insert_events(conn, &bucket_id, &vec![heartbeat.clone()])?;
                heartbeat
            }
        };
        last_heartbeat.insert(bucket_id.to_string(), Some(inserted_heartbeat));
        Ok(())
    }

    pub fn get_events(&mut self, conn: &Connection, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

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
            let data = row.get(3)?;

            if starttime_ns < starttime_filter_ns { starttime_ns = starttime_filter_ns }
            if endtime_ns > endtime_filter_ns { endtime_ns = endtime_filter_ns }
            let duration_ns = endtime_ns - starttime_ns;

            let time_seconds : i64 = (starttime_ns/1000000000) as i64;
            let time_subnanos : u32 = (starttime_ns%1000000000) as u32;

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
        let bucket = try!(self.get_bucket(&bucket_id));

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
        let (responder, requester) = mpsc_requests::channel::<Commands, Result<Responses, DatastoreError>>();
        let _thread = thread::spawn(move || {
            let mut di = DatastoreWorker::new(responder);
            di.work_loop(method);
        });
        Datastore {
            requester: requester,
        }
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        match self.requester.request(Commands::CreateBucket(bucket.clone())) {
            Ok(_) => Ok(()),
            Err(e) => Err(e)
        }
    }

    pub fn delete_bucket(&self, bucket_id: &str) -> Result<(), DatastoreError>{
        match self.requester.request(Commands::DeleteBucket(bucket_id.to_string())) {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        match self.requester.request(Commands::GetBucket(bucket_id.to_string())) {
            Ok(r) => match r {
                Responses::Bucket(b) => Ok(b),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        match self.requester.request(Commands::GetBuckets()) {
            Ok(r) => match r {
                Responses::BucketMap(bm) => Ok(bm),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn insert_events(&self, bucket_id: &str, events: &Vec<Event>) -> Result<(), DatastoreError> {
        match self.requester.request(Commands::InsertEvents(bucket_id.to_string(), events.clone())) {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn heartbeat(&self, bucket_id: &str, heartbeat: Event, pulsetime: f64) -> Result<(), DatastoreError> {
        match self.requester.request(Commands::Heartbeat(bucket_id.to_string(), heartbeat, pulsetime)) {
            Ok(r) => match r {
                Responses::Empty() => return Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn replace_last_event(&self, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        match self.requester.request(Commands::ReplaceLastEvent(bucket_id.to_string(), event.clone())) {
            Ok(r) => match r {
                Responses::Empty() => return Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_events(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        match self.requester.request(Commands::GetEvents(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone(), limit_opt.clone())) {
            Ok(r) => match r {
                Responses::EventList(el) => Ok(el),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_event_count(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, DatastoreError> {
        match self.requester.request(Commands::GetEventCount(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone())) {
            Ok(r) => match r {
                Responses::Count(n) => Ok(n),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }
}
