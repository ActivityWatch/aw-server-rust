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
    Logical(LogicalRule),
}

trait RuleTrait {
    fn matches(&self, event: &Event, values: &MatchValues) -> bool;

    /// Whether this rule reads the shared extracted values (i.e. matches
    /// against all of the event's values rather than `select_keys`).
    fn needs_values(&self) -> bool;
}

impl RuleTrait for Rule {
    fn matches(&self, event: &Event, values: &MatchValues) -> bool {
        match self {
            Rule::None => false,
            Rule::Regex(rule) => rule.matches(event, values),
            Rule::Logical(rule) => rule.matches(event, values),
        }
    }

    fn needs_values(&self) -> bool {
        match self {
            Rule::None => false,
            Rule::Regex(regex_rule) => regex_rule.select_keys.is_none(),
            Rule::Logical(rule) => rule.needs_values(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOperator {
    And,
    Or,
}

/// Combines nested rules with `and`/`or`, so one category can match e.g.
/// "app is Safari AND title ends with YouTube", or several unrelated apps.
pub struct LogicalRule {
    rules: Vec<Rule>,
    operator: LogicalOperator,
}

impl LogicalRule {
    /// An empty rule list is rejected: a vacuous `and` would match every event.
    pub fn new(rules: Vec<Rule>, operator: LogicalOperator) -> Result<Self, String> {
        if rules.is_empty() {
            return Err("logical rule must contain at least one rule".to_string());
        }
        Ok(Self { rules, operator })
    }
}

impl RuleTrait for LogicalRule {
    fn matches(&self, event: &Event, values: &MatchValues) -> bool {
        match self.operator {
            LogicalOperator::And => self.rules.iter().all(|r| r.matches(event, values)),
            LogicalOperator::Or => self.rules.iter().any(|r| r.matches(event, values)),
        }
    }

    fn needs_values(&self) -> bool {
        self.rules.iter().any(|r| r.needs_values())
    }
}

/// The string values of an event's data, extracted once and shared across every
/// rule in the set.
///
/// Rules without `select_keys` match against all of an event's values. Extracting
/// them once here avoids re-iterating `event.data.values()` (and re-running
/// `as_str()`) for each of the potentially hundreds of rules.
struct MatchValues<'a>(Vec<&'a str>);

impl<'a> MatchValues<'a> {
    fn from_event(event: &'a Event) -> Self {
        Self(event.data.values().filter_map(|v| v.as_str()).collect())
    }

    /// An empty set, for rule sets where no rule reads the shared values.
    fn none() -> Self {
        Self(Vec::new())
    }
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
    fn matches(&self, event: &Event, values: &MatchValues) -> bool {
        match &self.select_keys {
            Some(select_keys) => select_keys
                .iter()
                .filter_map(|key| event.data.get(key))
                .any(|val| self.value_matches(val)),
            // `values` holds the same strings (in the same order) that the
            // previous `event.data.values()` iteration produced.
            None => values
                .0
                .iter()
                .any(|value| self.regex.is_match(value).unwrap_or(false)),
        }
    }

    fn needs_values(&self) -> bool {
        self.select_keys.is_none()
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
/// `priority` is an optional integer ranking score. When set, it is used
/// instead of the depth-derived default to pick among matching rules (higher
/// wins). When `None`, ranking falls back to `depth * 10` so existing configs
/// keep their current ordering, while explicit values can slot between levels
/// (depth 1 → 10, depth 2 → 20).
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
/// Ranking is the optional integer `priority` on the rule when present,
/// otherwise `depth * 10` ("the deepest one will be chosen", with room to
/// slot values between levels). Equal ranks keep the later match, matching
/// the previous depth-only `>=` comparison.
///
/// Performance: two complementary optimizations.
///
/// 1. Rules are pre-ranked once per `categorize` call and evaluated in
///    descending rank order, so the first matching rule *is* the winner and the
///    remaining (lower-ranked) rules can be skipped. Previously every event was
///    matched against every rule, even after a top-priority match.
/// 2. An in-memory cache keyed on the event's data JSON means events with
///    identical data (same app/title — very common in practice) are only matched
///    against the rule set once. On a month's data with 50k+ events but only a
///    few hundred distinct app/title pairs this reduces regex work by >99%.
pub fn categorize(mut events: Vec<Event>, rules: &[CategoryRule]) -> Vec<Event> {
    let ranked_rules = _ranked_rules(rules);
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
            .or_insert_with(|| _pick_category(&event, &ranked_rules))
            .clone();
        event
            .data
            .insert("$category".into(), serde_json::json!(category));
        classified_events.push(event);
    }
    classified_events
}

/// A [`CategoryRule`] with its rank precomputed, for ordered evaluation.
struct RankedRule<'a> {
    rank: i64,
    index: usize,
    rule: &'a CategoryRule,
}

/// Precomputes rule ranks and orders rules so that the first match is the
/// winner under the historical `>=` semantics.
///
/// The old best-of-all loop selected the matching rule with the highest
/// `_effective_rank`, keeping the *later* rule on a tie. Sorting by rank
/// descending and breaking ties by descending original index makes the
/// first match in this order exactly that rule, so `_pick_category` can return
/// early instead of running every remaining rule.
///
/// Rules with an empty category path are dropped up front: they can never
/// replace the `Uncategorized` fallback (old depth comparison: len 0 does not
/// beat Uncategorized's len 1).
fn _ranked_rules(rules: &[CategoryRule]) -> Vec<RankedRule<'_>> {
    let mut ranked: Vec<RankedRule> = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| !rule.category.is_empty())
        .map(|(index, rule)| RankedRule {
            rank: _effective_rank(&rule.category, rule.priority),
            index,
            rule,
        })
        .collect();
    ranked.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| b.index.cmp(&a.index)));
    ranked
}

fn _pick_category(event: &Event, ranked_rules: &[RankedRule<'_>]) -> Vec<String> {
    // Rules with `select_keys` match against specific keys and never read the
    // shared values, so skip the extraction entirely when no rule needs it
    // (including the empty-rule-set case).
    let values = if ranked_rules
        .iter()
        .any(|ranked| ranked.rule.rule.needs_values())
    {
        MatchValues::from_event(event)
    } else {
        MatchValues::none()
    };
    // `Uncategorized` loses to any non-empty match, including one with a very
    // low explicit priority, so no rank threshold is needed here — the first
    // match in rank order always wins.
    for ranked in ranked_rules {
        if ranked.rule.rule.matches(event, &values) {
            return ranked.rule.category.clone();
        }
    }
    vec!["Uncategorized".into()]
}

/// Tags a list of events
///
/// An event can have many tags (as opposed to only one category) which will be put into the `$tags` key of
/// the event data object.
pub fn tag(mut events: Vec<Event>, rules: &[(String, Rule)]) -> Vec<Event> {
    // Whether any rule needs shared values does not depend on the event,
    // so compute it once instead of scanning the rule set per event.
    let needs_values = rules.iter().any(|(_, rule)| rule.needs_values());
    let mut events_tagged = Vec::new();
    for event in events.drain(..) {
        events_tagged.push(tag_one(event, rules, needs_values));
    }
    events_tagged
}

fn tag_one(mut event: Event, rules: &[(String, Rule)], needs_values: bool) -> Event {
    let values = if needs_values {
        MatchValues::from_event(&event)
    } else {
        MatchValues::none()
    };
    let mut tags: Vec<String> = Vec::new();
    for (cls, rule) in rules {
        if rule.matches(&event, &values) {
            tags.push(cls.clone());
        }
    }
    drop(values);
    tags.sort_unstable();
    tags.dedup();
    event.data.insert("$tags".into(), serde_json::json!(tags));
    event
}

fn _effective_rank(category: &[String], priority: Option<i64>) -> i64 {
    // Integer-only. Default is depth * 10 so explicit priorities can slot
    // between nesting levels (depth 1 → 10, depth 2 → 20). Relative order of
    // unprioritized rules is unchanged.
    // https://github.com/ActivityWatch/aw-server-rust/pull/663#issuecomment-5481349757
    priority.unwrap_or((category.len() as i64) * 10)
}

#[cfg(test)]
fn rule_matches(rule: &impl RuleTrait, event: &Event) -> bool {
    rule.matches(event, &MatchValues::from_event(event))
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
    assert!(rule_matches(&rule_from_regex, &e_match));
    assert!(rule_matches(&rule_from_new, &e_match));
    assert!(!rule_matches(&rule_from_regex, &e_no_match));
    assert!(!rule_matches(&rule_from_new, &e_no_match));

    assert!(!rule_matches(&rule_none, &e_match));
}

#[test]
fn test_rule_lookahead() {
    // Originally requested by a user here, to match aw-server-python: https://canary.discord.com/channels/755040852727955476/755334543891759194/994291987878522961
    let mut e_match = Event::default();
    e_match
        .data
        .insert("test".into(), serde_json::json!("testing lookahead"));

    let rule_from_regex = Rule::from(Regex::new("testing (?!lookahead)").unwrap());
    assert!(!rule_matches(&rule_from_regex, &e_match));
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

    assert!(rule_matches(&title_only, &event));
    assert!(!rule_matches(&app_only, &event));
    assert!(!rule_matches(&missing_key, &event));
    assert!(!rule_matches(&non_string_key, &event));
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
    // The same #597 tree, but A is given a higher priority than B1's default
    // (depth 2 → 20). Organizational nesting no longer forces B1 to win.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A".into()], Rule::from(Regex::new(r"test").unwrap()))
                .with_priority(25),
            CategoryRule::new(
                vec!["B".into(), "B1".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            ),
        ],
    );
    assert_eq!(category_of(&events), &serde_json::json!(vec!["A"]));
}

#[test]
fn test_categorize_inter_level_priority() {
    // depth * 10 leaves integers between levels: 15 beats a depth-1 default
    // (10) but loses to a depth-2 default (20).
    let between = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A".into()], Rule::from(Regex::new(r"test").unwrap())),
            CategoryRule::new(vec!["A2".into()], Rule::from(Regex::new(r"test").unwrap()))
                .with_priority(15),
        ],
    );
    assert_eq!(category_of(&between), &serde_json::json!(vec!["A2"]));

    let still_loses_to_deeper = categorize(
        vec![event_with_data("just a test")],
        &[
            CategoryRule::new(vec!["A2".into()], Rule::from(Regex::new(r"test").unwrap()))
                .with_priority(15),
            CategoryRule::new(
                vec!["B".into(), "B1".into()],
                Rule::from(Regex::new(r"test").unwrap()),
            ),
        ],
    );
    assert_eq!(
        category_of(&still_loses_to_deeper),
        &serde_json::json!(vec!["B", "B1"])
    );
}

#[test]
fn test_categorize_lower_priority_loses_to_default_depth() {
    // A deep rule can also be demoted below a shallow rule's default
    // (depth * 10) by setting an explicit lower priority on the deep rule.
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
fn test_categorize_empty_category_keeps_uncategorized() {
    // Empty path used to lose to Uncategorized (depth 0 < 1). The MIN-rank
    // sentinel must not let it replace the fallback.
    let events = categorize(
        vec![event_with_data("just a test")],
        &[CategoryRule::new(
            vec![],
            Rule::from(Regex::new(r"test").unwrap()),
        )],
    );
    assert_eq!(
        category_of(&events),
        &serde_json::json!(vec!["Uncategorized"])
    );
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

/// Deterministic xorshift RNG, so the randomized equivalence test below is
/// reproducible without adding a `rand` dependency.
#[cfg(test)]
struct Rng(u64);

#[cfg(test)]
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Independent old-style matching for the test oracle: deliberately does NOT
/// use the shared `MatchValues` extraction or `RuleTrait::matches`, so a
/// regression in that path cannot make both sides of the equivalence test
/// agree on the same wrong answer.
#[cfg(test)]
fn naive_rule_matches(rule: &Rule, event: &Event) -> bool {
    match rule {
        Rule::None => false,
        Rule::Regex(r) => {
            let matches_value = |v: &serde_json::Value| {
                v.as_str()
                    .map(|s| r.regex.is_match(s).unwrap_or(false))
                    .unwrap_or(false)
            };
            match &r.select_keys {
                Some(keys) => keys
                    .iter()
                    .filter_map(|key| event.data.get(key))
                    .any(matches_value),
                None => event.data.values().any(matches_value),
            }
        }
        Rule::Logical(l) => match l.operator {
            LogicalOperator::And => l.rules.iter().all(|r| naive_rule_matches(r, event)),
            LogicalOperator::Or => l.rules.iter().any(|r| naive_rule_matches(r, event)),
        },
    }
}

/// The previous best-of-all implementation, kept as the reference oracle for
/// the randomized equivalence test.
#[cfg(test)]
fn naive_pick_category(event: &Event, rules: &[CategoryRule]) -> Vec<String> {
    let mut category: Vec<String> = vec!["Uncategorized".into()];
    let mut rank = i64::MIN;
    for class in rules {
        if class.category.is_empty() {
            continue;
        }
        if naive_rule_matches(&class.rule, event) {
            let item_rank = _effective_rank(&class.category, class.priority);
            if item_rank >= rank {
                category = class.category.clone();
                rank = item_rank;
            }
        }
    }
    category
}

#[test]
fn test_categorize_matches_naive_best_of_all() {
    // Randomized cross-check: the rank-ordered early exit must select exactly
    // the category the old best-of-all (`>=`, later-wins) loop selected —
    // including ties, explicit priorities, empty paths, `select_keys` and
    // `ignore_case`.
    const PATTERNS: [&str; 6] = [
        "firefox",
        "chrome",
        "^term",
        "code$",
        "note|slack",
        "no-match-xyz",
    ];
    const KEYS: [&str; 3] = ["app", "title", "pid"];

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..200 {
        let rule_count = 1 + rng.below(12) as usize;
        let mut rules: Vec<CategoryRule> = Vec::with_capacity(rule_count);
        for _ in 0..rule_count {
            let pattern = PATTERNS[rng.below(PATTERNS.len() as u64) as usize];
            let ignore_case = rng.below(2) == 0;
            let select_keys = if rng.below(2) == 0 {
                None
            } else {
                Some(vec![KEYS[rng.below(KEYS.len() as u64) as usize].to_string()])
            };
            let mut rule = Rule::Regex(RegexRule::new(pattern, ignore_case, select_keys).unwrap());
            // Occasionally combine rules logically, so the naive oracle cross-check
            // also covers the new and/or branch (including nesting).
            if rng.below(3) == 0 {
                let pattern2 = PATTERNS[rng.below(PATTERNS.len() as u64) as usize];
                let ignore_case2 = rng.below(2) == 0;
                let select_keys2 = if rng.below(2) == 0 {
                    None
                } else {
                    Some(vec![KEYS[rng.below(KEYS.len() as u64) as usize].to_string()])
                };
                let second =
                    Rule::Regex(RegexRule::new(pattern2, ignore_case2, select_keys2).unwrap());
                let mut subrules = vec![rule, second];
                if rng.below(4) == 0 {
                    let pattern3 = PATTERNS[rng.below(PATTERNS.len() as u64) as usize];
                    let third = Rule::Regex(RegexRule::new(pattern3, false, None).unwrap());
                    let inner = Rule::Logical(
                        LogicalRule::new(vec![subrules.pop().unwrap(), third], LogicalOperator::Or)
                            .unwrap(),
                    );
                    subrules.push(inner);
                }
                let operator = if rng.below(2) == 0 {
                    LogicalOperator::And
                } else {
                    LogicalOperator::Or
                };
                rule = Rule::Logical(LogicalRule::new(subrules, operator).unwrap());
            }
            let depth = 1 + rng.below(3) as usize;
            let category: Vec<String> = (0..depth).map(|i| format!("Cat{case}_{i}")).collect();
            let mut cr = CategoryRule::new(category, rule);
            // Mix unprioritized rules (depth * 10) with explicit priorities,
            // deliberately including values that collide (ties).
            if rng.below(2) == 0 {
                cr = cr.with_priority(rng.below(41) as i64 - 10);
            }
            rules.push(cr);
        }

        let event_count = 1 + rng.below(10) as usize;
        let events: Vec<Event> = (0..event_count)
            .map(|_| {
                let mut e = Event::default();
                for key in KEYS {
                    if rng.below(3) == 0 {
                        continue; // sometimes omit a key
                    }
                    let value = if key == "pid" {
                        serde_json::json!(rng.below(1000))
                    } else {
                        serde_json::json!(PATTERNS[rng.below(PATTERNS.len() as u64) as usize])
                    };
                    e.data.insert(key.into(), value);
                }
                e
            })
            .collect();

        let expected: Vec<Vec<String>> = events
            .iter()
            .map(|e| naive_pick_category(e, &rules))
            .collect();
        let actual: Vec<Vec<String>> = categorize(events.clone(), &rules)
            .iter()
            .map(|e| {
                serde_json::from_value(e.data.get("$category").unwrap().clone())
                    .expect("$category must be a category array")
            })
            .collect();

        assert_eq!(
            actual, expected,
            "case {case}: rank-ordered pick diverged from best-of-all"
        );
    }
}

#[cfg(test)]
#[test]
fn test_logical_rule() {
    let mut event = Event::default();
    event.data.insert("app".into(), serde_json::json!("Safari"));
    event
        .data
        .insert("title".into(), serde_json::json!("Cats - YouTube"));
    let regex = |re: &str, key: &str| {
        Rule::Regex(RegexRule::new(re, false, Some(vec![key.into()])).unwrap())
    };
    let values = MatchValues::from_event(&event);

    let and = Rule::Logical(
        LogicalRule::new(
            vec![regex("^Safari$", "app"), regex("YouTube$", "title")],
            LogicalOperator::And,
        )
        .unwrap(),
    );
    assert!(and.matches(&event, &values));

    let and_miss = Rule::Logical(
        LogicalRule::new(
            vec![regex("^Firefox$", "app"), regex("YouTube$", "title")],
            LogicalOperator::And,
        )
        .unwrap(),
    );
    assert!(!and_miss.matches(&event, &values));

    // Nested: (app=Firefox) OR (app=Safari AND title~YouTube)
    let nested = Rule::Logical(
        LogicalRule::new(vec![regex("^Firefox$", "app"), and], LogicalOperator::Or).unwrap(),
    );
    assert!(nested.matches(&event, &values));
    assert!(!nested.needs_values());

    let unscoped = Rule::Logical(
        LogicalRule::new(
            vec![Rule::from(Regex::new("Cats").unwrap())],
            LogicalOperator::Or,
        )
        .unwrap(),
    );
    assert!(unscoped.needs_values());
    assert!(unscoped.matches(&event, &values));

    assert!(LogicalRule::new(vec![], LogicalOperator::And).is_err());
}
