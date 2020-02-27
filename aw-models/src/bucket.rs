use chrono::DateTime;
use chrono::Utc;
use serde_json::map::Map;
use serde_json::value::Value;
use std::collections::HashMap;

use crate::Event;

#[derive(Serialize, Deserialize, Clone, Debug)]
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

#[derive(Clone, Serialize, Deserialize)]
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
        hostname: "hostname".to_string(),
        created: None,
        data: json_map! {},
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    };
    debug!("bucket: {:?}", b);
}
