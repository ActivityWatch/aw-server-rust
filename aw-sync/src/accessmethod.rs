use std::collections::HashMap;

use aw_client_rust::AwClient;
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
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String>;
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String>;
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String>;
    fn close(&self);
}

impl AccessMethod for Datastore {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(self.get_buckets().unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        self.get_bucket(bucket_id)
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        self.create_bucket(bucket)?;
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
        Ok(self.get_events(bucket_id, start, end, limit).unwrap())
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        self.heartbeat(bucket_id, event, duration).unwrap();
        self.force_commit().unwrap();
        Ok(())
    }
    fn insert_events(&self, bucket_id: &str, events: Vec<Event>) -> Result<Vec<Event>, String> {
        let res = self.insert_events(bucket_id, &events[..]).unwrap();
        self.force_commit().unwrap();
        Ok(res)
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(self.get_event_count(bucket_id, None, None).unwrap())
    }
    fn close(&self) {
        self.close();
    }
}

impl AccessMethod for AwClient {
    fn get_buckets(&self) -> Result<HashMap<String, Bucket>, String> {
        Ok(self.get_buckets().unwrap())
    }
    fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let bucket = self.get_bucket(bucket_id);
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
        Ok(self.get_events(bucket_id, start, end, limit).unwrap())
    }
    fn insert_events(&self, _bucket_id: &str, _events: Vec<Event>) -> Result<Vec<Event>, String> {
        //Ok(self.insert_events(bucket_id, &events[..]).unwrap())
        Err("Not implemented".to_string())
    }
    fn get_event_count(&self, bucket_id: &str) -> Result<i64, String> {
        Ok(self.get_event_count(bucket_id).unwrap())
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), DatastoreError> {
        self.create_bucket(bucket.id.as_str(), bucket._type.as_str())
            .unwrap();
        Ok(())
        //Err(DatastoreError::InternalError("Not implemented".to_string()))
    }
    fn heartbeat(&self, bucket_id: &str, event: Event, duration: f64) -> Result<(), String> {
        self.heartbeat(bucket_id, &event, duration)
            .map_err(|e| format!("{:?}", e))
    }
    fn close(&self) {
        // NOP
    }
}
