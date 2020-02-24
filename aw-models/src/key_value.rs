#[derive(Serialize, Deserialize)]
pub struct Key {
    pub key: String
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeyValue {
    pub key: String,
    pub value: String
}

