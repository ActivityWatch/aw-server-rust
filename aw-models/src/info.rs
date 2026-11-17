use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Info {
    pub hostname: String,
    pub version: String,
    pub testing: bool,
    /// Name of the running instance profile ("default" unless `--profile` was
    /// given). Lets clients tell concurrent instances apart. Defaults on
    /// deserialization so a new client can still talk to an older server.
    #[serde(default = "default_profile")]
    pub profile: String,
    pub device_id: String,
}

fn default_profile() -> String {
    "default".to_string()
}
