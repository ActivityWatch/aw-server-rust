mod datastore;
mod worker;

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
