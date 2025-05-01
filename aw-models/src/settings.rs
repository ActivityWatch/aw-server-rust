use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct NewReleaseCheckData {
    pub how_often_to_check: i64,
    pub is_enabled: bool,
    pub next_check_time: String,
    pub times_checked: i32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct UserSatisfactionPollData {
    pub is_enabled: bool,
    pub next_poll_time: String,
    pub times_poll_is_shown: i32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct ViewElement {
    pub size: i32,
    #[serde(rename = "type")]
    pub element_type: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct View {
    pub elements: Vec<ViewElement>,
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct ClassData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i32>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct ClassRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(rename = "type")]
    pub rule_type: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct Class {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ClassData>,
    pub id: i32,
    pub name: Vec<String>,
    pub rule: ClassRule,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub landing_page: String,
    pub start_of_day: String,
    pub always_active_pattern: String,
    pub new_release_check_data: NewReleaseCheckData,
    pub start_of_week: String,
    pub use_color_fallback: bool,
    pub user_satisfaction_poll_data: UserSatisfactionPollData,
    pub request_timeout: i32,
    pub devmode: bool,
    pub theme: String,
    pub views: Vec<View>,
    pub duration_default: i32,
    pub classes: Vec<Class>,
    pub initial_timestamp: String,
}
