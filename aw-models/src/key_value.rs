use chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize)]
pub struct Key {
    pub key: String
}

// TODO: Invent a better naming scheme than calling the non-timestampy one "KV"
#[derive(Serialize, Deserialize)]
pub struct KV {
    pub key: String,
    pub value: String
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub timestamp: DateTime<Utc> 
}

