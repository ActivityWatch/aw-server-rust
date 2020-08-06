use serde::Deserialize;

use crate::TimeInterval;

// TODO Implement serialize once TimeInterval has implemented it
#[derive(Deserialize, Clone, Debug)]
pub struct Query {
    //#[serde(with = "DurationSerialization")]
    pub timeperiods: Vec<TimeInterval>,
    pub query: Vec<String>,
}
