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
    fn close(&self);
}

impl AccessMethod for Datastore {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(Datastore::get_buckets(self).unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        Datastore::get_bucket(self, bucket_id)
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        Datastore::create_bucket(self, bucket)?;
        self.force_commit().unwrap();
        Ok(())
    }
    fn get_events(
        &self,
        bucket_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        Ok(Datastore::get_events(self, bucket_id, start, end, limit).unwrap())
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        Datastore::heartbeat(self, bucket_id, event, duration).unwrap();
        self.force_commit().unwrap();
        Ok(())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<(), String> {
        Datastore::insert_events(self, bucket_id, &events[..]).unwrap();
        self.force_commit().unwrap();
        Ok(())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(Datastore::get_event_count(self, bucket_id, None, None).unwrap())
    }
    fn close(&self) {
        Datastore::close(self);
    }
}

impl AccessMethod for AwClient {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(AwClient::get_buckets(self).unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let bucket = AwClient::get_bucket(self, bucket_id);
        match bucket {
            Ok(bucket) => Ok(bucket),
            Err(e) => {
                warn!("{:?}", e);
                let code = e.status().unwrap();
                if code == StatusCode::NOT_FOUND {
                    Err(DatastoreError::NoSuchBucket(bucket_id.into()))
                } else {
                    panic!("Unexpected error");
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
        Ok(AwClient::get_events(self, bucket_id, start, end, limit).unwrap())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<(), String> {
        AwClient::insert_events(self, bucket_id, events).map_err(|e| e.to_string())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(AwClient::get_event_count(self, bucket_id).unwrap())
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        AwClient::create_bucket(self, bucket).unwrap();
        Ok(())
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        AwClient::heartbeat(self, bucket_id, &event, duration).map_err(|e| format!("{e:?}"))
    }
    fn close(&self) {
        // NOP
    }
}
