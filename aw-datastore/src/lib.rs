#[macro_use] extern crate log;

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
mod worker;
mod legacy_import;

pub use self::datastore::DatastoreInstance;
pub use self::worker::Datastore;

pub enum DatastoreMethod {
    Memory(),
    File(String),
}

/* TODO: Implement this as a proper error */
#[derive(Debug,Clone)]
pub enum DatastoreError {
    NoSuchBucket,
    BucketAlreadyExists,
    MpscError,
    ReadOnly,
    InternalError(String),
}
