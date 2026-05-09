//! Server-side privacy filter for heartbeat / event ingestion.
//!
//! See ActivityWatch/aw-server-rust#482. This is the double-filter that
//! runs at ingestion regardless of whether the watcher pre-filtered.
//! Privacy-aware watchers should still pre-filter before sending; this
//! module is the consistency guarantee for older or less-aware clients.
//!
//! Rules are loaded from server config and applied to events whose
//! bucket id starts with `bucket_prefix`. Each rule has:
//!   - `field`: dotted path inside `event.data` (e.g. `title`)
//!   - `pattern`: a regex applied to the value as a string
//!   - `action`: `drop` (discard event) or `redact` (replace value)
//!   - `replacement`: replacement string for redact (default `REDACTED`)

use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aw_models::Event;

/// Action to take on a matching event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    /// Discard the event entirely (return `None` from apply).
    Drop,
    /// Replace the matched field with `replacement` (default `REDACTED`).
    Redact,
}

fn default_enabled() -> bool {
    true
}

fn default_replacement() -> String {
    "REDACTED".to_string()
}

/// A single privacy filter rule, as serialized in the server config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyFilter {
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Bucket id prefix the rule applies to (e.g. `aw-watcher-window`).
    /// Empty string matches every bucket.
    #[serde(default)]
    pub bucket_prefix: String,

    /// Dotted path into `event.data`. Only string values are inspected.
    pub field: String,

    /// Regex pattern. Invalid patterns disable the rule with a loud log
    /// at compile time (see `compile`); they are never silently escaped.
    pub pattern: String,

    pub action: FilterAction,

    #[serde(default = "default_replacement")]
    pub replacement: String,
}

/// Compiled rule with its regex pre-built once.
pub struct CompiledRule {
    pub bucket_prefix: String,
    pub field: String,
    pub action: FilterAction,
    pub replacement: String,
    pub regex: Regex,
}

/// Compile a list of `PrivacyFilter` rules. Disabled or invalid-regex
/// rules are skipped with a `warn!` log (loud failure, no silent escape).
pub fn compile(filters: &[PrivacyFilter]) -> Vec<CompiledRule> {
    let mut out = Vec::new();
    for f in filters {
        if !f.enabled {
            continue;
        }
        match Regex::new(&f.pattern) {
            Ok(regex) => out.push(CompiledRule {
                bucket_prefix: f.bucket_prefix.clone(),
                field: f.field.clone(),
                action: f.action,
                replacement: f.replacement.clone(),
                regex,
            }),
            Err(err) => warn!(
                "Disabling privacy filter rule for bucket_prefix={:?} field={:?}: invalid regex: {}",
                f.bucket_prefix, f.field, err
            ),
        }
    }
    out
}

/// Look up a dotted field path in `event.data`. Returns the string value
/// if present and string-typed; `None` otherwise (so non-string fields
/// like numbers are not coerced into matchable strings).
fn get_field_str<'a>(event: &'a Event, field: &str) -> Option<&'a str> {
    let mut cur: &Value = event.data.get(field.split('.').next()?)?;
    for part in field.split('.').skip(1) {
        cur = cur.get(part)?;
    }
    cur.as_str()
}

/// Set a dotted field path inside `event.data` to a new string value.
/// Creates intermediate maps if necessary.
fn set_field_str(event: &mut Event, field: &str, value: &str) {
    let parts: Vec<&str> = field.split('.').collect();
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        event
            .data
            .insert(parts[0].to_string(), Value::String(value.to_string()));
        return;
    }
    // The aw-models Event uses serde_json::Map for `data`. Walk into it
    // for nested fields, ignoring non-object intermediates.
    let first = parts[0];
    if !event.data.contains_key(first) {
        return; // don't auto-create paths we couldn't read from
    }
    let entry = event.data.get_mut(first).unwrap();
    let mut cur = entry;
    for part in &parts[1..parts.len() - 1] {
        match cur.get_mut(*part) {
            Some(next) => cur = next,
            None => return,
        }
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert(
            parts[parts.len() - 1].to_string(),
            Value::String(value.to_string()),
        );
    }
}

/// Apply the compiled rules to an event for the given bucket id.
///
/// Returns `None` if any matching rule says `drop`. Returns `Some(event)`
/// with redacted fields applied otherwise (including the no-rules case).
pub fn apply(rules: &[CompiledRule], bucket_id: &str, mut event: Event) -> Option<Event> {
    for rule in rules {
        if !rule.bucket_prefix.is_empty() && !bucket_id.starts_with(&rule.bucket_prefix) {
            continue;
        }
        let Some(value) = get_field_str(&event, &rule.field) else {
            continue;
        };
        let matched = match rule.regex.is_match(value) {
            Ok(m) => m,
            Err(err) => {
                warn!("privacy_filter regex error on {:?}: {}", rule.field, err);
                false
            }
        };
        if !matched {
            continue;
        }
        match rule.action {
            FilterAction::Drop => return None,
            FilterAction::Redact => {
                set_field_str(&mut event, &rule.field, &rule.replacement);
            }
        }
    }
    Some(event)
}

/// Apply rules to a batch of events, removing those that should be dropped.
pub fn apply_batch(rules: &[CompiledRule], bucket_id: &str, events: Vec<Event>) -> Vec<Event> {
    if rules.is_empty() {
        return events;
    }
    events
        .into_iter()
        .filter_map(|e| apply(rules, bucket_id, e))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::{json, Map};

    fn evt(data: serde_json::Value) -> Event {
        let map = data.as_object().unwrap().clone();
        Event {
            id: None,
            timestamp: Utc::now(),
            duration: chrono::Duration::seconds(0),
            data: map,
        }
    }

    fn rule(bucket_prefix: &str, field: &str, pattern: &str, action: FilterAction) -> PrivacyFilter {
        PrivacyFilter {
            enabled: true,
            bucket_prefix: bucket_prefix.to_string(),
            field: field.to_string(),
            pattern: pattern.to_string(),
            action,
            replacement: default_replacement(),
        }
    }

    #[test]
    fn drop_matching_event() {
        let rules = compile(&[rule(
            "aw-watcher-window",
            "title",
            "(?i)private browsing|incognito",
            FilterAction::Drop,
        )]);
        let dropped = apply(
            &rules,
            "aw-watcher-window_test",
            evt(json!({"app": "firefox", "title": "Banking - Private Browsing"})),
        );
        assert!(dropped.is_none());

        let kept = apply(
            &rules,
            "aw-watcher-window_test",
            evt(json!({"app": "firefox", "title": "GitHub"})),
        );
        assert!(kept.is_some());
    }

    #[test]
    fn redact_matching_field() {
        let rules = compile(&[rule(
            "aw-watcher-window",
            "title",
            "(?i)password",
            FilterAction::Redact,
        )]);
        let result = apply(
            &rules,
            "aw-watcher-window_x11",
            evt(json!({"app": "1Password", "title": "1Password - Master password"})),
        )
        .expect("event should be kept after redact");
        assert_eq!(result.data.get("title").unwrap(), "REDACTED");
        assert_eq!(result.data.get("app").unwrap(), "1Password");
    }

    #[test]
    fn bucket_prefix_scoping() {
        let rules = compile(&[rule(
            "aw-watcher-window",
            "title",
            ".*",
            FilterAction::Drop,
        )]);
        // Different bucket prefix, rule should not apply.
        let kept = apply(
            &rules,
            "aw-watcher-afk_test",
            evt(json!({"status": "afk"})),
        );
        assert!(kept.is_some());
    }

    #[test]
    fn invalid_regex_disables_rule() {
        let rules = compile(&[rule("any", "title", "(unbalanced", FilterAction::Drop)]);
        assert!(rules.is_empty(), "invalid regex must not produce a rule");
    }

    #[test]
    fn disabled_rule_is_skipped() {
        let mut r = rule("any", "title", ".*", FilterAction::Drop);
        r.enabled = false;
        let rules = compile(&[r]);
        assert!(rules.is_empty());
    }

    #[test]
    fn empty_prefix_matches_any_bucket() {
        let rules = compile(&[rule("", "title", "secret", FilterAction::Drop)]);
        let result = apply(
            &rules,
            "aw-watcher-anything",
            evt(json!({"title": "a secret"})),
        );
        assert!(result.is_none());
    }

    #[test]
    fn non_string_field_is_skipped() {
        let rules = compile(&[rule("", "count", "42", FilterAction::Drop)]);
        // Numeric value, regex would match the digits but field-as-string lookup yields None.
        let kept = apply(&rules, "any", evt(json!({"count": 42})));
        assert!(kept.is_some());
    }

    #[test]
    fn dotted_field_path() {
        let mut event_data = Map::new();
        event_data.insert("url".to_string(), json!({"host": "bank.example.com"}));
        let event = Event {
            id: None,
            timestamp: Utc::now(),
            duration: chrono::Duration::seconds(0),
            data: event_data,
        };
        let rules = compile(&[rule("", "url.host", "(?i)bank", FilterAction::Drop)]);
        assert!(apply(&rules, "any", event).is_none());
    }

    #[test]
    fn redact_uses_custom_replacement() {
        let mut r = rule("", "title", ".*", FilterAction::Redact);
        r.replacement = "***".to_string();
        let rules = compile(&[r]);
        let result = apply(&rules, "any", evt(json!({"title": "anything"}))).unwrap();
        assert_eq!(result.data.get("title").unwrap(), "***");
    }

    #[test]
    fn apply_batch_drops_and_keeps() {
        let rules = compile(&[rule("", "title", "drop_me", FilterAction::Drop)]);
        let events = vec![
            evt(json!({"title": "drop_me first"})),
            evt(json!({"title": "keep this"})),
            evt(json!({"title": "drop_me again"})),
        ];
        let out = apply_batch(&rules, "any", events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data.get("title").unwrap(), "keep this");
    }
}
