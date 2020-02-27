use chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize)]
pub struct Key {
    pub key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub timestamp: Option<DateTime<Utc>>,
}

impl KeyValue {
    pub fn new<T: Into<String>>(key: T, value: T, timestamp: DateTime<Utc>) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: value.into(),
            timestamp: Some(timestamp),
        }
    }
}
