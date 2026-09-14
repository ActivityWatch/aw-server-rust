use std::collections::HashMap;

use aw_client_rust::blocking::AwClient;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;

use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, Event};

// This trait should be implemented by both AwClient and Datastore, unifying them under a single API
pub trait AccessMethod: std::fmt::Debug {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String>;
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError>;
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError>;
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String>;
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<(), String>;
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String>;
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String>;
    fn delete_events_by_id(&self, bucket_id: &str, event_ids: Vec<i64>) -> Result<(), String>;
}

/// Every method here returns a `Result`, so a datastore failure must be reported
/// through it rather than unwrapped.
///
/// These used to `unwrap()`, which on Android turns any datastore error (a
/// dead worker thread, a locked or corrupt database, a disappearing sync file)
/// into a panic that unwinds out of the JNI `extern "C"` frame and aborts the
/// whole app — see ActivityWatch/aw-android#220.
impl AccessMethod for Datastore {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Datastore::get_buckets(self).map_err(|e| format!("{e:?}"))
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        Datastore::get_bucket(self, bucket_id)
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        Datastore::create_bucket(self, bucket)?;
        self.force_commit()?;
        Ok(())
    }
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        Datastore::get_events(self, bucket_id, start, end, limit).map_err(|e| format!("{e:?}"))
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        Datastore::heartbeat(self, bucket_id, event, duration).map_err(|e| format!("{e:?}"))?;
        self.force_commit().map_err(|e| format!("{e:?}"))?;
        Ok(())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<(), String> {
        Datastore::insert_events(self, bucket_id, &events[..]).map_err(|e| format!("{e:?}"))?;
        self.force_commit().map_err(|e| format!("{e:?}"))?;
        Ok(())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Datastore::get_event_count(self, bucket_id, None, None).map_err(|e| format!("{e:?}"))
    }
    fn delete_events_by_id(&self, bucket_id: &str, event_ids: Vec<i64>) -> Result<(), String> {
        Datastore::delete_events_by_id(self, bucket_id, event_ids).map_err(|e| format!("{e:?}"))?;
        self.force_commit().unwrap();
        Ok(())
    }
}

impl AccessMethod for AwClient {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        AwClient::get_buckets(self).map_err(|e| e.to_string())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let bucket = AwClient::get_bucket(self, bucket_id);
        match bucket {
            Ok(bucket) => Ok(bucket),
            Err(e) => {
                warn!("{:?}", e);
                let code = e.status().unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                if code == StatusCode::NOT_FOUND {
                    Err(DatastoreError::NoSuchBucket(bucket_id.into()))
                } else {
                    Err(DatastoreError::InternalError(format!(
                        "Unexpected error fetching bucket {bucket_id}: {e}"
                    )))
                }
            }
        }
    }
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        AwClient::get_events(self, bucket_id, start, end, limit).map_err(|e| e.to_string())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<(), String> {
        AwClient::insert_events(self, bucket_id, events).map_err(|e| e.to_string())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        AwClient::get_event_count(self, bucket_id).map_err(|e| e.to_string())
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        AwClient::create_bucket(self, bucket)
            .map_err(|e| DatastoreError::InternalError(e.to_string()))?;
        Ok(())
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        AwClient::heartbeat(self, bucket_id, &event, duration).map_err(|e| format!("{e:?}"))
    }
    fn delete_events_by_id(&self, bucket_id: &str, event_ids: Vec<i64>) -> Result<(), String> {
        for event_id in event_ids {
            AwClient::delete_event(self, bucket_id, event_id).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
