use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct User {
    pub id:i32,
    pub username: String,
    pub name: String,
    pub lastname: String,
    pub password: String,
}
