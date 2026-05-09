use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aw_models::Event;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    Drop,
    Redact,
}

/// A single privacy filter rule.
///
/// Rules are matched against incoming heartbeat events by bucket prefix and field regex.
/// Matched events are either dropped (not stored) or redacted (field value replaced).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrivacyFilterRule {
    /// If set, only apply this rule to buckets whose ID starts with this string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_prefix: Option<String>,
    /// The event data field to match against (e.g. "title", "app", "url").
    pub field: String,
    /// Regex pattern to match against the field value.
    pub pattern: String,
    /// What to do when the pattern matches: drop the event or redact the field.
    pub action: FilterAction,
    /// Replacement string used when action is `redact`. Defaults to "[redacted]".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// Apply privacy filter rules to a heartbeat event.
///
/// Returns `None` if the event should be dropped (not stored), or `Some(event)` with
/// any redacted fields applied.
///
/// Rules are applied in order; the first matching drop rule stops processing.
/// Multiple redact rules can apply to the same event.
///
/// Invalid regex patterns are logged as warnings and skipped (graceful degradation).
pub fn apply_privacy_filter(
    bucket_id: &str,
    mut event: Event,
    rules: &[PrivacyFilterRule],
) -> Option<Event> {
    for rule in rules {
        if let Some(prefix) = &rule.bucket_prefix {
            if !bucket_id.starts_with(prefix.as_str()) {
                continue;
            }
        }

        let regex = match Regex::new(&rule.pattern) {
            Ok(r) => r,
            Err(e) => {
                warn!("Privacy filter: invalid regex '{}': {}", rule.pattern, e);
                continue;
            }
        };

        let field_str = match event.data.get(&rule.field) {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };

        let matches = match regex.is_match(&field_str) {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    "Privacy filter: regex match error for '{}': {}",
                    rule.pattern, e
                );
                continue;
            }
        };

        if matches {
            match rule.action {
                FilterAction::Drop => return None,
                FilterAction::Redact => {
                    let replacement = rule
                        .replacement
                        .as_deref()
                        .unwrap_or("[redacted]")
                        .to_string();
                    event
                        .data
                        .insert(rule.field.clone(), Value::String(replacement));
                }
            }
        }
    }
    Some(event)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration};
    use serde_json::json;
    use std::str::FromStr;

    use aw_models::Event;

    use super::{apply_privacy_filter, FilterAction, PrivacyFilterRule};

    fn make_event(data: serde_json::Map<String, serde_json::Value>) -> Event {
        Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data,
        }
    }

    fn drop_rule(field: &str, pattern: &str) -> PrivacyFilterRule {
        PrivacyFilterRule {
            bucket_prefix: None,
            field: field.to_string(),
            pattern: pattern.to_string(),
            action: FilterAction::Drop,
            replacement: None,
        }
    }

    fn redact_rule(field: &str, pattern: &str, replacement: &str) -> PrivacyFilterRule {
        PrivacyFilterRule {
            bucket_prefix: None,
            field: field.to_string(),
            pattern: pattern.to_string(),
            action: FilterAction::Redact,
            replacement: Some(replacement.to_string()),
        }
    }

    #[test]
    fn test_drop_matching_event() {
        let event = make_event(json_map! {"title": json!("Private Browsing - Mozilla Firefox")});
        let rules = vec![drop_rule("title", "(?i)private browsing|incognito")];
        assert!(apply_privacy_filter("aw-watcher-window_host", event, &rules).is_none());
    }

    #[test]
    fn test_keep_non_matching_event() {
        let event = make_event(json_map! {"title": json!("GitHub - Work")});
        let rules = vec![drop_rule("title", "(?i)private browsing|incognito")];
        assert!(apply_privacy_filter("aw-watcher-window_host", event, &rules).is_some());
    }

    #[test]
    fn test_redact_field() {
        let event =
            make_event(json_map! {"title": json!("Secret Project - IDE"), "app": json!("code")});
        let rules = vec![redact_rule("title", "(?i)secret", "[redacted]")];
        let result = apply_privacy_filter("aw-watcher-window_host", event, &rules).unwrap();
        assert_eq!(result.data["title"], json!("[redacted]"));
        assert_eq!(result.data["app"], json!("code"));
    }

    #[test]
    fn test_redact_default_replacement() {
        let event = make_event(json_map! {"title": json!("incognito tab")});
        let rules = vec![PrivacyFilterRule {
            bucket_prefix: None,
            field: "title".to_string(),
            pattern: "(?i)incognito".to_string(),
            action: FilterAction::Redact,
            replacement: None,
        }];
        let result = apply_privacy_filter("aw-watcher-window_host", event, &rules).unwrap();
        assert_eq!(result.data["title"], json!("[redacted]"));
    }

    #[test]
    fn test_bucket_prefix_scoping() {
        let event = make_event(json_map! {"title": json!("Private Browsing")});
        let rules = vec![PrivacyFilterRule {
            bucket_prefix: Some("aw-watcher-window".to_string()),
            field: "title".to_string(),
            pattern: "(?i)private browsing".to_string(),
            action: FilterAction::Drop,
            replacement: None,
        }];

        // matches prefix → dropped
        assert!(apply_privacy_filter("aw-watcher-window_host", event.clone(), &rules).is_none());
        // different bucket → kept
        assert!(apply_privacy_filter("aw-watcher-web_host", event, &rules).is_some());
    }

    #[test]
    fn test_invalid_regex_skipped() {
        let event = make_event(json_map! {"title": json!("anything")});
        let rules = vec![drop_rule("title", "[invalid regex (")];
        // invalid regex is skipped, event passes through
        assert!(apply_privacy_filter("aw-watcher-window_host", event, &rules).is_some());
    }

    #[test]
    fn test_non_string_field_skipped() {
        let event = make_event(json_map! {"count": json!(42)});
        let rules = vec![drop_rule("count", "42")];
        // non-string field values are skipped
        assert!(apply_privacy_filter("aw-watcher-window_host", event, &rules).is_some());
    }

    #[test]
    fn test_multiple_redact_rules_applied() {
        let event = make_event(json_map! {
            "title": json!("Secret Meeting Notes"),
            "app": json!("PrivateApp")
        });
        let rules = vec![
            redact_rule("title", "(?i)secret", "[title redacted]"),
            redact_rule("app", "(?i)private", "[app redacted]"),
        ];
        let result = apply_privacy_filter("aw-watcher-window_host", event, &rules).unwrap();
        assert_eq!(result.data["title"], json!("[title redacted]"));
        assert_eq!(result.data["app"], json!("[app redacted]"));
    }

    #[test]
    fn test_empty_rules() {
        let event = make_event(json_map! {"title": json!("anything")});
        assert!(apply_privacy_filter("aw-watcher-window_host", event, &[]).is_some());
    }
}
