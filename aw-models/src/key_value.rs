use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Key {
    pub key: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: Value,
    pub timestamp: Option<DateTime<Utc>>,
}

impl KeyValue {
    pub fn new<K: Into<String>, V: Into<Value>>(
        key: K,
        value: V,
        timestamp: DateTime<Utc>,
    ) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: value.into(),
            timestamp: Some(timestamp),
        }
    }
}
