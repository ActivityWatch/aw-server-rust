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
mod legacy_import;
mod worker;

pub use self::datastore::DatastoreInstance;
pub use self::worker::Datastore;

#[derive(Clone)]
pub enum DatastoreMethod {
    Memory(),
    File(String),
    /// Encrypted SQLite file using SQLCipher. Only available with the
    /// `encryption` or `encryption-vendored` feature flags.
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    FileEncrypted(String, String), // (path, key)
}

impl fmt::Debug for DatastoreMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DatastoreMethod::Memory() => write!(f, "Memory()"),
            DatastoreMethod::File(p) => write!(f, "File({p:?})"),
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
    MpscError,
    InternalError(String),
    // Errors specific to when migrate is disabled
    Uninitialized(String),
    OldDbVersion(String),
}
