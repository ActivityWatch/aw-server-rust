use serde_json::Value;
use chrono::DateTime;
use chrono::Utc;

use super::duration::Duration;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub duration: Duration,
    pub data: Value, /* TODO: force this to be a value::Object somehow */
}

