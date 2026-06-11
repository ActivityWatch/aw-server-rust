use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::map::Map;
use serde_json::Value;

use aw_models::Event;

/// Action to take when a rule matches an event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyFilterAction {
    /// Drop the entire event from storage
    Drop,
    /// Redact a specific field's value with a replacement
    Redact,
}

/// A single privacy filter rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyFilterRule {
    /// If true, this rule is active
    pub enabled: bool,
    /// Only apply to buckets whose ID starts with this prefix (e.g. "aw-watcher-window")
    pub bucket_prefix: Option<String>,
    /// Dotted path to the event data field to check (e.g. "title")
    pub field: Option<String>,
    /// Regex pattern to match against the field value
    pub pattern: String,
    /// What to do when matched
    pub action: PrivacyFilterAction,
    /// Replacement text for the redact action
    pub replacement: Option<String>,
    /// Pre-compiled regex, populated lazily on first match. Not serialized.
    #[serde(skip)]
    regex_cache: OnceLock<Option<regex::Regex>>,
}

impl PartialEq for PrivacyFilterRule {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.bucket_prefix == other.bucket_prefix
            && self.field == other.field
            && self.pattern == other.pattern
            && self.action == other.action
            && self.replacement == other.replacement
    }
}

impl PrivacyFilterRule {
    /// Check if this rule matches a given event in a given bucket.
    pub fn matches(&self, bucket_id: &str, event: &Event) -> bool {
        if !self.enabled {
            return false;
        }

        // Check bucket prefix if specified
        if let Some(ref prefix) = self.bucket_prefix {
            if !bucket_id.starts_with(prefix) {
                return false;
            }
        }

        // Check field pattern if specified
        if let Some(ref field_path) = self.field {
            let field_value = resolve_field(&event.data, field_path);
            match field_value {
                Some(Value::String(s)) => {
                    // Compile the regex once and cache it for subsequent calls.
                    let re = self
                        .regex_cache
                        .get_or_init(|| regex::Regex::new(&self.pattern).ok());
                    re.as_ref().map(|re| re.is_match(s)).unwrap_or(false)
                }
                Some(_) | None => false,
            }
        } else {
            true
        }
    }

    /// Apply this rule's action to an event.
    /// Returns None if dropped, Some(event) if kept (possibly redacted).
    pub fn apply<'a>(&self, event: &'a mut Event) -> Option<&'a mut Event> {
        match self.action {
            PrivacyFilterAction::Drop => None,
            PrivacyFilterAction::Redact => {
                if let Some(ref replacement) = self.replacement {
                    if let Some(ref field_path) = self.field {
                        set_field(
                            &mut event.data,
                            field_path,
                            Value::String(replacement.clone()),
                        );
                    }
                }
                Some(event)
            }
        }
    }
}

/// Engine that holds and applies privacy filter rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyFilterEngine {
    rules: Vec<PrivacyFilterRule>,
}

impl PrivacyFilterEngine {
    pub fn new(rules: Vec<PrivacyFilterRule>) -> Self {
        PrivacyFilterEngine { rules }
    }

    /// Example rules for common sensitive data patterns.
    /// Not applied automatically — use `new()` with these rules to opt in.
    pub fn with_defaults() -> Self {
        let rules = vec![
            PrivacyFilterRule {
                enabled: true,
                bucket_prefix: Some("aw-watcher-window".to_string()),
                field: Some("title".to_string()),
                pattern: r"(?i)(private browsing|incognito)".to_string(),
                action: PrivacyFilterAction::Drop,
                replacement: None,
                regex_cache: OnceLock::new(),
            },
            PrivacyFilterRule {
                enabled: true,
                bucket_prefix: Some("aw-watcher-window".to_string()),
                field: Some("title".to_string()),
                pattern: r"(?i).*banking.*".to_string(),
                action: PrivacyFilterAction::Redact,
                replacement: Some("REDACTED".to_string()),
                regex_cache: OnceLock::new(),
            },
        ];
        PrivacyFilterEngine { rules }
    }

    /// Parse rules from a JSON string (as stored in settings).
    pub fn from_json(json_str: &str) -> Result<Self, String> {
        let rules: Vec<PrivacyFilterRule> = serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse privacy filter rules: {e}"))?;
        for rule in &rules {
            if rule.action == PrivacyFilterAction::Redact
                && rule.replacement.as_deref().is_none_or(str::is_empty)
            {
                return Err(format!(
                    "Redact rule with pattern {:?} is missing `replacement` — add a non-empty replacement string or use action=drop",
                    rule.pattern
                ));
            }
            if rule.action == PrivacyFilterAction::Redact && rule.field.is_none() {
                return Err(format!(
                    "Redact rule with pattern {:?} is missing `field` — specify which data field to redact (e.g. \"title\")",
                    rule.pattern
                ));
            }
            if rule.action == PrivacyFilterAction::Drop && rule.field.is_none() {
                return Err(format!(
                    "Drop rule with pattern {:?} is missing `field` — without a field path the pattern is never evaluated and every event in the matching bucket is dropped (specify a dotted field path, e.g. \"title\")",
                    rule.pattern
                ));
            }
            if let Err(e) = regex::Regex::new(&rule.pattern) {
                return Err(format!(
                    "Rule with pattern {:?} has an invalid regex: {e}",
                    rule.pattern
                ));
            }
        }
        Ok(PrivacyFilterEngine { rules })
    }

    /// Serialize rules to JSON string.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.rules)
            .map_err(|e| format!("Failed to serialize privacy filter rules: {e}"))
    }

    /// Filter a single event for a given bucket.
    /// Applies all matching rules. Returns None if dropped, Some (possibly redacted) event if kept.
    pub fn filter_event(&self, bucket_id: &str, event: Event) -> Option<Event> {
        let mut event = event;
        for rule in &self.rules {
            if rule.matches(bucket_id, &event) {
                match rule.apply(&mut event) {
                    Some(_) => {}        // Redacted — continue applying other rules
                    None => return None, // Dropped
                }
            }
        }
        Some(event)
    }

    /// Filter a batch of events for a given bucket.
    pub fn filter_events(&self, bucket_id: &str, events: Vec<Event>) -> Vec<Event> {
        events
            .into_iter()
            .filter_map(|e| self.filter_event(bucket_id, e))
            .collect()
    }
}

/// Resolve a dotted field path (e.g. "title", "data.url") from a serde_json Map.
fn resolve_field<'a>(data: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current: &Map<String, Value> = data;
    for (i, part) in parts.iter().enumerate() {
        let val = current.get(*part)?;
        if i == parts.len() - 1 {
            return Some(val);
        }
        match val {
            Value::Object(map) => current = map,
            _ => return None,
        }
    }
    None
}

/// Set a field value at a dotted path in a serde_json Map.
fn set_field(data: &mut Map<String, Value>, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current: &mut Map<String, Value> = data;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            current.insert(part.to_string(), value);
            return;
        }
        current = match current
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
        {
            Some(m) => m,
            None => return, // intermediate segment is not an object — skip silently
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn test_event(title: &str) -> Event {
        Event {
            id: None,
            timestamp: Utc::now(),
            duration: chrono::Duration::seconds(1),
            data: json_map! {"title": json!(title), "app": json!("Firefox")},
        }
    }

    #[test]
    fn test_drop_incognito_window() {
        let engine = PrivacyFilterEngine::with_defaults();
        let event = test_event("Private Browsing - Mozilla Firefox");
        let rule = &engine.rules[0];
        assert!(rule.matches("aw-watcher-window", &event));
    }

    #[test]
    fn test_allow_normal_window() {
        let engine = PrivacyFilterEngine::with_defaults();
        let event = test_event("GitHub - Mozilla Firefox");
        let rule = &engine.rules[0];
        assert!(!rule.matches("aw-watcher-window", &event));
    }

    #[test]
    fn test_redact_banking() {
        let engine = PrivacyFilterEngine::with_defaults();
        let mut event = test_event("Online Banking - My Account Balance");
        let rule = &engine.rules[1];
        assert!(rule.matches("aw-watcher-window", &event));
        let result = rule.apply(&mut event);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().data.get("title").unwrap().as_str().unwrap(),
            "REDACTED"
        );
    }

    #[test]
    fn test_bucket_scoping() {
        let rule = PrivacyFilterRule {
            enabled: true,
            bucket_prefix: Some("aw-watcher-window".to_string()),
            field: Some("title".to_string()),
            pattern: r"(?i)(private browsing|incognito)".to_string(),
            action: PrivacyFilterAction::Drop,
            replacement: None,
            regex_cache: OnceLock::new(),
        };
        let event = test_event("Private Browsing - Mozilla Firefox");
        assert!(rule.matches("aw-watcher-window", &event));
        assert!(!rule.matches("aw-watcher-afk", &event));
    }

    #[test]
    fn test_disabled_rule() {
        let rule = PrivacyFilterRule {
            enabled: false,
            bucket_prefix: Some("aw-watcher-window".to_string()),
            field: Some("title".to_string()),
            pattern: ".*".to_string(),
            action: PrivacyFilterAction::Drop,
            replacement: None,
            regex_cache: OnceLock::new(),
        };
        let event = test_event("Anything at all");
        assert!(!rule.matches("aw-watcher-window", &event));
    }

    #[test]
    fn test_invalid_regex_no_panic() {
        let rule = PrivacyFilterRule {
            enabled: true,
            bucket_prefix: Some("aw-watcher-window".to_string()),
            field: Some("title".to_string()),
            pattern: r"[invalid".to_string(),
            action: PrivacyFilterAction::Drop,
            replacement: None,
            regex_cache: OnceLock::new(),
        };
        let event = test_event("test");
        assert!(!rule.matches("aw-watcher-window", &event));
    }

    #[test]
    fn test_set_field_no_panic_on_non_object_intermediate() {
        let mut data = serde_json::Map::new();
        data.insert(
            "title".to_string(),
            Value::String("flat string".to_string()),
        );
        // "title" is a string, not an object — setting "title.nested" should not panic
        set_field(
            &mut data,
            "title.nested",
            Value::String("value".to_string()),
        );
        // title should remain unchanged
        assert_eq!(data.get("title").unwrap().as_str().unwrap(), "flat string");
    }

    #[test]
    fn test_from_json_redact_without_replacement_is_error() {
        let json = r#"[{"enabled":true,"pattern":"(?i)secret","action":"redact","field":"title"}]"#;
        let result = PrivacyFilterEngine::from_json(json);
        assert!(
            result.is_err(),
            "Redact rule without replacement must fail from_json"
        );
        assert!(result.unwrap_err().contains("replacement"));
    }

    #[test]
    fn test_from_json_redact_with_empty_replacement_is_error() {
        let json = r#"[{"enabled":true,"pattern":"(?i)secret","action":"redact","field":"title","replacement":""}]"#;
        let result = PrivacyFilterEngine::from_json(json);
        assert!(
            result.is_err(),
            "Redact rule with empty replacement must fail from_json"
        );
        assert!(result.unwrap_err().contains("replacement"));
    }

    #[test]
    fn test_from_json_redact_without_field_is_error() {
        let json = r#"[{"enabled":true,"pattern":"(?i)secret","action":"redact","replacement":"REDACTED"}]"#;
        let result = PrivacyFilterEngine::from_json(json);
        assert!(
            result.is_err(),
            "Redact rule without field must fail from_json"
        );
        assert!(result.unwrap_err().contains("field"));
    }

    #[test]
    fn test_from_json_invalid_regex_is_error() {
        let json = r#"[{"enabled":true,"pattern":"[unclosed","action":"drop","field":"title"}]"#;
        let result = PrivacyFilterEngine::from_json(json);
        assert!(
            result.is_err(),
            "Rule with invalid regex must fail from_json"
        );
    }

    #[test]
    fn test_from_json_drop_without_field_is_error() {
        let json = r#"[{"enabled":true,"pattern":"(?i)incognito","action":"drop"}]"#;
        let result = PrivacyFilterEngine::from_json(json);
        assert!(
            result.is_err(),
            "Drop rule without field must fail from_json"
        );
        assert!(result.unwrap_err().contains("field"));
    }

    #[test]
    fn test_drop_action() {
        let rule = PrivacyFilterRule {
            enabled: true,
            bucket_prefix: None,
            field: Some("title".to_string()),
            pattern: ".*".to_string(),
            action: PrivacyFilterAction::Drop,
            replacement: None,
            regex_cache: OnceLock::new(),
        };
        let mut event = test_event("anything");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event);
        assert!(result.is_none(), "Drop action should return None");
    }
}
