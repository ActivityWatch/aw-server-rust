use std::fmt;
use std::thread;
use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;

use rusqlite::Connection;
use rusqlite::DropBehavior;
use rusqlite::TransactionBehavior;

use aw_models::Bucket;
use aw_models::Event;

use crate::DatastoreError;
use crate::DatastoreMethod;
use crate::DatastoreInstance;

use crossbeam_requests;

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

/*
 * TODO:
 * - Allow read requests to go straight through a read-only db connection instead of requesting the
 * worker thread for better performance?
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
    ForceCommit(),
}

struct DatastoreWorker {
    responder: RequestReceiver,
    quit: bool
}

impl DatastoreWorker {
    pub fn new(responder: crossbeam_requests::RequestReceiver<Commands, Result<Responses, DatastoreError>>) -> Self {
        DatastoreWorker {
            responder,
            quit: false,
        }
    }

    fn work_loop(&mut self, method: DatastoreMethod) -> () {
        // Open SQLite connection
        let mut conn = match method {
            DatastoreMethod::Memory() => Connection::open_in_memory()
                .expect("Failed to create in-memory datastore"),
            DatastoreMethod::File(path) => Connection::open(path)
                .expect("Failed to create datastore")
        };
        let mut ds = DatastoreInstance::new(&mut conn, true).unwrap();
        let mut last_heartbeat = HashMap::new();

        // Ensure legacy import
        #[cfg(feature = "legacy_import")]
        {
            let mut transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                Ok(transaction) => transaction,
                Err(err) => panic!("Unable to start immediate transaction on SQLite database! {}", err)
            };
            match ds.ensure_legacy_import(&mut transaction) {
                Ok(_) => (),
                Err(err) => error!("Failed to do legacy import: {:?}", err),
            }
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {}", err)
            }
        }

        // Start handling and respond to requests
        loop {
            let mut transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                Ok(transaction) => transaction,
                Err(err) => panic!("Unable to start immediate transaction on SQLite database! {}", err)
            };
            let mut commit = false;
            let mut uncommited_events = 0;
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
                            Ok(_) => {
                                commit = true;
                                Ok(Responses::Empty())
                            },
                            Err(e) => Err(e)
                        }
                    },
                    Commands::DeleteBucket(bucketname) => {
                        match ds.delete_bucket(&transaction, &bucketname) {
                            Ok(_) => {
                                commit = true;
                                Ok(Responses::Empty())
                            },
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
                                uncommited_events += events.len();
                                last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                                Ok(Responses::EventList(events))
                            },
                            Err(e) => Err(e)
                        }
                    },
                    Commands::Heartbeat(bucketname, event, pulsetime) => {
                        match ds.heartbeat(&transaction, &bucketname, event, pulsetime, &mut last_heartbeat) {
                            Ok(e) => {
                                uncommited_events += 1;
                                Ok(Responses::Event(e))
                            },
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
                    Commands::ForceCommit() => {
                        commit = true;
                        Ok(Responses::Empty())
                    },
                };
                response_sender.respond(response);
                if commit || uncommited_events > 100 { break };
            }
            debug!("Commiting DB! Force commit {}, {} uncommited events", commit, uncommited_events);
            match transaction.commit() {
                Ok(_) => (),
                Err(err) => panic!("Failed to commit datastore transaction! {}", err)
            }
            if self.quit { break };
        }
        info!("DB Worker thread finished");
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

    pub fn force_commit(&self) -> Result<(), DatastoreError> {
        let cmd = Commands::ForceCommit();
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Responses::Empty() => Ok(()),
                _ => panic!("Invalid response")
            },
            Err(e) => Err(e)
        }
    }
}
