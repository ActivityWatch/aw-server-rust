use std::collections::HashMap;
use std::fmt;
use std::thread;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

use rusqlite::Connection;
use rusqlite::DropBehavior;
use rusqlite::Transaction;
use rusqlite::TransactionBehavior;

use aw_models::Bucket;
use aw_models::Event;

use crate::DatastoreError;
use crate::DatastoreInstance;
use crate::DatastoreMethod;

use mpsc_requests::ResponseReceiver;

type RequestSender = mpsc_requests::RequestSender<Command, Result<Response, DatastoreError>>;
type RequestReceiver = mpsc_requests::RequestReceiver<Command, Result<Response, DatastoreError>>;

#[derive(Clone)]
pub struct Datastore {
    requester: RequestSender,
}

impl fmt::Debug for Datastore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Datastore()")
    }
}

/*
 * TODO:
 * - Allow read requests to go straight through a read-only db connection instead of requesting the
 * worker thread for better performance?
 * TODO: Add an separate "Import" request which does an import with an transaction
 */

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Response {
    Empty(),
    Bucket(Bucket),
    BucketMap(HashMap<String, Bucket>),
    Event(Event),
    EventList(Vec<Event>),
    Count(i64),
    KeyValue(String),
    KeyValues(HashMap<String, String>),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Command {
    CreateBucket(Bucket),
    DeleteBucket(String),
    GetBucket(String),
    GetBuckets(),
    InsertEvents(String, Vec<Event>),
    Heartbeat(String, Event, f64),
    GetEvent(String, i64),
    GetEvents(
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<u64>,
    ),
    GetEventCount(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    DeleteEventsById(String, Vec<i64>),
    ForceCommit(),
    GetKeyValues(String),
    GetKeyValue(String),
    SetKeyValue(String, String),
    DeleteKeyValue(String),
    Close(),
}

fn _unwrap_response(
    receiver: ResponseReceiver<Result<Response, DatastoreError>>,
) -> Result<(), DatastoreError> {
    match receiver.collect().unwrap() {
        Ok(r) => match r {
            Response::Empty() => Ok(()),
            _ => panic!("Invalid response"),
        },
        Err(e) => Err(e),
    }
}

struct DatastoreWorker {
    responder: RequestReceiver,
    legacy_import: bool,
    quit: bool,
    uncommitted_events: usize,
    commit: bool,
    last_heartbeat: HashMap<String, Option<Event>>,
}

impl DatastoreWorker {
    pub fn new(
        responder: mpsc_requests::RequestReceiver<Command, Result<Response, DatastoreError>>,
        legacy_import: bool,
    ) -> Self {
        DatastoreWorker {
            responder,
            legacy_import,
            quit: false,
            uncommitted_events: 0,
            commit: false,
            last_heartbeat: HashMap::new(),
        }
    }

    fn work_loop(&mut self, method: DatastoreMethod) {
        // Open SQLite connection
        let mut conn = match &method {
            DatastoreMethod::Memory() => {
                Connection::open_in_memory().expect("Failed to create in-memory datastore")
            }
            DatastoreMethod::File(path) => {
                Connection::open(path).expect("Failed to create datastore")
            }
        };
        let mut ds = DatastoreInstance::new(&conn, true).unwrap();

        // Ensure legacy import
        if self.legacy_import {
            let transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                Ok(transaction) => transaction,
                Err(err) => {
                    panic!("Unable to start immediate transaction on SQLite database! {err}")
                }
            };
            match ds.ensure_legacy_import(&transaction) {
                Ok(_) => (),
                Err(err) => error!("Failed to do legacy import: {:?}", err),
            }
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {err}"),
            }
        }

        // Start handling and respond to requests
        loop {
            let last_commit_time: DateTime<Utc> = Utc::now();
            let mut tx: Transaction =
                match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                    Ok(tx) => tx,
                    Err(err) => {
                        error!("Unable to start transaction! {:?}", err);
                        // Wait 1s before retrying
                        std::thread::sleep(std::time::Duration::from_millis(1000));
                        continue;
                    }
                };
            tx.set_drop_behavior(DropBehavior::Commit);

            self.uncommitted_events = 0;
            self.commit = false;
            loop {
                let (request, response_sender) = match self.responder.poll() {
                    Ok((req, res_sender)) => (req, res_sender),
                    Err(err) => {
                        // All references to responder is gone, quit
                        error!("DB worker quitting, error: {err:?}");
                        self.quit = true;
                        break;
                    }
                };
                let response = self.handle_request(request, &mut ds, &tx);
                response_sender.respond(response);

                let now: DateTime<Utc> = Utc::now();
                let commit_interval_passed: bool = (now - last_commit_time) > Duration::seconds(15);
                if self.commit
                    || commit_interval_passed
                    || self.uncommitted_events > 100
                    || self.quit
                {
                    break;
                };
            }
            debug!(
                "Committing DB! Force commit {}, {} uncommitted events",
                self.commit, self.uncommitted_events
            );
            match tx.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {err}"),
            }
            if self.quit {
                break;
            };
        }
        info!("DB Worker thread finished");
    }

    fn handle_request(
        &mut self,
        request: Command,
        ds: &mut DatastoreInstance,
        tx: &Transaction,
    ) -> Result<Response, DatastoreError> {
        match request {
            Command::CreateBucket(bucket) => match ds.create_bucket(tx, bucket) {
                Ok(_) => {
                    self.commit = true;
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::DeleteBucket(bucketname) => match ds.delete_bucket(tx, &bucketname) {
                Ok(_) => {
                    self.commit = true;
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::GetBucket(bucketname) => match ds.get_bucket(&bucketname) {
                Ok(b) => Ok(Response::Bucket(b)),
                Err(e) => Err(e),
            },
            Command::GetBuckets() => Ok(Response::BucketMap(ds.get_buckets())),
            Command::InsertEvents(bucketname, events) => {
                match ds.insert_events(tx, &bucketname, events) {
                    Ok(events) => {
                        self.uncommitted_events += events.len();
                        self.last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                        Ok(Response::EventList(events))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::Heartbeat(bucketname, event, pulsetime) => {
                match ds.heartbeat(tx, &bucketname, event, pulsetime, &mut self.last_heartbeat) {
                    Ok(e) => {
                        self.uncommitted_events += 1;
                        Ok(Response::Event(e))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::GetEvent(bucketname, event_id) => {
                match ds.get_event(tx, &bucketname, event_id) {
                    Ok(el) => Ok(Response::Event(el)),
                    Err(e) => Err(e),
                }
            }
            Command::GetEvents(bucketname, starttime_opt, endtime_opt, limit_opt) => {
                match ds.get_events(tx, &bucketname, starttime_opt, endtime_opt, limit_opt) {
                    Ok(el) => Ok(Response::EventList(el)),
                    Err(e) => Err(e),
                }
            }
            Command::GetEventCount(bucketname, starttime_opt, endtime_opt) => {
                match ds.get_event_count(tx, &bucketname, starttime_opt, endtime_opt) {
                    Ok(n) => Ok(Response::Count(n)),
                    Err(e) => Err(e),
                }
            }
            Command::DeleteEventsById(bucketname, event_ids) => {
                match ds.delete_events_by_id(tx, &bucketname, event_ids) {
                    Ok(()) => Ok(Response::Empty()),
                    Err(e) => Err(e),
                }
            }
            Command::ForceCommit() => {
                self.commit = true;
                Ok(Response::Empty())
            }
            Command::GetKeyValues(pattern) => match ds.get_key_values(tx, pattern.as_str()) {
                Ok(result) => Ok(Response::KeyValues(result)),
                Err(e) => Err(e),
            },
            Command::SetKeyValue(key, data) => match ds.insert_key_value(tx, &key, &data) {
                Ok(()) => Ok(Response::Empty()),
                Err(e) => Err(e),
            },
            Command::GetKeyValue(key) => match ds.get_key_value(tx, &key) {
                Ok(result) => Ok(Response::KeyValue(result)),
                Err(e) => Err(e),
            },
            Command::DeleteKeyValue(key) => match ds.delete_key_value(tx, &key) {
                Ok(()) => Ok(Response::Empty()),
                Err(e) => Err(e),
            },
            Command::Close() => {
                self.quit = true;
                Ok(Response::Empty())
            }
        }
    }
}

impl Datastore {
    pub fn new(dbpath: String, legacy_import: bool) -> Self {
        let method = DatastoreMethod::File(dbpath);
        Datastore::_new_internal(method, legacy_import)
    }

    pub fn new_in_memory(legacy_import: bool) -> Self {
        let method = DatastoreMethod::Memory();
        Datastore::_new_internal(method, legacy_import)
    }

    fn _new_internal(method: DatastoreMethod, legacy_import: bool) -> Self {
        let (requester, responder) =
            mpsc_requests::channel::<Command, Result<Response, DatastoreError>>();
        let _thread = thread::spawn(move || {
            let mut di = DatastoreWorker::new(responder, legacy_import);
            di.work_loop(method);
        });
        Datastore { requester }
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        let cmd = Command::CreateBucket(bucket.clone());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn delete_bucket(&self, bucket_id: &str) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteBucket(bucket_id.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Empty() => Ok(()),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cmd = Command::GetBucket(bucket_id.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Bucket(b) => Ok(b),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        let cmd = Command::GetBuckets();
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::BucketMap(bm) => Ok(bm),
                e => Err(DatastoreError::InternalError(format!(
                    "Invalid response: {e:?}"
                ))),
            },
            Err(e) => Err(e),
        }
    }

    pub fn insert_events(
        &self,
        bucket_id: &str,
        events: &[Event],
    ) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Command::InsertEvents(bucket_id.to_string(), events.to_vec());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::EventList(events) => Ok(events),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn heartbeat(
        &self,
        bucket_id: &str,
        heartbeat: Event,
        pulsetime: f64,
    ) -> Result<Event, DatastoreError> {
        let cmd = Command::Heartbeat(bucket_id.to_string(), heartbeat, pulsetime);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Event(e) => Ok(e),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_event(&self, bucket_id: &str, event_id: i64) -> Result<Event, DatastoreError> {
        let cmd = Command::GetEvent(bucket_id.to_string(), event_id);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Event(el) => Ok(el),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_events(
        &self,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Command::GetEvents(bucket_id.to_string(), starttime_opt, endtime_opt, limit_opt);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::EventList(el) => Ok(el),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_event_count(
        &self,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
    ) -> Result<i64, DatastoreError> {
        let cmd = Command::GetEventCount(bucket_id.to_string(), starttime_opt, endtime_opt);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Count(n) => Ok(n),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn delete_events_by_id(
        &self,
        bucket_id: &str,
        event_ids: Vec<i64>,
    ) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteEventsById(bucket_id.to_string(), event_ids);
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Empty() => Ok(()),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn force_commit(&self) -> Result<(), DatastoreError> {
        let cmd = Command::ForceCommit();
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Empty() => Ok(()),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_key_values(&self, pattern: &str) -> Result<HashMap<String, String>, DatastoreError> {
        let cmd = Command::GetKeyValues(pattern.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::KeyValues(value) => Ok(value),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_key_value(&self, key: &str) -> Result<String, DatastoreError> {
        let cmd = Command::GetKeyValue(key.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::KeyValue(kv) => Ok(kv),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn set_key_value(&self, key: &str, data: &str) -> Result<(), DatastoreError> {
        let cmd = Command::SetKeyValue(key.to_string(), data.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        _unwrap_response(receiver)
    }

    pub fn delete_key_value(&self, key: &str) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteKeyValue(key.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        _unwrap_response(receiver)
    }

    // Should block until worker has stopped
    pub fn close(&self) {
        info!("Sending close request to database");
        let receiver = self.requester.request(Command::Close()).unwrap();

        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Empty() => (),
                _ => panic!("Invalid response"),
            },
            Err(e) => panic!("Error closing database: {:?}", e),
        }
    }
}
