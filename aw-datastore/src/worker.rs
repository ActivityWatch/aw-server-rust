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
use aw_models::KeyValue;

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
 * TODO: Add an seperate "Import" request which does an import with an transaction
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
    KeyValue(KeyValue),
    StringVec(Vec<String>),
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
    GetEvents(
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<u64>,
    ),
    GetEventCount(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    DeleteEventsById(String, Vec<i64>),
    ForceCommit(),
    InsertKeyValue(String, String),
    GetKeyValue(String),
    GetKeysStarting(String),
    DeleteKeyValue(String),
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
    uncommited_events: usize,
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
            uncommited_events: 0,
            commit: false,
            last_heartbeat: HashMap::new(),
        }
    }

    fn work_loop(&mut self, method: DatastoreMethod) {
        // Open SQLite connection
        let mut conn = match method {
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
                Err(err) => panic!(
                    "Unable to start immediate transaction on SQLite database! {}",
                    err
                ),
            };
            match ds.ensure_legacy_import(&transaction) {
                Ok(_) => (),
                Err(err) => error!("Failed to do legacy import: {:?}", err),
            }
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {}", err),
            }
        }

        // Start handling and respond to requests
        loop {
            let last_commit_time: DateTime<Utc> = Utc::now();
            let mut transaction = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            self.uncommited_events = 0;
            self.commit = false;
            transaction.set_drop_behavior(DropBehavior::Commit);
            loop {
                let (request, response_sender) = match self.responder.poll() {
                    Ok((req, res_sender)) => (req, res_sender),
                    Err(_) => {
                        // All references to responder is gone, quit
                        info!("DB worker quitting");
                        self.quit = true;
                        break;
                    }
                };
                let response = self.handle_request(request, &mut ds, &transaction);
                response_sender.respond(response);
                let now: DateTime<Utc> = Utc::now();
                let commit_interval_passed: bool = (now - last_commit_time) > Duration::seconds(15);
                if self.commit || commit_interval_passed || self.uncommited_events > 100 {
                    break;
                };
            }
            debug!(
                "Commiting DB! Force commit {}, {} uncommited events",
                self.commit, self.uncommited_events
            );
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {}", err),
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
        transaction: &Transaction,
    ) -> Result<Response, DatastoreError> {
        match request {
            Command::CreateBucket(bucket) => match ds.create_bucket(&transaction, bucket) {
                Ok(_) => {
                    self.commit = true;
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::DeleteBucket(bucketname) => {
                match ds.delete_bucket(&transaction, &bucketname) {
                    Ok(_) => {
                        self.commit = true;
                        Ok(Response::Empty())
                    }
                    Err(e) => Err(e),
                }
            }
            Command::GetBucket(bucketname) => match ds.get_bucket(&bucketname) {
                Ok(b) => Ok(Response::Bucket(b)),
                Err(e) => Err(e),
            },
            Command::GetBuckets() => Ok(Response::BucketMap(ds.get_buckets())),
            Command::InsertEvents(bucketname, events) => {
                match ds.insert_events(&transaction, &bucketname, events) {
                    Ok(events) => {
                        self.uncommited_events += events.len();
                        self.last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                        Ok(Response::EventList(events))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::Heartbeat(bucketname, event, pulsetime) => {
                match ds.heartbeat(
                    &transaction,
                    &bucketname,
                    event,
                    pulsetime,
                    &mut self.last_heartbeat,
                ) {
                    Ok(e) => {
                        self.uncommited_events += 1;
                        Ok(Response::Event(e))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::GetEvents(bucketname, starttime_opt, endtime_opt, limit_opt) => {
                match ds.get_events(
                    &transaction,
                    &bucketname,
                    starttime_opt,
                    endtime_opt,
                    limit_opt,
                ) {
                    Ok(el) => Ok(Response::EventList(el)),
                    Err(e) => Err(e),
                }
            }
            Command::GetEventCount(bucketname, starttime_opt, endtime_opt) => {
                match ds.get_event_count(&transaction, &bucketname, starttime_opt, endtime_opt) {
                    Ok(n) => Ok(Response::Count(n)),
                    Err(e) => Err(e),
                }
            }
            Command::DeleteEventsById(bucketname, event_ids) => {
                match ds.delete_events_by_id(&transaction, &bucketname, event_ids) {
                    Ok(()) => Ok(Response::Empty()),
                    Err(e) => Err(e),
                }
            }
            Command::ForceCommit() => {
                self.commit = true;
                Ok(Response::Empty())
            }
            Command::InsertKeyValue(key, data) => {
                match ds.insert_key_value(&transaction, &key, &data) {
                    Ok(()) => Ok(Response::Empty()),
                    Err(e) => Err(e),
                }
            }
            Command::GetKeyValue(key) => match ds.get_key_value(&transaction, &key) {
                Ok(result) => Ok(Response::KeyValue(result)),
                Err(e) => Err(e),
            },
            Command::GetKeysStarting(pattern) => {
                match ds.get_keys_starting(&transaction, &pattern) {
                    Ok(result) => Ok(Response::StringVec(result)),
                    Err(e) => Err(e),
                }
            }
            Command::DeleteKeyValue(key) => match ds.delete_key_value(&transaction, &key) {
                Ok(()) => Ok(Response::Empty()),
                Err(e) => Err(e),
            },
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
                    "Invalid response: {:?}",
                    e
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

    pub fn insert_key_value(&self, key: &str, data: &str) -> Result<(), DatastoreError> {
        let cmd = Command::InsertKeyValue(key.to_string(), data.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        _unwrap_response(receiver)
    }

    pub fn delete_key_value(&self, key: &str) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteKeyValue(key.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        _unwrap_response(receiver)
    }

    pub fn get_key_value(&self, key: &str) -> Result<KeyValue, DatastoreError> {
        let cmd = Command::GetKeyValue(key.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::KeyValue(value) => Ok(value),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn get_keys_starting(&self, pattern: &str) -> Result<Vec<String>, DatastoreError> {
        let cmd = Command::GetKeysStarting(pattern.to_string());
        let receiver = self.requester.request(cmd).unwrap();

        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::StringVec(value) => Ok(value),
                _ => panic!("Invalid response"),
            },
            Err(e) => Err(e),
        }
    }
}
