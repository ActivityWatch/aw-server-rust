use chrono::DateTime;
use chrono::Utc;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bucket {
    pub id: String,
    #[serde(rename = "type")] /* type is a reserved Rust keyword */
    pub _type: String,
    pub client: String,
    pub hostname: String,
    pub created: Option<DateTime<Utc>>
}
