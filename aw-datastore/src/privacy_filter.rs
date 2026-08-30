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
    /// Replacement text for the redact action.
    ///
    /// Capture substitution (`$1`, `$2`, `$name`) is opt-in: it runs only
    /// when `pattern` has capturing groups *and* this string contains a
    /// capture template whose referenced groups participate in the match.
    /// `$0` and unmatched alternation groups stay whole-field. Static
    /// replacements always replace the entire field — so stored rules like
    /// `(token)` + `REDACTED` do not leak unmatched text.
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

    /// Redact `source` using this rule's pattern and `replacement`.
    ///
    /// `$1`/`$name` substitution (awatcher-compatible) runs only when the
    /// pattern has capturing groups *and* `replacement` is a capture
    /// template whose every referenced group participates in every match.
    /// `$0` and unmatched alternation/optional groups stay whole-field —
    /// otherwise `replace_all` would leak unmatched sensitive text.
    fn redact_value(&self, source: &str, replacement: &str) -> String {
        let re = self
            .regex_cache
            .get_or_init(|| regex::Regex::new(&self.pattern).ok());
        match re.as_ref() {
            Some(re) if re.captures_len() > 1 => match capture_refs_in_template(replacement, re) {
                Some(refs) if referenced_captures_present(re, source, &refs) => {
                    re.replace_all(source, replacement).into_owned()
                }
                _ => replacement.to_owned(),
            },
            _ => replacement.to_owned(),
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
                        let current = resolve_field(&event.data, field_path)
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        let new_value = match current.as_deref() {
                            Some(source) => self.redact_value(source, replacement),
                            None => replacement.clone(),
                        };
                        set_field(&mut event.data, field_path, Value::String(new_value));
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

/// A `$` reference in a replacement template: `$1` / `${1}` or `$name` / `${name}`.
enum CaptureRef {
    Index(usize),
    Name(String),
}

/// Parse `replacement` as a capture template whose every `$` reference names
/// a group that exists on `re`. `$$` is an escaped dollar.
///
/// Returns `None` (stay whole-field) for dangling refs, empty `${}`, `$0`
/// (whole-match identity), and Perl-only `$&`/`$``/`$'`. Those must not opt
/// into `replace_all`: the regex crate expands unknown / empty-named groups
/// to `""`, and `$0` substitutes the match unchanged — both leak unmatched
/// field text.
fn capture_refs_in_template(replacement: &str, re: &regex::Regex) -> Option<Vec<CaptureRef>> {
    let n_groups = re.captures_len();
    let named: Vec<&str> = re.capture_names().flatten().collect();
    let bytes = replacement.as_bytes();
    let mut i = 0;
    let mut refs = Vec::new();
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'$') {
            i += 2;
            continue;
        }
        let rest = &replacement[i + 1..];
        if rest.is_empty() {
            break;
        }
        let first = rest.as_bytes()[0];
        if first == b'{' {
            match rest[1..].find('}') {
                Some(end) => {
                    let name = &rest[1..1 + end];
                    refs.push(resolve_capture_ref(name, n_groups, &named)?);
                    i += 2 + end + 1; // ${ name }
                }
                None => return None,
            }
            continue;
        }
        // Longest ident, matching the regex crate: `$1a` is name `1a`, not `$1` + `a`.
        // `$&`/`$``/`$'` are Perl-only and are *not* interpolated by `regex`.
        if first.is_ascii_alphanumeric() || first == b'_' {
            let rb = rest.as_bytes();
            let mut j = 1;
            while j < rb.len() && (rb[j].is_ascii_alphanumeric() || rb[j] == b'_') {
                j += 1;
            }
            let name = &rest[..j];
            refs.push(resolve_capture_ref(name, n_groups, &named)?);
            i += 1 + j;
            continue;
        }
        i += 1;
    }
    if refs.is_empty() {
        None
    } else {
        Some(refs)
    }
}

fn resolve_capture_ref(name: &str, n_groups: usize, named: &[&str]) -> Option<CaptureRef> {
    // `${}` is a named ref with an empty name, *not* `$0`. The regex crate
    // expands it to "" (no such group), which would leak unmatched text.
    if name.is_empty() {
        return None;
    }
    if name.bytes().all(|b| b.is_ascii_digit()) {
        let n = name.parse::<usize>().ok()?;
        // Group 0 is the whole match. `$0` / `${0}` would take replace_all
        // and persist the match plus unmatched field text unchanged.
        if n > 0 && n < n_groups {
            return Some(CaptureRef::Index(n));
        }
        return None;
    }
    named
        .contains(&name)
        .then(|| CaptureRef::Name(name.to_owned()))
}

/// True when every referenced group participates in every match.
///
/// Alternation / optional groups exist on the regex but may be `None` for a
/// particular match (`$2` with `(token)|(secret)` matching `token=abc`).
/// `replace_all` expands those to `""` and leaks unmatched field text.
fn referenced_captures_present(re: &regex::Regex, source: &str, refs: &[CaptureRef]) -> bool {
    let mut any = false;
    for caps in re.captures_iter(source) {
        any = true;
        for r in refs {
            let present = match r {
                CaptureRef::Index(n) => caps.get(*n).is_some(),
                CaptureRef::Name(name) => caps.name(name).is_some(),
            };
            if !present {
                return false;
            }
        }
    }
    any
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

    fn redact_rule(pattern: &str, replacement: &str) -> PrivacyFilterRule {
        PrivacyFilterRule {
            enabled: true,
            bucket_prefix: None,
            field: Some("title".to_string()),
            pattern: pattern.to_string(),
            action: PrivacyFilterAction::Redact,
            replacement: Some(replacement.to_string()),
            regex_cache: OnceLock::new(),
        }
    }

    #[test]
    fn test_redact_without_captures_replaces_whole_field() {
        let rule = redact_rule(r"(?i).*banking.*", "REDACTED");
        let mut event = test_event("Online Banking - My Account Balance");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "REDACTED"
        );
    }

    #[test]
    fn test_redact_with_capture_groups_like_awatcher() {
        // awatcher: match `org\.kde\.(.*)`, replace `$1` → `dolphin`
        let rule = redact_rule(r"(.*) - Mozilla Firefox", "$1");
        let mut event = test_event("GitHub - Mozilla Firefox");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "GitHub"
        );
    }

    #[test]
    fn test_redact_static_replacement_with_captures_stays_whole_field() {
        // Existing stored rules may use capturing groups with a static
        // replacement. Partial replace_all would leak unmatched text
        // (`token=abc token=def` → `REDACTED=abc REDACTED=def`).
        let rule = redact_rule(r"(token)", "REDACTED");
        let mut event = test_event("token=abc token=def");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "REDACTED"
        );
    }

    #[test]
    fn test_redact_strips_url_path_keep_host() {
        let rule = redact_rule(r"https://([^/]+)/.*", "https://$1/");
        let mut event = test_event("https://bank.example/account?token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "https://bank.example/"
        );
    }

    #[test]
    fn test_redact_named_capture_like_awatcher() {
        let rule = redact_rule(r"https://(?P<host>[^/]+)/.*", "https://$host/");
        let mut event = test_event("https://bank.example/account?token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "https://bank.example/"
        );
    }

    #[test]
    fn test_redact_awatcher_vscode_dirty_indicator() {
        // awatcher README: match-title = "● (.*)", replace-title = "$1"
        let rule = redact_rule(r"● (.*)", "$1");
        let mut event = test_event("● file_config.rs - awatcher - Visual Studio Code");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "file_config.rs - awatcher - Visual Studio Code"
        );
    }

    #[test]
    fn test_redact_replace_all_occurrences_with_template() {
        let rule = redact_rule(r"(token)=\S+", "$1=REDACTED");
        let mut event = test_event("token=abc token=def");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "token=REDACTED token=REDACTED"
        );
    }

    #[test]
    fn test_redact_dangling_capture_ref_stays_whole_field() {
        // `$5` is not group 1; replace_all would expand it to "" and leak `=abc`.
        let rule = redact_rule(r"(token)", "REDACTED $5");
        let mut event = test_event("token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "REDACTED $5"
        );
    }

    #[test]
    fn test_redact_empty_braced_ref_stays_whole_field() {
        // `${}` names no group; replace_all would expand it to "" and leak `=abc`.
        let rule = redact_rule(r"(token)", "${}");
        let mut event = test_event("token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(result.data.get("title").unwrap().as_str().unwrap(), "${}");
    }

    #[test]
    fn test_replacement_is_capture_template() {
        let one = regex::Regex::new(r"(token)").unwrap();
        let named = regex::Regex::new(r"(?P<host>[^/]+)").unwrap();
        let two = regex::Regex::new(r"(a)(b)").unwrap();

        assert!(capture_refs_in_template("$1", &one).is_some());
        assert!(capture_refs_in_template("https://$1/", &one).is_some());
        assert!(capture_refs_in_template("${1}", &one).is_some());
        assert!(capture_refs_in_template("$0", &one).is_none());
        assert!(capture_refs_in_template("https://$host/", &named).is_some());
        assert!(capture_refs_in_template("$1$2", &two).is_some());
        assert!(capture_refs_in_template("REDACTED", &one).is_none());
        assert!(capture_refs_in_template("token=REDACTED", &one).is_none());
        assert!(capture_refs_in_template("$$", &one).is_none());
        assert!(capture_refs_in_template("cost $$5", &one).is_none());
        assert!(capture_refs_in_template("REDACTED $5", &one).is_none());
        assert!(capture_refs_in_template("$2", &one).is_none());
        assert!(capture_refs_in_template("$host", &one).is_none());
        assert!(capture_refs_in_template("$1$2", &one).is_none());
        assert!(capture_refs_in_template("REDACTED $'", &one).is_none());
        assert!(capture_refs_in_template("REDACTED $&", &one).is_none());
        assert!(capture_refs_in_template("REDACTED $`", &one).is_none());
        assert!(capture_refs_in_template("${}", &one).is_none());
        assert!(capture_refs_in_template("https://${}/", &one).is_none());
        assert!(capture_refs_in_template("${0}", &one).is_none());
        assert!(capture_refs_in_template("$1$0", &one).is_none());
    }

    #[test]
    fn test_redact_group_zero_stays_whole_field() {
        // `$0` is the whole match. replace_all would substitute it unchanged
        // and leave unmatched sensitive text (`token=abc` stays `token=abc`).
        let rule = redact_rule(r"(token)", "$0");
        let mut event = test_event("token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(result.data.get("title").unwrap().as_str().unwrap(), "$0");
    }

    #[test]
    fn test_redact_braced_group_zero_stays_whole_field() {
        let rule = redact_rule(r"(token)", "${0}");
        let mut event = test_event("token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(result.data.get("title").unwrap().as_str().unwrap(), "${0}");
    }

    #[test]
    fn test_redact_unmatched_alternation_capture_stays_whole_field() {
        // `$2` exists on `(token)|(secret)` but does not participate when
        // the first alternative matches. replace_all expands it to "" and
        // leaves `=abc`. Fail closed: whole-field redaction.
        let rule = redact_rule(r"(token)|(secret)", "$2");
        let mut event = test_event("token=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(result.data.get("title").unwrap().as_str().unwrap(), "$2");
    }

    #[test]
    fn test_redact_participating_alternation_capture_still_replaces() {
        // Same pattern, but `$2` *does* participate. Match span is still
        // only `secret`; leftover `=abc` is the opt-in replace_all contract
        // (same as `$1=REDACTED` keeping the space between tokens).
        let rule = redact_rule(r"(token)|(secret)", "$2");
        let mut event = test_event("secret=abc");
        assert!(rule.matches("any-bucket", &event));
        let result = rule.apply(&mut event).unwrap();
        assert_eq!(
            result.data.get("title").unwrap().as_str().unwrap(),
            "secret=abc"
        );
    }
}
