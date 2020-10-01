#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct BucketsExport {
    pub buckets: HashMap<String, Bucket>,
}
