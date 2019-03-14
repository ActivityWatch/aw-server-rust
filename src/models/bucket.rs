use chrono::DateTime;
use chrono::Utc;
use std::collections::HashMap;

use crate::models::Event;

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
    pub events: Option<Vec<Event>>,
}

#[derive(Clone,Serialize,Deserialize)]
pub struct BucketsExport {
    pub buckets: HashMap<String, Bucket>,
}
