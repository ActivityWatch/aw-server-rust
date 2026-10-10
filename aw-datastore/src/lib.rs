#[macro_use]
extern crate log;

use std::fmt;

#[macro_export]
macro_rules! json_map {
    { $( $key:literal : $value:expr),* } => {{
        use serde_json::Value;
        use serde_json::map::Map;
        #[allow(unused_mut)]
        let mut map : Map<String, Value> = Map::new();
        $(
          map.insert( $key.to_string(), json!($value) );
        )*
        map
    }};
}

mod datastore;
mod export;
mod legacy_import;
mod privacy_filter;
mod worker;

pub use self::datastore::DatastoreInstance;
pub use self::datastore::MIN_READ_COMPAT_DB_VERSION;
pub use self::datastore::NEWEST_DB_VERSION;

pub use self::worker::Datastore;
pub use self::worker::LegacyImportOptions;

#[derive(Clone)]
pub enum DatastoreMethod {
    Memory(),
    File(String),
    /// Existing file, opened `mode=ro&immutable=1`. Never migrates, never
    /// creates `-wal`/`-shm`. Used by aw-sync when reading a peer's db.
    FileReadOnly(String),
    /// Existing file opened read-only on a second connection, sharing the
    /// `-wal`/`-shm` with a writer in the same process, so it sees every
    /// committed write. Never migrates. Used by aw-server to serve query
    /// reads without occupying the writer's worker (#805).
    FileReader(String),
    /// Encrypted SQLite file using SQLCipher. Only available with the
    /// `encryption` or `encryption-vendored` feature flags.
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    FileEncrypted(String, zeroize::Zeroizing<String>), // (path, key)
}

impl fmt::Debug for DatastoreMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DatastoreMethod::Memory() => write!(f, "Memory()"),
            DatastoreMethod::File(p) => write!(f, "File({p:?})"),
            DatastoreMethod::FileReadOnly(p) => write!(f, "FileReadOnly({p:?})"),
            DatastoreMethod::FileReader(p) => write!(f, "FileReader({p:?})"),
            #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
            DatastoreMethod::FileEncrypted(p, _) => write!(f, "FileEncrypted({p:?}, <redacted>)"),
        }
    }
}

/* TODO: Implement this as a proper error */
#[derive(Debug, Clone)]
pub enum DatastoreError {
    NoSuchBucket(String),
    BucketAlreadyExists(String),
    NoSuchKey(String),
    NoSuchEvent(String, i64),
    MpscError,
    InternalError(String),
    // Errors specific to when migrate is disabled
    Uninitialized(String),
    OldDbVersion(String),
}

/// Run `PRAGMA quick_check` on a plain (unencrypted) SQLite file.
///
/// Returns `Ok(true)` if the check passed, `Ok(false)` if `path` is not a
/// plain SQLite file (e.g. SQLCipher-encrypted, which cannot be checked
/// without the key), and `Err` if it is a SQLite file that fails the check
/// or cannot be opened. Opens the file read-write so a sibling `-wal` is
/// applied: only call it on a copy you own (aw-server uses it to verify a
/// migrated database before switching to it).
pub fn sqlite_quick_check(path: &std::path::Path) -> Result<bool, String> {
    use std::io::Read;
    let mut header = [0u8; 16];
    let is_plain_sqlite = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map(|_| &header == b"SQLite format 3\0")
        .unwrap_or(false);
    if !is_plain_sqlite {
        return Ok(false);
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("could not open {path:?}: {e}"))?;
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|e| format!("quick_check failed on {path:?}: {e}"))?;
    if result == "ok" {
        Ok(true)
    } else {
        Err(format!("quick_check on {path:?} reported: {result}"))
    }
}
