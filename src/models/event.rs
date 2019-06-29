use serde_json::Value;
use serde_json::Map;
use chrono::DateTime;
use chrono::Utc;
use chrono::Duration;

use crate::models::duration::DurationSerialization;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    #[serde(with = "DurationSerialization")]
    pub duration: Duration,
    pub data: Map<String, Value>,
}

impl Event {
    pub fn calculate_endtime(&self) -> DateTime<Utc> {
        self.timestamp + chrono::Duration::nanoseconds(self.duration.num_nanoseconds().unwrap() as i64)
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Event) -> bool {
        if self.timestamp != other.timestamp { return false; }
        if self.duration != other.duration { return false; }
        if self.data != other.data { return false; }
        return true;
    }
}
