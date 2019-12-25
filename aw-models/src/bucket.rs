use chrono::DateTime;
use chrono::Utc;
use std::collections::HashMap;
use serde_json::value::Value;
use serde_json::map::Map;

use crate::Event;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bucket {
    #[serde(skip)]
    pub bid: Option<i64>,
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")] /* type is a reserved Rust keyword */
    pub _type: String,
    pub client: String,
    pub hostname: String,
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub data: Map<String, Value>,
    #[serde(default, skip_deserializing)]
    pub metadata: BucketMetadata,
    pub events: Option<Vec<Event>>, /* Should only be set during import/export */
    pub last_updated: Option<DateTime<Utc>>, // TODO: Should probably be moved into metadata field
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BucketMetadata {
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

impl Default for BucketMetadata {
    fn default() -> BucketMetadata {
        BucketMetadata {
            start: None,
            end: None,
        }
    }
}

#[derive(Clone,Serialize,Deserialize)]
pub struct BucketsExport {
    pub buckets: HashMap<String, Bucket>,
}
