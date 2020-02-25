use chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize)]
pub struct Key {
    pub key: String
}

// TODO: Invent a better naming scheme than calling the non-timestampy one "KV"
#[derive(Serialize, Deserialize, Debug)]
pub struct KV {
    pub key: String,
    pub value: String
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub timestamp: DateTime<Utc> 
}

impl KeyValue {
    pub fn new<T: Into<String>>(key: T, value: T, timestamp: DateTime<Utc>) -> KeyValue {
        KeyValue {key: key.into(), value: value.into(), timestamp: timestamp }
    }
}

