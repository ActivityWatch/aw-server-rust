//! Default classes
//!
//! Taken from default classes in aw-webui

use serde::{Deserialize, Serialize};
use serde_json;

pub type CategoryId = Vec<String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySpec {
    #[serde(rename = "type")]
    pub spec_type: String,
    #[serde(default)]
    pub regex: String,
    #[serde(default)]
    pub ignore_case: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassSetting {
    #[serde(default)]
    pub id: Option<i32>,
    pub name: Vec<String>,
    pub rule: CategorySpec,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Parse categorization classes from a settings JSON value.
///
/// Accepts the payload the webui writes to `settings.classes`: a JSON array of
/// `{name, rule, ...}` objects. `id` is optional (the webui assigns it in
/// memory). `null`, empty arrays, missing fields, or unparseable JSON fall
/// back to [`default_classes`].
///
/// A JSON *string* is treated as double-encoded payload (the localStorage →
/// server settings migration bug, ActivityWatch/activitywatch#1067) and decoded
/// once more before parsing.
pub fn classes_from_settings_json(value: &serde_json::Value) -> Vec<(CategoryId, CategorySpec)> {
    let value = decode_maybe_double_encoded(value);
    match value {
        serde_json::Value::Null => default_classes(),
        serde_json::Value::Array(ref arr) if arr.is_empty() => default_classes(),
        other => match serde_json::from_value::<Vec<ClassSetting>>(other) {
            Ok(classes) if classes.is_empty() => default_classes(),
            Ok(classes) => classes.into_iter().map(|c| (c.name, c.rule)).collect(),
            Err(e) => {
                log::warn!("Failed to parse settings.classes, using defaults: {:?}", e);
                default_classes()
            }
        },
    }
}

/// Parse `settings.classes` from the raw datastore/HTTP body string.
pub fn classes_from_settings_str(raw: &str) -> Vec<(CategoryId, CategorySpec)> {
    match serde_json::from_str::<serde_json::Value>(raw.trim()) {
        Ok(v) => classes_from_settings_json(&v),
        Err(e) => {
            log::warn!(
                "settings.classes is not valid JSON, using defaults: {:?}",
                e
            );
            default_classes()
        }
    }
}

fn decode_maybe_double_encoded(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
        _ => value.clone(),
    }
}

/// Returns the default categorization classes
pub fn default_classes() -> Vec<(CategoryId, CategorySpec)> {
    vec![
        (
            vec!["Work".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Google Docs|libreoffice|ReText".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Work".to_string(), "Programming".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "GitHub|Stack Overflow|BitBucket|Gitlab|vim|Spyder|kate|Ghidra|Scite"
                    .to_string(),
                ignore_case: false,
            },
        ),
        (
            vec![
                "Work".to_string(),
                "Programming".to_string(),
                "ActivityWatch".to_string(),
            ],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "ActivityWatch|aw-".to_string(),
                ignore_case: true,
            },
        ),
        (
            vec!["Work".to_string(), "Image".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Gimp|Inkscape".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Work".to_string(), "Video".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Kdenlive".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Work".to_string(), "Audio".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Audacity".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Work".to_string(), "3D".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Blender".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Media".to_string(), "Games".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Minecraft|RimWorld".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Media".to_string(), "Video".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "YouTube|Plex|VLC".to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Media".to_string(), "Social Media".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "reddit|Facebook|Twitter|Instagram|devRant".to_string(),
                ignore_case: true,
            },
        ),
        (
            vec!["Media".to_string(), "Music".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Spotify|Deezer".to_string(),
                ignore_case: true,
            },
        ),
        (
            vec!["Comms".to_string(), "IM".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Messenger|Telegram|Signal|WhatsApp|Rambox|Slack|Riot|Discord|Nheko"
                    .to_string(),
                ignore_case: false,
            },
        ),
        (
            vec!["Comms".to_string(), "Email".to_string()],
            CategorySpec {
                spec_type: "regex".to_string(),
                regex: "Gmail|Thunderbird|mutt|alpine".to_string(),
                ignore_case: false,
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn work_regex(classes: &[(CategoryId, CategorySpec)]) -> Option<&str> {
        classes
            .iter()
            .find(|(name, _)| name == &vec!["Work".to_string()])
            .map(|(_, spec)| spec.regex.as_str())
    }

    #[test]
    fn custom_classes_without_id_are_used() {
        let value = json!([
            {
                "name": ["Work"],
                "rule": {"type": "regex", "regex": "Slack|Notion", "ignore_case": true}
            },
            {
                "name": ["Uncategorized", "Games"],
                "rule": {"type": "regex", "regex": "Steam"}
            }
        ]);
        let classes = classes_from_settings_json(&value);
        assert_eq!(classes.len(), 2);
        assert_eq!(work_regex(&classes), Some("Slack|Notion"));
        assert!(classes[0].1.ignore_case);
        assert_eq!(
            classes[1].0,
            vec!["Uncategorized".to_string(), "Games".to_string()]
        );
        assert_eq!(classes[1].1.regex, "Steam");
    }

    #[test]
    fn null_and_empty_fall_back_to_defaults() {
        let defaults = default_classes();
        assert_eq!(
            work_regex(&classes_from_settings_json(&json!(null))),
            work_regex(&defaults)
        );
        assert_eq!(
            work_regex(&classes_from_settings_json(&json!([]))),
            work_regex(&defaults)
        );
        assert_eq!(
            work_regex(&classes_from_settings_str("null")),
            work_regex(&defaults)
        );
    }

    #[test]
    fn invalid_json_falls_back_to_defaults() {
        let defaults = default_classes();
        assert_eq!(
            work_regex(&classes_from_settings_str("not-json")),
            work_regex(&defaults)
        );
        assert_eq!(
            work_regex(&classes_from_settings_json(&json!({"name": "Work"}))),
            work_regex(&defaults)
        );
    }

    #[test]
    fn double_encoded_string_is_decoded() {
        // ActivityWatch/activitywatch#1067: classes stored as a JSON string of JSON.
        let inner = json!([
            {"name": ["Work"], "rule": {"type": "regex", "regex": "CustomApp"}}
        ]);
        let wrapped = serde_json::Value::String(inner.to_string());
        let classes = classes_from_settings_json(&wrapped);
        assert_eq!(work_regex(&classes), Some("CustomApp"));
    }

    #[test]
    fn webui_shaped_payload_with_id_and_data_parses() {
        let value = json!([
            {
                "id": 0,
                "name": ["Media", "Video"],
                "rule": {"type": "regex", "regex": "YouTube|Plex", "ignore_case": false},
                "data": {"color": "#F33", "score": 0}
            }
        ]);
        let classes = classes_from_settings_json(&value);
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].0, vec!["Media".to_string(), "Video".to_string()]);
        assert_eq!(classes[0].1.regex, "YouTube|Plex");
    }
}
