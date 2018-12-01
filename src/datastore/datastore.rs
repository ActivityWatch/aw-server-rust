extern crate rusqlite;
extern crate chrono;

use std::fmt;
use std::sync::Mutex;
use std::thread;
use std::collections::HashMap;

use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;
use chrono::Duration;

use rusqlite::Connection;
use rusqlite::DropBehavior;

use mpsc_requests;

use models::Bucket;
use models::Event;
use transform;

/*
 * TODO:
 * - Needs refactoring?
 * - Add macro for getting requester lock
 */

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

#[derive(Debug)]
pub enum DatastoreError {
    NoSuchBucket,
    BucketAlreadyExists,
    RequestLockTimeout,
    InternalError,
}

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

pub struct Datastore {
    requester: Mutex<Requester>,
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
        )", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)", &[]).unwrap();

    conn.execute("
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bucketrow INTEGER NOT NULL,
            starttime INTEGER NOT NULL,
            endtime INTEGER NOT NULL,
            data TEXT NOT NULL,
            FOREIGN KEY (bucketrow) REFERENCES buckets(id)
        )", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_bucketrow_index ON events(bucketrow)", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_starttime_index ON events(starttime)", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_endtime_index ON events(endtime)", &[]).unwrap();
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
            DatastoreMethod::Memory() => Connection::open_in_memory().unwrap(),
            DatastoreMethod::File(path) => Connection::open(path).unwrap()
        };
        _create_tables(&conn);
        let mut ds = DatastoreInstance {
            commit: false,
            uncommited_events: 0,
            buckets_cache: HashMap::new()
        };
        let mut last_heartbeat = HashMap::new();
        ds.get_stored_buckets(&conn);
        loop {
            let mut transaction = conn.transaction().unwrap();
            transaction.set_drop_behavior(DropBehavior::Commit);
            loop {
                let mut request = match self.responder.poll() {
                    Ok(r) => r,
                    Err(_) => { // All references to responder is gone, quit
                        println!("DB worker quitting");
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
                        Ok(Responses::BucketMap(ds.get_buckets().unwrap()))
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
            println!("Commiting DB! Force commit {}, {} uncommited events", ds.commit, ds.uncommited_events);
            transaction.commit().unwrap();
            ds.commit = false;
            ds.uncommited_events = 0;
            if self.quit { break };
        }
    }
}

impl DatastoreInstance {

    fn get_stored_buckets(&mut self, conn: &Connection) {
        let mut stmt = conn.prepare("SELECT id, name, type, client, hostname, created FROM buckets").unwrap();
        let buckets = stmt.query_map(&[], |row| {
            Bucket {
                bid: row.get(0),
                id: row.get(1),
                _type: row.get(2),
                client: row.get(3),
                hostname: row.get(4),
                created: row.get(5),
            }
        }).unwrap();
        for bucket in buckets {
            match bucket {
                Ok(b) => {
                    println!("{:?}", b);
                    self.buckets_cache.insert(b.id.clone(), b.clone());
                },
                Err(e) => {
                    println!("Failed to parse bucket from SQLite, database is corrupt!");
                    println!("{}", e);
                }
            }
        };
        ()
    }

    fn create_bucket(&mut self, conn: &Connection, bucket: &Bucket) -> Result<(), DatastoreError> {
        let mut stmt = conn.prepare("
            INSERT INTO buckets (name, type, client, hostname, created)
            VALUES (?1, ?2, ?3, ?4, ?5)").unwrap();
        let res = stmt.execute(&[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created.unwrap()]);

        match res {
            Ok(_) => {
                let rowid = conn.last_insert_rowid();
                let mut inserted_bucket = bucket.clone();
                inserted_bucket.bid = Some(rowid);

                println!("{:?}", inserted_bucket);
                self.buckets_cache.insert(bucket.id.clone(), inserted_bucket);
                self.commit = true;
                return Ok(());
            },
            // FIXME: This match is ugly, is it possible to write it in a cleaner way?
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => { println!("{}", err); return Err(DatastoreError::InternalError); }
                },
                _ => { println!("{}", err); return Err(DatastoreError::InternalError); }
            }
        }
    }

    fn delete_bucket(&mut self, conn: &Connection, bucket_id: &str) -> Result<(), DatastoreError>{
        let bucket = (self.get_bucket(&bucket_id))?;
        // Delete all events in bucket
        match conn.execute("DELETE FROM events WHERE id = ?1", &[&bucket.bid]) {
            Ok(_) => (),
            Err(err) => { println!("{}", err); return Err(DatastoreError::InternalError) }
        }
        // Delete bucket itself
        match conn.execute("DELETE FROM buckets WHERE name = ?1", &[&bucket.id]) {
            Ok(_) => {
                self.buckets_cache.remove(bucket_id);
                self.commit = true;
                return Ok(());
            },
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
                },
                _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
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

    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        return Ok(self.buckets_cache.clone());
    }


    pub fn insert_events(&mut self, conn: &Connection, bucket_id: &str, events: &Vec<Event>) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut stmt = conn.prepare("
            INSERT INTO events(bucketrow, starttime, endtime, data)
            VALUES (?1, ?2, ?3, ?4)").unwrap();
        for event in events {
            let starttime_nanos = event.timestamp.timestamp_nanos();
            let duration_nanos = event.duration.num_nanoseconds().unwrap();
            let endtime_nanos = starttime_nanos + duration_nanos;
            let res = stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data]);
            match res {
                Ok(_) => self.uncommited_events += 1,
                Err(e) => {
                    println!("Failed to insert event: {}", e);
                    println!("{:?}", event);
                    return Err(DatastoreError::InternalError);
                }
            }
        }
        Ok(())
    }

    pub fn replace_last_event(&mut self, conn: &Connection, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut stmt = conn.prepare("
            UPDATE events
            SET starttime = ?2, endtime = ?3, data = ?4
            WHERE bucketrow = ?1
                AND endtime = (SELECT max(endtime) FROM events WHERE bucketrow = ?1)
        ").unwrap();
        let starttime_nanos = event.timestamp.timestamp_nanos();
        let duration_nanos = event.duration.num_nanoseconds().unwrap();
        let endtime_nanos = starttime_nanos + duration_nanos;
        stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data]).unwrap();
        self.uncommited_events += 1;
        Ok(())
    }

    pub fn heartbeat(&mut self, conn: &Connection, bucket_id: &str, heartbeat: Event, pulsetime: f64, last_heartbeat: &mut HashMap<String, Option<Event>>) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));
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
                self.replace_last_event(conn, &bucket_id, &merged_heartbeat);
                merged_heartbeat
            },
            None => {
                println!("Failed to merge!");
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
            println!("Starttime in event query was lower than endtime!");
            return Ok(list);
        }
        let limit = match limit_opt {
            Some(l) => l as i64,
            None => -1
        };

        let mut stmt = conn.prepare("
            SELECT id, starttime, endtime, data
            FROM events
            WHERE bucketrow = ?1
                AND endtime >= ?2
                AND starttime <= ?3
            ORDER BY starttime DESC
            LIMIT ?4
        ;").unwrap();

        let rows = stmt.query_map(&[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns, &limit], |row| {
            let id = row.get(0);
            let mut starttime_ns : i64 = row.get(1);
            let mut endtime_ns : i64 = row.get(2);
            let data = row.get(3);

            if starttime_ns < starttime_filter_ns { starttime_ns = starttime_filter_ns }
            if endtime_ns > endtime_filter_ns { endtime_ns = endtime_filter_ns }
            let duration_ns = endtime_ns - starttime_ns;

            let time_seconds : i64 = (starttime_ns/1000000000) as i64;
            let time_subnanos : u32 = (starttime_ns%1000000000) as u32;

            return Event {
                id: Some(id),
                timestamp: DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(time_seconds, time_subnanos), Utc),
                duration: Duration::nanoseconds(duration_ns),
                data: data,
            }
        }).unwrap();
        for row in rows {
            list.push(row.unwrap());
        }
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
            println!("Endtime in event query was same or lower than starttime!");
            return Ok(0);
        }

        let mut stmt = conn.prepare("
            SELECT count(*) FROM events
            WHERE bucketrow = ?1
                AND (starttime >= ?2 OR endtime <= ?3)"
        ).unwrap();

        let ret = stmt.query_row(&[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns],
            |row| row.get(0)
        ).unwrap();

        return Ok(ret);
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
            requester: Mutex::new(requester),
        }
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::CreateBucket(bucket.clone())) {
            Ok(_) => Ok(()),
            Err(e) => Err(e)
        }
    }

    pub  fn delete_bucket(&self, bucket_id: &str) -> Result<(), DatastoreError>{
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::DeleteBucket(bucket_id.to_string())) {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::GetBucket(bucket_id.to_string())) {
            Ok(r) => match r {
                Responses::Bucket(b) => Ok(b),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::GetBuckets()) {
            Ok(r) => match r {
                Responses::BucketMap(bm) => Ok(bm),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn insert_events(&self, bucket_id: &str, events: &Vec<Event>) -> Result<(), DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::InsertEvents(bucket_id.to_string(), events.clone())) {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn heartbeat(&self, bucket_id: &str, heartbeat: Event, pulsetime: f64) -> Result<(), DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::Heartbeat(bucket_id.to_string(), heartbeat, pulsetime)) {
            Ok(r) => match r {
                Responses::Empty() => return Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn replace_last_event(&self, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::ReplaceLastEvent(bucket_id.to_string(), event.clone())) {
            Ok(r) => match r {
                Responses::Empty() => return Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_events(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::GetEvents(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone(), limit_opt.clone())) {
            Ok(r) => match r {
                Responses::EventList(el) => Ok(el),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_event_count(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, DatastoreError> {
        let requester = match self.requester.lock() {
            Ok(r) => r,
            Err(_) => return Err(DatastoreError::RequestLockTimeout)
        };
        match requester.request(Commands::GetEventCount(bucket_id.to_string(), starttime_opt.clone(), endtime_opt.clone())) {
            Ok(r) => match r {
                Responses::Count(n) => Ok(n),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }
}
