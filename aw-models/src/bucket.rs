use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::map::Map;
use serde_json::value::Value;
use std::collections::HashMap;

use crate::Event;
use crate::TryVec;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct Bucket {
    #[serde(skip)]
    pub bid: Option<i64>,
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")] /* type is a reserved Rust keyword */ pub _type: String,
    pub client: String,
    pub hostname: String,
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub data: Map<String, Value>,
    #[serde(default, skip_deserializing)]
    pub metadata: BucketMetadata,
    // Events should only be "Some" during import/export
    // It's using a TryVec to discard only the events which were failed to be serialized so only a
    // few events are being dropped during import instead of failing the whole import
    pub events: Option<TryVec<Event>>,
    pub last_updated: Option<DateTime<Utc>>, // TODO: Should probably be moved into metadata field
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct BucketMetadata {
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct BucketsExport {
    pub buckets: HashMap<String, Bucket>,
}

#[test]
fn test_bucket() {
    let b = Bucket {
        bid: None,
        id: "id".to_string(),
        _type: "type".to_string(),
        client: "client".to_string(),
        hostname: "hostname".into(),
        created: None,
        data: json_map! {},
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    };
    debug!("bucket: {:?}", b);
}
