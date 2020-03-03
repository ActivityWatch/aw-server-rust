use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use serde_json::Map;
use serde_json::Value;

use crate::duration::DurationSerialization;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    #[serde(with = "DurationSerialization", default = "default_duration")]
    pub duration: Duration,
    pub data: Map<String, Value>,
}

impl Event {
    pub fn calculate_endtime(&self) -> DateTime<Utc> {
        self.timestamp
            + chrono::Duration::nanoseconds(self.duration.num_nanoseconds().unwrap() as i64)
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Event) -> bool {
        !(self.timestamp != other.timestamp ||
            self.duration != other.duration ||
            self.data != other.data)
    }
}

impl Default for Event {
    fn default() -> Self {
        Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: serde_json::Map::new(),
        }
    }
}

fn default_duration() -> Duration {
    Duration::seconds(0)
}

#[test]
fn test_event() {
    let e = Event {
        id: None,
        timestamp: Utc::now(),
        duration: Duration::seconds(1),
        data: json_map! {"test": json!(1)},
    };
    debug!("event: {:?}", e);
}
