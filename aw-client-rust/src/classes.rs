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
