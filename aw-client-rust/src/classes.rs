//! Default classes
//!
//! Taken from default classes in aw-webui

use log::warn;
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::blocking::AwClient as ActivityWatchClient;

pub type CategoryId = Vec<String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySpec {
    #[serde(rename = "type")]
    pub spec_type: String,
    pub regex: String,
    #[serde(default)]
    pub ignore_case: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassSetting {
    pub name: Vec<String>,
    pub rule: CategorySpec,
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

/// Get classes from server-side settings using default localhost:5600.
/// Might throw an error if not set yet, in which case we use the default classes as a fallback.
pub fn get_classes() -> Vec<(CategoryId, CategorySpec)> {
    get_classes_from_server("localhost", 5600)
}

/// Get classes from server-side settings with custom host and port.
/// Might throw an error if not set yet, in which case we use the default classes as a fallback.
pub fn get_classes_from_server(host: &str, port: u16) -> Vec<(CategoryId, CategorySpec)> {
    let mut rng = rand::rng();
    let random_int = rng.random_range(0..10001);
    let client_id = format!("get-setting-{}", random_int);

    // Create a client with a random ID, similar to the Python implementation
    let awc = match ActivityWatchClient::new(host, port, &client_id) {
        Ok(client) => client,
        Err(_) => {
            warn!(
                "Failed to create ActivityWatch client for {}:{}, using default classes",
                host, port
            );
            return default_classes();
        }
    };

    awc.get_setting("classes")
        .map(|setting_value| {
            // Try to deserialize the setting into Vec<ClassSetting>
            if setting_value.is_null() {
                return default_classes();
            }

            let class_settings: Vec<ClassSetting> = serde_json::from_value(setting_value)
                .unwrap_or_else(|_| {
                    warn!("Failed to deserialize classes setting, using default classes");
                    return vec![];
                });

            // Convert ClassSetting to (CategoryId, CategorySpec) format
            class_settings
                .into_iter()
                .map(|class| (class.name, class.rule))
                .collect()
        })
        .unwrap_or_else(|_| {
            warn!(
                "Failed to get classes from server {}:{}, using default classes as fallback",
                host, port
            );
            default_classes()
        })
}
