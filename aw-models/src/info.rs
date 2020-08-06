use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Info {
    pub hostname: String,
    pub version: String,
    pub testing: bool,
    pub device_id: String,
}
