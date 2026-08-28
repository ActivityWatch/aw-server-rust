/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py
use aw_models::Event;
use fancy_regex::Regex;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

const REGEX_CACHE_CAPACITY: usize = 512;

static REGEX_CACHE: OnceLock<Mutex<LruCache<String, Arc<Regex>>>> = OnceLock::new();

pub enum Rule {
    None,
    Regex(RegexRule),
}

impl RuleTrait for Rule {
    fn matches(&self, event: &Event) -> bool {
        match self {
            Rule::None => false,
            Rule::Regex(rule) => rule.matches(event),
        }
    }
}

trait RuleTrait {
    fn matches(&self, event: &Event) -> bool;
}

pub struct RegexRule {
    regex: Arc<Regex>,
    select_keys: Option<Vec<String>>,
}

impl RegexRule {
    pub fn new(
        regex_str: &str,
        ignore_case: bool,
        select_keys: Option<Vec<String>>,
    ) -> Result<RegexRule, fancy_regex::Error> {
        // Validate that select_keys is not an empty list, which would silently never match.
        if let Some(ref keys) = select_keys {
            if keys.is_empty() {
                return Err(fancy_regex::Error::ParseError(
                    0,
                    fancy_regex::ParseError::GeneralParseError(
                        "select_keys must not be empty".to_string(),
                    ),
                ));
            }
        }

        // can't use `RegexBuilder::case_insensitive` because it's not supported by fancy_regex,
        // so we need to prefix with `(?i)` to make it case insensitive.
        let full_regex_str = if ignore_case {
            format!("(?i){regex_str}")
        } else {
            regex_str.to_string()
        };

        let cache = REGEX_CACHE.get_or_init(|| {
            Mutex::new(LruCache::new(
                NonZeroUsize::new(REGEX_CACHE_CAPACITY).unwrap(),
            ))
        });
        let mut cache = cache.lock().unwrap();

        let regex = if let Some(re) = cache.get(&full_regex_str) {
            re.clone()
        } else {
            let re = Arc::new(Regex::new(&full_regex_str)?);
            cache.put(full_regex_str.clone(), re.clone());
            re
        };

        Ok(RegexRule { regex, select_keys })
    }

    fn value_matches(&self, value: &serde_json::Value) -> bool {
        match value.as_str() {
            Some(value) => self.regex.is_match(value).unwrap_or(false),
            None => false,
        }
    }
}

/// This struct defines the rules for classification.
/// For now it just needs to contain the regex to match with, but in the future it might contain a
/// glob-pattern, or other options for classifying.
/// It's puropse is to make the API easy to extend in the future without having to break backwards
/// compatibility (or have to maintain "old" query2 functions).
impl RuleTrait for RegexRule {
    fn matches(&self, event: &Event) -> bool {
        match &self.select_keys {
            Some(select_keys) => select_keys
                .iter()
                .filter_map(|key| event.data.get(key))
                .any(|val| self.value_matches(val)),
            None => event.data.values().any(|val| self.value_matches(val)),
        }
    }
}

impl From<Regex> for Rule {
    fn from(re: Regex) -> Self {
        Rule::Regex(RegexRule {
            regex: Arc::new(re),
            select_keys: None,
        })
    }
}

/// A category matching rule passed to [`categorize`].
///
/// `priority` is an optional ranking score. When set, it is used instead of
/// category depth to pick among matching rules (higher wins). When `None`,
/// ranking falls back to depth so existing configs keep their current behavior.
pub struct CategoryRule {
    pub category: Vec<String>,
    pub rule: Rule,
    pub priority: Option<i64>,
}

impl CategoryRule {
    pub fn new(category: Vec<String>, rule: Rule) -> Self {
        Self {
            category,
            rule,
            priority: None,
        }
    }

    pub fn with_priority(mut self, priority: i64) -> Self {
        self.priority = Some(priority);
        self
    }
}

impl From<(Vec<String>, Rule)> for CategoryRule {
    fn from((category, rule): (Vec<String>, Rule)) -> Self {
        Self::new(category, rule)
    }
}

/// Categorizes a list of events
///
/// An event can only have one category, although the category may have a hierarchy,
/// for instance: "Work -> ActivityWatch -> aw-server-rust"
/// If multiple categories match, the highest-ranking one is chosen.
/// Ranking is the optional `priority` on the rule when present, otherwise category
/// depth ("the deepest one will be chosen"). Equal ranks keep the later match,
/// matching the previous depth-only `>=` comparison.
///
/// Performance: builds an in-memory cache keyed on the event's data JSON so that
/// events with identical data (same app/title — very common in practice) are only
/// matched against the rule set once. On a month's data with 50k+ events but only
/// a few hundred distinct app/title pairs this reduces regex work by >99%.
pub fn categorize(mut events: Vec<Event>, rules: &[CategoryRule]) -> Vec<Event> {
    // Cache: serialized event data → assigned category
    let mut category_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut classified_events = Vec::with_capacity(events.len());
    for mut event in events.drain(..) {
        // Key on the full event data. serde_json::Map preserves insertion order, so
        // events with the same fields in the same order produce the same key — which
        // is the normal case for heartbeat-based watchers.
        let cache_key = serde_json::to_string(&event.data).unwrap_or_default();
        let category = category_cache
            .entry(cache_key)
            .or_insert_with(|| _pick_category(&event, rules))
            .clone();
        event
            .data
            .insert("$category".into(), serde_json::json!(category));
        classified_events.push(event);
    }
    classified_events
}

fn _pick_category(event: &Event, rules: &[CategoryRule]) -> Vec<String> {
    let mut category: Vec<String> = vec!["Uncategorized".into()];
    // Uncategorized loses to any actual match, including a match with a very
    // low explicit priority. i64::MIN is only used as this sentinel.
    let mut rank = i64::MIN;
    for class in rules {
        if class.rule.matches(event) {
            let item_rank = _effective_rank(&class.category, class.priority);
            if item_rank >= rank {
                category = class.category.clone();
                rank = item_rank;
            }
        }
    }
    category
}

/// Tags a list of events
///
/// An event can have many tags (as opposed to only one category) which will be put into the `$tags` key of
/// the event data object.
pub fn tag(mut events: Vec<Event>, rules: &[(String, Rule)]) -> Vec<Event> {
    let mut events_tagged = Vec::new();
    for event in events.drain(..) {
        events_tagged.push(tag_one(event, rules));
    }
    events_tagged
}

fn tag_one(mut event: Event, rules: &[(String, Rule)]) -> Event {
    let mut tags: Vec<String> = Vec::new();
    for (cls, rule) in rules {
        if rule.matches(&event) {
            tags.push(cls.clone());
        }
    }
    tags.sort_unstable();
    tags.dedup();
    event.data.insert("$tags".into(), serde_json::json!(tags));
    event
}

fn _effective_rank(category: &[String], priority: Option<i64>) -> i64 {
    // Defaulting to depth is the shipped contract and what Erik suggested
    // for the optional override: https://github.com/ActivityWatch/aw-server-rust/issues/597
    priority.unwrap_or(category.len() as i64)
}

#[test]
fn test_rule() {
    let mut e_match = Event::default();
    e_match
        .data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut e_no_match = Event::default();
    e_no_match
        .data
        .insert("nonono".into(), serde_json::json!("no match!"));

    let rule_from_regex = Rule::from(Regex::new("test").unwrap());
    let rule_from_new = Rule::Regex(RegexRule::new("test", false, None).unwrap());
    let rule_none = Rule::None;
    assert!(rule_from_regex.matches(&e_match));
    assert!(rule_from_new.matches(&e_match));
    assert!(!rule_from_regex.matches(&e_no_match));
    assert!(!rule_from_new.matches(&e_no_match));

    assert!(!rule_none.matches(&e_match));
}

#[test]
fn test_rule_lookahead() {
    // Originally requested by a user here, to match aw-server-python: https://canary.discord.com/channels/755040852727955476/755334543891759194/994291987878522961
    let mut e_match = Event::default();
    e_match
        .data
        .insert("test".into(), serde_json::json!("testing lookahead"));

    let rule_from_regex = Rule::from(Regex::new("testing (?!lookahead)").unwrap());
    assert!(!rule_from_regex.matches(&e_match));
}

#[test]
fn test_rule_select_keys() {
    let mut event = Event::default();
    event
        .data
        .insert("app".into(), serde_json::json!("terminal"));
    event
        .data
        .insert("title".into(), serde_json::json!("just a test"));
    event.data.insert("pid".into(), serde_json::json!(123));

    let title_only =
        Rule::Regex(RegexRule::new("test", false, Some(vec!["title".into()])).unwrap());
    let app_only = Rule::Regex(RegexRule::new("test", false, Some(vec!["app".into()])).unwrap());
    let missing_key =
        Rule::Regex(RegexRule::new("test", false, Some(vec!["missing".into()])).unwrap());
    let non_string_key =
        Rule::Regex(RegexRule::new("123", false, Some(vec!["pid".into()])).unwrap());

    assert!(title_only.matches(&event));
    assert!(!app_only.matches(&event));
    assert!(!missing_key.matches(&event));
    assert!(!non_string_key.matches(&event));
}

#[test]
fn test_rule_select_keys_empty_list() {
    // An empty select_keys list should return an error rather than
    // silently producing a rule that never matches anything.
    let result = RegexRule::new("test", false, Some(vec![]));
    assert!(result.is_err());
}
#[test]
fn test_categorize() {
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let rules: Vec<CategoryRule> = vec![
        CategoryRule::new(
            vec!["Test".into()],
            Rule::from(Regex::new(r"test").unwrap()),
        ),
        CategoryRule::new(
            vec!["Test".into(), "Subtest".into()],
            Rule::from(Regex::new(r"test").unwrap()),
        ),
        CategoryRule::new(
            vec!["Other".into()],
            Rule::from(Regex::new(r"nonmatching").unwrap()),
        ),
    ];
    events = categorize(events, &rules);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().unwrap().data.get("$category").unwrap(),
        &serde_json::json!(vec!["Test", "Subtest"])
    );
}

#[test]
fn test_categorize_uncategorized() {
    // Checks that the category correctly becomes uncategorized when no category matches
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let rules: Vec<CategoryRule> = vec![CategoryRule::new(
        vec!["Non-matching".into(), "test".into()],
        Rule::from(Regex::new(r"not going to match").unwrap()),
    )];
    events = categorize(events, &rules);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().unwrap().data.get("$category").unwrap(),
        &serde_json::json!(vec!["Uncategorized"])
    );
}

#[cfg(test)]
fn event_with_data(value: &str) -> Event {
    let mut e = Event::default();
    e.data.insert("test".into(), serde_json::json!(value));
    e
}

#[cfg(test)]
fn category_of(events: &[Event]) -> &serde_json::Value {
    events.first().unwrap().data.get("$category").unwrap()
}

#[test]
fn test_categorize_depth_wins_without_priority() {
    // Reported scenario from ActivityWatch/aw-server-rust#597: a deeper nested
    // match still beats a shallower match when neither rule sets priority.
    // Category A (depth 1) and Category B → B1 (depth 2) both match.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A".into()], Rule::from(Regex::new(r"test").unwrap())),
            CategoryRule::new(
                vec!["B".into(), "B1".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            ),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["B", "B1"]));
}

#[test]
fn test_categorize_explicit_priority_overrides_depth() {
    // The same #597 tree, but A is given a higher priority than B1's depth.
    // Organizational nesting no longer forces B1 to win.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A".into()], Rule::from(Regex::new(r"test").unwrap()))
                .with_priority(10),
            CategoryRule::new(
                vec!["B".into(), "B1".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            ),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["A"]));
}

#[test]
fn test_categorize_lower_priority_loses_to_default_depth() {
    // A deep rule can also be demoted below a shallow rule's default (depth)
    // by setting an explicit lower priority on the deep rule.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A".into()], Rule::from(Regex::new(r"test").unwrap())),
            CategoryRule::new(
                vec!["B".into(), "B1".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            )
            .with_priority(0),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["A"]));
}

#[test]
fn test_categorize_equal_priority_keeps_later_match() {
    // Preserve the historical `>=` later-wins rule when ranks tie.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(
                vec!["First".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            )
            .with_priority(5),
            CategoryRule::new(
                vec!["Second".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            )
            .with_priority(5),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["Second"]));
}

#[test]
fn test_categorize_negative_priority_still_beats_uncategorized() {
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["Low".into()], Rule::from(Regex::new(r"test").unwrap()))
                .with_priority(-100),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["Low"]));
}

#[test]
fn test_categorize_cache_correctness() {
    // Verifies that the deduplication cache produces the same result as
    // per-event categorization when many events share the same data.
    let mut base = Event::default();
    base.data.insert("app".into(), serde_json::json!("firefox"));
    base.data
        .insert("title".into(), serde_json::json!("GitHub"));

    let mut other = Event::default();
    other
        .data
        .insert("app".into(), serde_json::json!("terminal"));
    other.data.insert("title".into(), serde_json::json!("bash"));

    // 50 events with same data, then 1 different event, then 50 more same
    let mut events: Vec<Event> = std::iter::repeat(base.clone())
        .take(50)
        .chain(std::iter::once(other.clone()))
        .chain(std::iter::repeat(base.clone()).take(50))
        .collect();

    let rules: Vec<CategoryRule> = vec![
        CategoryRule::new(
            vec!["Browser".into()],
            Rule::Regex(RegexRule::new("firefox", true, Some(vec!["app".into()])).unwrap()),
        ),
        CategoryRule::new(
            vec!["Terminal".into()],
            Rule::Regex(RegexRule::new("terminal", true, Some(vec!["app".into()])).unwrap()),
        ),
    ];

    events = categorize(events, &rules);

    assert_eq!(events.len(), 101);
    // All firefox events → Browser
    for e in events.iter().take(50) {
        assert_eq!(
            e.data.get("$category").unwrap(),
            &serde_json::json!(vec!["Browser"])
        );
    }
    // The single terminal event → Terminal
    assert_eq!(
        events[50].data.get("$category").unwrap(),
        &serde_json::json!(vec!["Terminal"])
    );
    // Remaining firefox events → Browser (cache hit path)
    for e in events.iter().skip(51) {
        assert_eq!(
            e.data.get("$category").unwrap(),
            &serde_json::json!(vec!["Browser"])
        );
    }
}

#[test]
fn test_tag() {
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let rules: Vec<(String, Rule)> = vec![
        ("test".into(), Rule::from(Regex::new(r"test").unwrap())),
        ("test-2".into(), Rule::from(Regex::new(r"test").unwrap())),
        (
            "nomatch".into(),
            Rule::from(Regex::new(r"nomatch").unwrap()),
        ),
    ];
    events = tag(events, &rules);

    assert_eq!(events.len(), 1);

    let event = events.first().unwrap();
    let tags = event.data.get("$tags").unwrap();
    assert_eq!(tags, &serde_json::json!(vec!["test", "test-2"]));
}

/// Verify that syntactically invalid regex patterns are rejected by RegexRule::new()
/// rather than panicking or silently succeeding.
///
/// Note: fancy_regex supports possessive quantifiers (e.g. `++`, `**`) which standard
/// Python `re` does not. The original ActivityWatch#1340 bug (`Notepad++` → 500 error)
/// only affected the Python aw-server; in aw-server-rust `Notepad++` is a valid
/// possessive quantifier and is accepted. Users wanting a literal `+` must escape it:
/// `Notepad\+\+`.
#[test]
fn test_invalid_regex_patterns_are_rejected() {
    let invalid_patterns = [
        "***",       // no target for first `*` quantifier
        "???",       // no target for first `?` quantifier
        "[unclosed", // unclosed character class
        "(",         // unclosed group
        "(?P<name",  // malformed named capturing group
        "\\",        // lone backslash (incomplete escape sequence)
        "(?i",       // incomplete flag group (no closing parenthesis)
    ];
    for pattern in &invalid_patterns {
        let result = RegexRule::new(pattern, false, None);
        assert!(
            result.is_err(),
            "Expected pattern {:?} to be rejected as invalid regex, but it was accepted",
            pattern
        );
    }
}

/// Verify that valid patterns — including possessive quantifiers and lookaheads — are
/// accepted. These cover the correct workaround for ActivityWatch#1340 and other
/// patterns users commonly write.
#[test]
fn test_valid_regex_patterns_are_accepted() {
    let valid_patterns = [
        r"Notepad\+\+", // literal `+` match — correct workaround for #1340
        "Notepad++",    // possessive quantifier (valid in fancy_regex, unlike Python re)
        r"test.*value",
        r"(?i)case.insensitive",
        r"^start",
        r"end$",
        r"\d+",
        r"[a-z]+",
        r"(group1|group2)",
        r"look(?=ahead)",
        r"look(?!ahead)",
    ];
    for pattern in &valid_patterns {
        let result = RegexRule::new(pattern, false, None);
        assert!(
            result.is_ok(),
            "Expected pattern {:?} to be accepted as valid regex, but it was rejected: {:?}",
            pattern,
            result.err()
        );
    }
}
