use crate::TimeInterval;

#[derive(Deserialize, Clone, Debug)]
pub struct Query {
    //#[serde(with = "DurationSerialization")]
    pub timeperiods: Vec<TimeInterval>,
    pub query: Vec<String>,
}
