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

use crate::privacy_filter::PrivacyFilterEngine;
use crate::DatastoreError;
use crate::DatastoreInstance;
use crate::DatastoreMethod;

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
        bool,
    ),
    GetEventCount(String, Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    DeleteEventsById(String, Vec<i64>),
    ForceCommit(),
    GetKeyValues(String),
    GetKeyValue(String),
    SetKeyValue(String, String),
    DeleteKeyValue(String),
    RefreshPrivacyFilter(),
    RenameBucket(String, String),
    MigrateHostname(String),
    MigrateTestBucketNames(),
    Close(),
}

fn _unwrap_empty_response(response: Response) -> Result<(), DatastoreError> {
    match response {
        Response::Empty() => Ok(()),
        _ => panic!("Invalid response"),
    }
}

struct DatastoreWorker {
    responder: RequestReceiver,
    legacy_import: bool,
    quit: bool,
    uncommitted_events: usize,
    commit: bool,
    last_heartbeat: HashMap<String, Option<Event>>,
    privacy_engine: PrivacyFilterEngine,
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
            privacy_engine: PrivacyFilterEngine::new(vec![]),
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
            #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
            DatastoreMethod::FileEncrypted(path, key) => {
                let conn = Connection::open(path).expect("Failed to create encrypted datastore");
                conn.pragma_update(None, "key", key.as_str())
                    .expect("Failed to set SQLCipher encryption key");
                // PRAGMA key always succeeds even with a wrong passphrase; the
                // first real SQL query is what fails. Read user_version immediately
                // to surface an incorrect key as a clear error rather than an
                // opaque panic later.
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("Failed to open encrypted database: wrong passphrase or not an encrypted database");
                info!("Opened encrypted database at {}", path);
                conn
            }
        };

        // WAL turns each commit into a single sequential WAL append+fsync where
        // delete mode paid two fsyncs plus journal-file churn, and lets future
        // reader connections proceed while a commit is in flight.
        // synchronous=FULL is set explicitly (rather than relying on the
        // default) so a commit remains durable on disk the moment it returns;
        // with NORMAL the WAL is only synced at checkpoints, which would
        // silently widen the loss window on power failure.
        // In-memory databases ignore the request (journal_mode stays "memory").
        let journal_mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .expect("Failed to query journal_mode");
        if !matches!(&method, DatastoreMethod::Memory()) && journal_mode != "wal" {
            warn!("Failed to enable WAL (journal_mode={journal_mode}), continuing without it");
        }
        conn.pragma_update(None, "synchronous", "FULL")
            .expect("Failed to set synchronous=FULL");

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
                Err(err) => {
                    error!("Failed to commit legacy import transaction: {err}");
                    // Continue without panicking — legacy import will be retried on
                    // next startup if the commit didn't persist.
                }
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
            // ForceCommit and Close promise the caller that their data is
            // committed, so their acks are held back until the transaction
            // below has actually committed. Acking first (as before) let a
            // caller reopen the database and read a pre-commit snapshot —
            // harmless under the rollback journal's locking, but a real race
            // in WAL mode where readers never block on the writer.
            // All other commands are acked immediately: a watcher heartbeat
            // must not wait up to 15 s for the batch commit.
            let mut deferred_ack = None;
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
                let ack_after_commit = matches!(request, Command::ForceCommit() | Command::Close());
                let response = self.handle_request(request, &mut ds, &tx);
                if ack_after_commit {
                    // Both commands force a commit, so the loop ends here.
                    deferred_ack = Some((response_sender, response));
                    break;
                }
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
                Ok(_) => {
                    if let Some((sender, response)) = deferred_ack.take() {
                        sender.respond(response);
                    }
                }
                Err(err) => {
                    error!(
                        "Failed to commit datastore transaction ({} events lost): {err}",
                        self.uncommitted_events
                    );
                    // Continue instead of panicking — the worker thread survives this
                    // transient failure (e.g. SQLITE_FULL on disk full). Note: clients
                    // already received success responses before the commit, so they won't
                    // know to retry. Rolled-back events create a gap in the timeline;
                    // watchers will resume sending heartbeats from current state, but the
                    // specific batch of events is permanently lost.
                    if let Some((sender, _)) = deferred_ack.take() {
                        sender.respond(Err(DatastoreError::InternalError(format!(
                            "Failed to commit datastore transaction: {err}"
                        ))));
                    }
                }
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
                let filtered = self.privacy_engine.filter_events(&bucketname, events);
                if filtered.is_empty() {
                    return Ok(Response::EventList(vec![]));
                }
                match ds.insert_events(tx, &bucketname, filtered) {
                    Ok(events) => {
                        self.uncommitted_events += events.len();
                        self.last_heartbeat.insert(bucketname.to_string(), None); // invalidate last_heartbeat cache
                        Ok(Response::EventList(events))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::Heartbeat(bucketname, event, pulsetime) => {
                // Apply privacy filter to heartbeat
                let filtered = match self.privacy_engine.filter_event(&bucketname, event.clone()) {
                    Some(event) => event,
                    None => {
                        // Heartbeat dropped by filter — return last cached event so the
                        // watcher's heartbeat-merge state machine continues correctly.
                        // Fall back to the incoming event itself if no prior event is cached
                        // (avoids returning a zero-timestamp default Event).
                        let last = self
                            .last_heartbeat
                            .get(&bucketname)
                            .and_then(|e| e.clone())
                            .unwrap_or(event);
                        return Ok(Response::Event(last));
                    }
                };
                match ds.heartbeat(
                    tx,
                    &bucketname,
                    filtered,
                    pulsetime,
                    &mut self.last_heartbeat,
                ) {
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
            Command::GetEvents(bucketname, starttime_opt, endtime_opt, limit_opt, unclipped) => {
                let result = if unclipped {
                    ds.get_events_unclipped(tx, &bucketname, starttime_opt, endtime_opt, limit_opt)
                } else {
                    ds.get_events(tx, &bucketname, starttime_opt, endtime_opt, limit_opt)
                };
                match result {
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
            Command::RefreshPrivacyFilter() => {
                // Reload privacy filter rules from settings
                match ds.get_key_value(tx, "settings.privacy_filters") {
                    Ok(json_str) => match PrivacyFilterEngine::from_json(&json_str) {
                        Ok(engine) => self.privacy_engine = engine,
                        Err(e) => warn!("Failed to parse privacy_filters setting: {e}"),
                    },
                    Err(_) => {
                        // Settings key absent — clear rules so removing the key disables filtering
                        self.privacy_engine = PrivacyFilterEngine::new(vec![]);
                    }
                }
                Ok(Response::Empty())
            }
            Command::RenameBucket(old_id, new_id) => match ds.rename_bucket(tx, &old_id, &new_id) {
                Ok(()) => {
                    self.commit = true;
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::MigrateHostname(new_hostname) => {
                match ds.migrate_hostname(tx, &new_hostname) {
                    Ok(count) => {
                        if count > 0 {
                            self.commit = true;
                        }
                        Ok(Response::Count(count as i64))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::MigrateTestBucketNames() => {
                match ds.migrate_test_bucket_names(tx) {
                    Ok(count) => {
                        if count > 0 {
                            self.commit = true;
                        }
                        Ok(Response::Count(count as i64))
                    }
                    Err(e) => Err(e),
                }
            }
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

    /// Create an encrypted datastore using SQLCipher.
    ///
    /// Requires the `encryption` or `encryption-vendored` feature flag.
    /// Build with: `cargo build --no-default-features --features encryption`
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    pub fn new_encrypted(dbpath: String, key: String, legacy_import: bool) -> Self {
        let method = DatastoreMethod::FileEncrypted(dbpath, zeroize::Zeroizing::new(key));
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

    /// Send a command to the worker thread and wait for its response.
    ///
    /// Fails with `InternalError` instead of panicking when the worker thread
    /// is gone (e.g. it panicked on an earlier request), so callers such as
    /// HTTP endpoints can degrade to a 5xx response instead of crashing the
    /// request.
    fn request(&self, cmd: Command) -> Result<Response, DatastoreError> {
        let receiver = self.requester.request(cmd).map_err(|e| {
            DatastoreError::InternalError(format!(
                "Failed to send request, datastore worker is gone: {e:?}"
            ))
        })?;
        receiver.collect().map_err(|e| {
            DatastoreError::InternalError(format!(
                "Failed to receive response, datastore worker died while handling request: {e:?}"
            ))
        })?
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        let cmd = Command::CreateBucket(bucket.clone());
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn delete_bucket(&self, bucket_id: &str) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteBucket(bucket_id.to_string());
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cmd = Command::GetBucket(bucket_id.to_string());
        match self.request(cmd)? {
            Response::Bucket(b) => Ok(b),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        let cmd = Command::GetBuckets();
        match self.request(cmd)? {
            Response::BucketMap(bm) => Ok(bm),
            e => Err(DatastoreError::InternalError(format!(
                "Invalid response: {e:?}"
            ))),
        }
    }

    pub fn insert_events(
        &self,
        bucket_id: &str,
        events: &[Event],
    ) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Command::InsertEvents(bucket_id.to_string(), events.to_vec());
        match self.request(cmd)? {
            Response::EventList(events) => Ok(events),
            _ => panic!("Invalid response"),
        }
    }

    pub fn heartbeat(
        &self,
        bucket_id: &str,
        heartbeat: Event,
        pulsetime: f64,
    ) -> Result<Event, DatastoreError> {
        let cmd = Command::Heartbeat(bucket_id.to_string(), heartbeat, pulsetime);
        match self.request(cmd)? {
            Response::Event(e) => Ok(e),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_event(&self, bucket_id: &str, event_id: i64) -> Result<Event, DatastoreError> {
        let cmd = Command::GetEvent(bucket_id.to_string(), event_id);
        match self.request(cmd)? {
            Response::Event(el) => Ok(el),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_events(
        &self,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Command::GetEvents(
            bucket_id.to_string(),
            starttime_opt,
            endtime_opt,
            limit_opt,
            false,
        );
        match self.request(cmd)? {
            Response::EventList(el) => Ok(el),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_events_unclipped(
        &self,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
        limit_opt: Option<u64>,
    ) -> Result<Vec<Event>, DatastoreError> {
        let cmd = Command::GetEvents(
            bucket_id.to_string(),
            starttime_opt,
            endtime_opt,
            limit_opt,
            true,
        );
        match self.request(cmd)? {
            Response::EventList(el) => Ok(el),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_event_count(
        &self,
        bucket_id: &str,
        starttime_opt: Option<DateTime<Utc>>,
        endtime_opt: Option<DateTime<Utc>>,
    ) -> Result<i64, DatastoreError> {
        let cmd = Command::GetEventCount(bucket_id.to_string(), starttime_opt, endtime_opt);
        match self.request(cmd)? {
            Response::Count(n) => Ok(n),
            _ => panic!("Invalid response"),
        }
    }

    pub fn delete_events_by_id(
        &self,
        bucket_id: &str,
        event_ids: Vec<i64>,
    ) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteEventsById(bucket_id.to_string(), event_ids);
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn force_commit(&self) -> Result<(), DatastoreError> {
        let cmd = Command::ForceCommit();
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn get_key_values(&self, pattern: &str) -> Result<HashMap<String, String>, DatastoreError> {
        let cmd = Command::GetKeyValues(pattern.to_string());
        match self.request(cmd)? {
            Response::KeyValues(value) => Ok(value),
            _ => panic!("Invalid response"),
        }
    }

    pub fn get_key_value(&self, key: &str) -> Result<String, DatastoreError> {
        let cmd = Command::GetKeyValue(key.to_string());
        match self.request(cmd)? {
            Response::KeyValue(kv) => Ok(kv),
            _ => panic!("Invalid response"),
        }
    }

    pub fn set_key_value(&self, key: &str, data: &str) -> Result<(), DatastoreError> {
        let cmd = Command::SetKeyValue(key.to_string(), data.to_string());
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn delete_key_value(&self, key: &str) -> Result<(), DatastoreError> {
        let cmd = Command::DeleteKeyValue(key.to_string());
        _unwrap_empty_response(self.request(cmd)?)
    }

    pub fn refresh_privacy_filter(&self) -> Result<(), DatastoreError> {
        _unwrap_empty_response(self.request(Command::RefreshPrivacyFilter())?)
    }

    /// Renames a bucket from `old_id` to `new_id`.
    pub fn rename_bucket(&self, old_id: &str, new_id: &str) -> Result<(), DatastoreError> {
        let cmd = Command::RenameBucket(old_id.to_string(), new_id.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        _unwrap_response(receiver)
    }

    /// Migrates all buckets whose hostname is "unknown" or "Unknown" to `new_hostname`.
    /// Returns the number of buckets updated.
    pub fn migrate_hostname(&self, new_hostname: &str) -> Result<usize, DatastoreError> {
        let cmd = Command::MigrateHostname(new_hostname.to_string());
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Count(n) => Ok(n as usize),
                _ => Err(DatastoreError::InternalError(
                    "Unexpected response to MigrateHostname command".to_string(),
                )),
            },
            Err(e) => Err(e),
        }
    }

    /// Migrates all buckets whose name starts with `aw-watcher-android-test` to use
    /// `aw-watcher-android` instead (e.g. debug-build buckets from older app versions).
    /// Returns the number of buckets updated.
    pub fn migrate_test_bucket_names(&self) -> Result<usize, DatastoreError> {
        let cmd = Command::MigrateTestBucketNames();
        let receiver = self.requester.request(cmd).unwrap();
        match receiver.collect().unwrap() {
            Ok(r) => match r {
                Response::Count(n) => Ok(n as usize),
                _ => Err(DatastoreError::InternalError(
                    "Unexpected response to MigrateTestBucketNames command".to_string(),
                )),
            },
            Err(e) => Err(e),
        }
    }

    // Should block until worker has stopped
    pub fn close(&self) {
        info!("Sending close request to database");
        match self.request(Command::Close()) {
            Ok(Response::Empty()) => (),
            Ok(_) => panic!("Invalid response"),
            // Worker already gone means there is nothing left to close
            Err(e) => warn!("Error closing database: {e:?}"),
        }
    }
}
