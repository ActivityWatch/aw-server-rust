use std::collections::HashMap;
use std::fmt;
use std::thread;
use std::{
    fs::File,
    io::{BufWriter, Write},
};

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;

use rusqlite::Connection;
use rusqlite::DropBehavior;
use rusqlite::OpenFlags;
use rusqlite::Transaction;
use rusqlite::TransactionBehavior;

use aw_models::Bucket;
use aw_models::Event;
use aw_models::TryVec;

use crate::privacy_filter::PrivacyFilterEngine;
use crate::DatastoreError;
use crate::DatastoreInstance;
use crate::DatastoreMethod;

type RequestSender = mpsc_requests::RequestSender<Command, Result<Response, DatastoreError>>;
type RequestReceiver = mpsc_requests::RequestReceiver<Command, Result<Response, DatastoreError>>;

/// SQLite URI for a side-effect-free open: no `-wal`/`-shm`, no locks that
/// fight a file syncer. `?`/`#`/`%` in the path are encoded so they cannot
/// be parsed as the query string.
///
/// `immutable=1` is load-bearing. It tells SQLite the file cannot change, so
/// the open never creates `-wal`/`-shm` and never takes a lock — which is
/// why a pull can read a peer file in a directory this device does not own.
/// Do not drop it to "see the WAL": that reintroduces sidecars in peers'
/// folders, and a plain `mode=ro` connection then rejects `BEGIN IMMEDIATE`.
/// See [`Datastore::open_read_only`].
///
/// The connection can live across a multi-page pull. Syncthing/Dropbox write
/// a temp file and rename, so an open handle keeps the old inode on POSIX
/// (a consistent snapshot) and blocks the rename on Windows (the syncer
/// retries). Only in-place rewriting (`rsync --inplace`, a naive `cp` over
/// the file) defeats it. Copy-then-open is the belt-and-braces option if
/// that ever bites; not needed now.
///
/// Windows path normalisation (backslash → slash, drive-letter, UNC) is
/// gated on `cfg!(windows)`, not path shape. A backslash is a legal POSIX
/// filename character; rewriting it on Linux/macOS would make the probe
/// target a different file.
fn sqlite_readonly_uri(path: &str) -> String {
    let encoded = path
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23");
    if cfg!(windows) {
        let encoded = encoded.replace('\\', "/");
        let has_drive_letter = encoded.len() >= 2 && encoded.as_bytes().get(1) == Some(&b':');
        let is_unc = encoded.starts_with("//");
        if has_drive_letter {
            return format!("file:///{encoded}?mode=ro&immutable=1");
        }
        if is_unc {
            // SQLite UNC form is file:////server/share/file.db (four slashes).
            return format!("file://{encoded}?mode=ro&immutable=1");
        }
    }
    format!("file:{encoded}?mode=ro&immutable=1")
}

fn open_readonly_connection(path: &str) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        sqlite_readonly_uri(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}

/// `PRAGMA quick_check` on a separate read-only connection.
///
/// Returns the problems found (empty when the database is healthy). A file
/// too damaged to even prepare the statement is reported as a problem rather
/// than an error, so callers can tell "corrupt" from "could not check".
fn quick_check(path: &str) -> rusqlite::Result<Vec<String>> {
    let run = || -> rusqlite::Result<Vec<String>> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut stmt = conn.prepare("PRAGMA quick_check(20)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    };
    match run() {
        Ok(rows) if rows == ["ok"] => Ok(vec![]),
        Ok(rows) => Ok(rows),
        Err(rusqlite::Error::SqliteFailure(e, msg))
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            Ok(vec![msg.unwrap_or_else(|| e.to_string())])
        }
        Err(e) => Err(e),
    }
}

/// Check the database for corruption without delaying startup.
///
/// A forced kill (e.g. Windows Update rebooting the machine) can leave a
/// damaged database behind. The server then keeps running, slowly and with
/// gaps, and nothing says why (ActivityWatch/aw-server-rust#467). The check
/// reads the whole file, so it runs on its own read-only connection in the
/// background: WAL readers never block the writer.
fn spawn_integrity_check(path: String) {
    let spawned = thread::Builder::new()
        .name("aw-datastore-quick-check".to_string())
        .spawn(move || {
            let start = std::time::Instant::now();
            match quick_check(&path) {
                Ok(problems) if problems.is_empty() => info!(
                    "Database integrity check (quick_check) passed in {:.2?}",
                    start.elapsed()
                ),
                Ok(problems) => error!(
                    "Database integrity check FAILED for {path}: {}. \
                     ActivityWatch may be slow or lose data. Stop aw-server, back up \
                     the file, then recover it with \
                     `sqlite3 <db> .recover | sqlite3 <new db>` \
                     (https://sqlite.org/recovery.html) or restore a backup.",
                    problems.join("; ")
                ),
                Err(e) => warn!("Could not run database integrity check on {path}: {e}"),
            }
        });
    if let Err(e) = spawned {
        warn!("Could not start database integrity check thread: {e}");
    }
}

/// Fold the WAL back into the main file on a clean close, so a forced kill
/// later has nothing pending to replay.
fn checkpoint_wal(conn: &Connection) {
    let result = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    match result {
        Ok((0, _, _)) => debug!("Checkpointed WAL on close"),
        Ok((busy, log, done)) => warn!(
            "WAL checkpoint on close incomplete (busy={busy}, log={log}, checkpointed={done})"
        ),
        Err(e) => warn!("WAL checkpoint on close failed: {e}"),
    }
}

/// Read `user_version` without mutating the file.
fn probe_user_version(path: &str) -> Result<i32, DatastoreError> {
    let conn = open_readonly_connection(path).map_err(|e| {
        DatastoreError::InternalError(format!("read-only open failed for {path}: {e}"))
    })?;
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| {
            DatastoreError::InternalError(format!("user_version read failed for {path}: {e}"))
        })
}

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
#[derive(Debug)]
pub enum Response {
    Export(File, Option<String>),
    ExportCsv(File),
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
#[derive(Debug)]
pub enum Command {
    Export(Option<String>, File),
    ExportCsv(
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<u64>,
        File,
    ),
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

/// Key the webui writes via POST /0/settings/privacy_filters.
/// The worker's in-memory PrivacyFilterEngine is the only thing that actually
/// filters inserts/heartbeats, so every write/delete of this key (and startup)
/// must reload the engine. RefreshPrivacyFilter exists for explicit reloads.
const PRIVACY_FILTERS_KEY: &str = "settings.privacy_filters";
const STOPWATCH_BUCKET_TYPE: &str = "general.stopwatch";

fn _unwrap_empty_response(response: Response) -> Result<(), DatastoreError> {
    match response {
        Response::Empty() => Ok(()),
        _ => panic!("Invalid response"),
    }
}

/// Controls whether/how `Datastore::new*` attempts a legacy (Python
/// aw-server) database import on startup. See `ensure_legacy_import` for the
/// semantics of `force` and `db_path_override`.
#[derive(Clone, Debug, Default)]
pub struct LegacyImportOptions {
    /// Whether a legacy import should be attempted at all.
    pub enabled: bool,
    /// Run the import even if the datastore was not freshly created.
    /// Corresponds to `aw-server --import-legacy`.
    pub force: bool,
    /// Override the default `peewee-sqlite.v2.db` lookup path.
    /// Corresponds to `aw-server --legacy-dbpath <PATH>`.
    pub db_path_override: Option<String>,
}

struct DatastoreWorker {
    responder: RequestReceiver,
    legacy_import_opts: LegacyImportOptions,
    quit: bool,
    uncommitted_events: usize,
    commit: bool,
    last_heartbeat: HashMap<String, Option<Event>>,
    privacy_engine: PrivacyFilterEngine,
}

impl DatastoreWorker {
    pub fn new(
        responder: mpsc_requests::RequestReceiver<Command, Result<Response, DatastoreError>>,
        legacy_import_opts: LegacyImportOptions,
    ) -> Self {
        DatastoreWorker {
            responder,
            legacy_import_opts,
            quit: false,
            uncommitted_events: 0,
            commit: false,
            last_heartbeat: HashMap::new(),
            privacy_engine: PrivacyFilterEngine::new(vec![]),
        }
    }

    /// Replace the in-memory engine from `settings.privacy_filters`.
    /// Only an absent key clears the engine, so deleting the setting actually
    /// disables filtering. Parse errors and query errors keep the previous
    /// engine: unfiltering on a bad save or a transient database error would
    /// silently store the events these rules exist to keep out.
    fn reload_privacy_engine(&mut self, ds: &DatastoreInstance, conn: &Connection) {
        match ds.get_key_value(conn, PRIVACY_FILTERS_KEY) {
            Ok(json_str) => match PrivacyFilterEngine::from_json(&json_str) {
                Ok(engine) => self.privacy_engine = engine,
                Err(e) => warn!("Failed to parse privacy_filters setting: {e}"),
            },
            Err(DatastoreError::NoSuchKey(_)) => {
                self.privacy_engine = PrivacyFilterEngine::new(vec![]);
            }
            Err(e) => warn!("Failed to load privacy_filters setting: {e:?}"),
        }
    }

    fn work_loop(&mut self, method: DatastoreMethod) {
        let read_only = matches!(&method, DatastoreMethod::FileReadOnly(_));

        // Open SQLite connection
        let mut conn = match &method {
            DatastoreMethod::Memory() => {
                Connection::open_in_memory().expect("Failed to create in-memory datastore")
            }
            DatastoreMethod::File(path) => {
                Connection::open(path).expect("Failed to create datastore")
            }
            DatastoreMethod::FileReadOnly(path) => {
                open_readonly_connection(path).expect("Failed to open datastore read-only")
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

        // Set busy timeout to handle concurrent access on systems with strict file locking (e.g., Windows)
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .expect("Failed to set busy timeout");

        // WAL / synchronous=FULL are writes. Skip them on a peer file: a pull
        // must not create `-wal`/`-shm` in a directory this device does not
        // own (ActivityWatch/aw-server-rust#693). In-memory databases ignore
        // the request (journal_mode stays "memory").
        if !read_only {
            let journal_mode: String = conn
                .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
                .expect("Failed to query journal_mode");
            if !matches!(&method, DatastoreMethod::Memory()) && journal_mode != "wal" {
                warn!("Failed to enable WAL (journal_mode={journal_mode}), continuing without it");
            }
            conn.pragma_update(None, "synchronous", "FULL")
                .expect("Failed to set synchronous=FULL");
        }

        // After the WAL switch: that needs an exclusive lock a concurrent
        // reader would hold up. Encrypted files would need the key; skipped.
        if let DatastoreMethod::File(path) = &method {
            spawn_integrity_check(path.clone());
        }

        let mut ds = DatastoreInstance::new(&conn, !read_only).unwrap();

        // Load persisted privacy filters before serving inserts. The engine
        // starts empty; without this, rules saved in a previous process sit
        // unused until something happens to send RefreshPrivacyFilter.
        self.reload_privacy_engine(&ds, &conn);

        // Ensure legacy import
        if self.legacy_import_opts.enabled {
            let transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                Ok(transaction) => transaction,
                Err(err) => {
                    panic!("Unable to start immediate transaction on SQLite database! {err}")
                }
            };
            match ds.ensure_legacy_import(
                &transaction,
                self.legacy_import_opts.db_path_override.as_deref(),
                self.legacy_import_opts.force,
            ) {
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

        // BEGIN IMMEDIATE takes a reserved (write) lock. On a read-only
        // connection that is either SQLITE_READONLY (the worker retried
        // forever) or a no-op depending on SQLite version — Deferred is the
        // correct read-only behavior either way.
        let tx_behavior = if read_only {
            TransactionBehavior::Deferred
        } else {
            TransactionBehavior::Immediate
        };

        // Start handling and respond to requests
        loop {
            let last_commit_time: DateTime<Utc> = Utc::now();
            let mut tx: Transaction = match conn.transaction_with_behavior(tx_behavior) {
                Ok(tx) => tx,
                Err(err) => {
                    error!("Unable to start transaction! {:?}", err);
                    // Wait 1s before retrying
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    continue;
                }
            };
            // Snapshot BEFORE the request loop. SetKeyValue/DeleteKeyValue
            // reload the engine from the still-open transaction so a later
            // insert in the same batch is filtered. If commit fails we restore
            // this snapshot — not "keep current" (that is the rolled-back
            // view) and not a durable re-query (a read error would leave the
            // uncommitted engine in place: fail-open after a rolled-back delete).
            let privacy_engine_at_tx_start = self.privacy_engine.clone();
            tx.set_drop_behavior(DropBehavior::Commit);

            self.uncommitted_events = 0;
            self.commit = false;
            // Commands that force a commit are acknowledged only after it
            // succeeds. Other commands can return before the batch commits.
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
                let response = self.handle_request(request, &mut ds, &tx);
                if self.commit || self.quit {
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
                    // Before acking Close: the caller may exit right after.
                    if self.quit && !read_only {
                        checkpoint_wal(&conn);
                    }
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
                    //
                    // Restore the pre-transaction engine. Reloading from the durable
                    // connection is not enough: if that read fails, keep-on-error would
                    // preserve the uncommitted engine (empty after a rolled-back delete).
                    self.privacy_engine = privacy_engine_at_tx_start;
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
            Command::Export(bucket_id, mut file) => {
                let mut writer = BufWriter::new(&mut file);
                let name = ds.write_export(tx, bucket_id.as_deref(), &mut writer)?;
                writer.flush().map_err(|err| {
                    DatastoreError::InternalError(format!("Failed to flush export: {err}"))
                })?;
                drop(writer);
                Ok(Response::Export(file, name))
            }
            Command::ExportCsv(bucket_id, start, end, limit, mut file) => {
                let mut writer = BufWriter::new(&mut file);
                ds.write_events_csv(tx, &bucket_id, start, end, limit, &mut writer)?;
                writer.flush().map_err(|err| {
                    DatastoreError::InternalError(format!("Failed to flush CSV export: {err}"))
                })?;
                drop(writer);
                Ok(Response::ExportCsv(file))
            }
            Command::CreateBucket(mut bucket) => {
                // Attached events (import) must use the same privacy gate as
                // InsertEvents, or a new-bucket import stores data the rules
                // would drop or redact.
                if let Some(events) = bucket.events.take() {
                    let filtered = self
                        .privacy_engine
                        .filter_events(&bucket.id, events.take_inner());
                    if !filtered.is_empty() {
                        bucket.events = Some(TryVec::new(filtered));
                    }
                }
                match ds.create_bucket(tx, bucket) {
                    Ok(_) => {
                        self.commit = true;
                        Ok(Response::Empty())
                    }
                    Err(e) => Err(e),
                }
            }
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

                        // Manual timer changes must be durable before the UI confirms them.
                        self.commit |= ds.get_bucket(&bucketname)?._type == STOPWATCH_BUCKET_TYPE;
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
                        self.commit |= ds.get_bucket(&bucketname)?._type == STOPWATCH_BUCKET_TYPE;
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
                    Ok(()) => {
                        self.commit |= ds.get_bucket(&bucketname)?._type == STOPWATCH_BUCKET_TYPE;
                        Ok(Response::Empty())
                    }
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
                Ok(()) => {
                    if key == PRIVACY_FILTERS_KEY {
                        self.reload_privacy_engine(ds, tx);
                    }
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::GetKeyValue(key) => match ds.get_key_value(tx, &key) {
                Ok(result) => Ok(Response::KeyValue(result)),
                Err(e) => Err(e),
            },
            Command::DeleteKeyValue(key) => match ds.delete_key_value(tx, &key) {
                Ok(()) => {
                    if key == PRIVACY_FILTERS_KEY {
                        self.reload_privacy_engine(ds, tx);
                    }
                    Ok(Response::Empty())
                }
                Err(e) => Err(e),
            },
            Command::RefreshPrivacyFilter() => {
                self.reload_privacy_engine(ds, tx);
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
            Command::MigrateTestBucketNames() => match ds.migrate_test_bucket_names(tx) {
                Ok(count) => {
                    if count > 0 {
                        self.commit = true;
                    }
                    Ok(Response::Count(count as i64))
                }
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
        Datastore::new_with_legacy_import_opts(
            dbpath,
            LegacyImportOptions {
                enabled: legacy_import,
                ..Default::default()
            },
        )
    }

    /// Like [`Datastore::new`], but with full control over the legacy
    /// import behavior (forcing it, overriding the lookup path). Used by
    /// `aw-server --import-legacy` / `--legacy-dbpath`.
    pub fn new_with_legacy_import_opts(
        dbpath: String,
        legacy_import_opts: LegacyImportOptions,
    ) -> Self {
        let method = DatastoreMethod::File(dbpath);
        Datastore::_new_internal(method, legacy_import_opts)
    }

    /// Open an existing database without writing to it.
    ///
    /// Uses `file:…?mode=ro&immutable=1`, never runs migrations, never sets
    /// `journal_mode`/`synchronous`. Returns `OldDbVersion` when
    /// `user_version` is outside
    /// `MIN_READ_COMPAT_DB_VERSION..=NEWEST_DB_VERSION` so a caller can skip
    /// that peer (ActivityWatch/aw-server-rust#693). Older versions in that
    /// range differ only in indexes, which queries adapt to.
    ///
    /// `immutable=1` means SQLite will not look at a peer's `-wal`/`-shm`.
    /// Committed-but-not-yet-checkpointed frames in that WAL are therefore
    /// invisible. aw-sync checkpoints on clean close, so the steady-state
    /// file is self-contained; this only bites mid-push. A stale-but-
    /// consistent snapshot is strictly better than a torn one, and dropping
    /// `immutable` would reintroduce `-shm` files in a foreign directory.
    pub fn open_read_only(dbpath: String) -> Result<Self, DatastoreError> {
        let version = probe_user_version(&dbpath)?;
        let supported = crate::MIN_READ_COMPAT_DB_VERSION..=crate::NEWEST_DB_VERSION;
        if !supported.contains(&version) {
            return Err(DatastoreError::OldDbVersion(format!(
                "Tried to open a database with an incompatible database version! \
                 Database has version {version} while the supported versions are {}..={}",
                crate::MIN_READ_COMPAT_DB_VERSION,
                crate::NEWEST_DB_VERSION
            )));
        }
        Ok(Datastore::_new_internal(
            DatastoreMethod::FileReadOnly(dbpath),
            LegacyImportOptions::default(),
        ))
    }

    pub fn new_in_memory(legacy_import: bool) -> Self {
        let method = DatastoreMethod::Memory();
        Datastore::_new_internal(
            method,
            LegacyImportOptions {
                enabled: legacy_import,
                ..Default::default()
            },
        )
    }

    /// Create an encrypted datastore using SQLCipher.
    ///
    /// Requires the `encryption` or `encryption-vendored` feature flag.
    /// Build with: `cargo build --no-default-features --features encryption`
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    pub fn new_encrypted(dbpath: String, key: String, legacy_import: bool) -> Self {
        Datastore::new_encrypted_with_legacy_import_opts(
            dbpath,
            key,
            LegacyImportOptions {
                enabled: legacy_import,
                ..Default::default()
            },
        )
    }

    /// Like [`Datastore::new_encrypted`], but with full control over the
    /// legacy import behavior. See [`Datastore::new_with_legacy_import_opts`].
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    pub fn new_encrypted_with_legacy_import_opts(
        dbpath: String,
        key: String,
        legacy_import_opts: LegacyImportOptions,
    ) -> Self {
        let method = DatastoreMethod::FileEncrypted(dbpath, zeroize::Zeroizing::new(key));
        Datastore::_new_internal(method, legacy_import_opts)
    }

    fn _new_internal(method: DatastoreMethod, legacy_import_opts: LegacyImportOptions) -> Self {
        let (requester, responder) =
            mpsc_requests::channel::<Command, Result<Response, DatastoreError>>();
        let _thread = thread::spawn(move || {
            let mut di = DatastoreWorker::new(responder, legacy_import_opts);
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

    /// Write a consistent export to an empty temporary file. The file is
    /// returned only after serialization and flushing succeed. Event rows are
    /// serialized individually, including this worker's uncommitted writes.
    pub fn export_to_file(
        &self,
        bucket_id: Option<&str>,
        file: File,
    ) -> Result<(File, Option<String>), DatastoreError> {
        match self.request(Command::Export(bucket_id.map(str::to_owned), file))? {
            Response::Export(file, name) => Ok((file, name)),
            _ => panic!("Invalid response"),
        }
    }

    /// Stream one bucket's events as CSV into `file`, one SQL row at a time.
    ///
    /// Same snapshot rules as [`Datastore::export_to_file`]: the worker writes
    /// including uncommitted events, and the file is returned only after
    /// serialization and flush succeed.
    pub fn export_csv_to_file(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
        file: File,
    ) -> Result<File, DatastoreError> {
        match self.request(Command::ExportCsv(
            bucket_id.to_owned(),
            start,
            end,
            limit,
            file,
        ))? {
            Response::ExportCsv(file) => Ok(file),
            _ => panic!("Invalid response"),
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
        _unwrap_empty_response(self.request(cmd)?)
    }

    /// Migrates all buckets whose hostname is "unknown" or "Unknown" to `new_hostname`.
    /// Returns the number of buckets updated.
    pub fn migrate_hostname(&self, new_hostname: &str) -> Result<usize, DatastoreError> {
        let cmd = Command::MigrateHostname(new_hostname.to_string());
        match self.request(cmd)? {
            Response::Count(n) => Ok(n as usize),
            _ => Err(DatastoreError::InternalError(
                "Unexpected response to MigrateHostname command".to_string(),
            )),
        }
    }

    /// Migrates all buckets whose name starts with `aw-watcher-android-test` to use
    /// `aw-watcher-android` instead (e.g. debug-build buckets from older app versions).
    /// Returns the number of buckets updated.
    pub fn migrate_test_bucket_names(&self) -> Result<usize, DatastoreError> {
        let cmd = Command::MigrateTestBucketNames();
        match self.request(cmd)? {
            Response::Count(n) => Ok(n as usize),
            _ => Err(DatastoreError::InternalError(
                "Unexpected response to MigrateTestBucketNames command".to_string(),
            )),
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

#[cfg(test)]
mod sqlite_readonly_uri_tests {
    use super::sqlite_readonly_uri;

    #[test]
    fn posix_absolute() {
        assert_eq!(
            sqlite_readonly_uri("/var/lib/activitywatch/peer.db"),
            "file:/var/lib/activitywatch/peer.db?mode=ro&immutable=1"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn posix_path_with_literal_backslash_is_not_rewritten() {
        // A backslash is a valid POSIX filename character. Windows
        // normalisation is cfg!(windows)-gated so this path is passed
        // through untouched on Linux/macOS.
        assert_eq!(
            sqlite_readonly_uri(r"/home/erik/we\ird.db"),
            r"file:/home/erik/we\ird.db?mode=ro&immutable=1"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_drive_letter() {
        assert_eq!(
            sqlite_readonly_uri(r"C:\Users\bob\peer.db"),
            "file:///C:/Users/bob/peer.db?mode=ro&immutable=1"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_unc() {
        assert_eq!(
            sqlite_readonly_uri(r"\\server\share\peer.db"),
            "file:////server/share/peer.db?mode=ro&immutable=1"
        );
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::{quick_check, Datastore};
    use rusqlite::Connection;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn quick_check_passes_on_fresh_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db").to_str().unwrap().to_string();
        let ds = Datastore::new(path.clone(), false);
        ds.force_commit().unwrap();
        assert_eq!(quick_check(&path).unwrap(), Vec::<String>::new());
        ds.close();
    }

    #[test]
    fn quick_check_reports_corrupt_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, data TEXT);
                 CREATE INDEX t_data ON t (data);",
            )
            .unwrap();
            let tx = conn.transaction().unwrap();
            for i in 0..2000 {
                tx.execute(
                    "INSERT INTO t (data) VALUES (?1)",
                    [format!("row {i:0>64}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        // Overwrite a stretch of b-tree pages (not the header page) with garbage.
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(4096 * 3)).unwrap();
        file.write_all(&[0xAB; 4096 * 4]).unwrap();
        drop(file);

        let problems = quick_check(path.to_str().unwrap()).unwrap();
        assert!(!problems.is_empty(), "corruption went unreported");
    }

    #[test]
    fn close_truncates_wal_while_another_connection_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal.db");
        let path_str = path.to_str().unwrap().to_string();
        let ds = Datastore::new(path_str.clone(), false);
        ds.set_key_value("test.key", "value").unwrap();
        ds.force_commit().unwrap();

        // A second open connection stops SQLite's last-close auto-checkpoint,
        // as a reader (aw-sync, the integrity check) would in production.
        // `Connection::open` is lazy: only a first read makes the reader take
        // its WAL-mode shared lock. Without it the worker's connection is the
        // last one and deletes the WAL when dropped, which races the assert
        // below (Close is acked before the connection drops).
        let reader = Connection::open(&path).unwrap();
        reader
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        let wal = dir.path().join("wal.db-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);

        ds.close();
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
    }
}
