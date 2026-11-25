/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py
use aw_models::Event;
use fancy_regex::Regex;
use lru::LruCache;
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

const REGEX_CACHE_CAPACITY: usize = 512;

static REGEX_CACHE: OnceLock<Mutex<LruCache<String, Arc<Regex>>>> = OnceLock::new();

pub enum Rule {
    None,
    Regex(RegexRule),
}

/// The check the unchecked public APIs pass to their checked variants.
fn no_check() -> Result<(), Infallible> {
    Ok(())
}

fn into_ok<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

trait RuleTrait {
    /// Calls `check` before every regex match, and stops with its error.
    fn matches<E>(
        &self,
        event: &Event,
        values: &MatchValues,
        check: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E>;

    /// Whether this rule reads the shared extracted values (i.e. matches
    /// against all of the event's values rather than `select_keys`).
    fn needs_values(&self) -> bool;
}

impl RuleTrait for Rule {
    fn matches<E>(
        &self,
        event: &Event,
        values: &MatchValues,
        check: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        match self {
            Rule::None => Ok(false),
            Rule::Regex(rule) => rule.matches(event, values, check),
        }
    }

    fn needs_values(&self) -> bool {
        match self {
            Rule::None => false,
            Rule::Regex(regex_rule) => regex_rule.select_keys.is_none(),
        }
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
    // An empty pattern compiles to a match-all regex, which would silently
    // categorize every event. aw-core's Python `Rule` deliberately never
    // matches on an empty regex (see ActivityWatch/aw-webui#1026); mirror
    // that here instead of diverging between the two servers.
    never_matches: bool,
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

        let never_matches = regex_str.is_empty();

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

        Ok(RegexRule {
            regex,
            select_keys,
            never_matches,
        })
    }

    fn value_matches<E>(
        &self,
        value: &str,
        check: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        check()?;
        Ok(self.regex.is_match(value).unwrap_or(false))
    }
}

/// This struct defines the rules for classification.
/// For now it just needs to contain the regex to match with, but in the future it might contain a
/// glob-pattern, or other options for classifying.
/// It's puropse is to make the API easy to extend in the future without having to break backwards
/// compatibility (or have to maintain "old" query2 functions).
impl RuleTrait for RegexRule {
    fn matches<E>(
        &self,
        event: &Event,
        values: &MatchValues,
        check: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        if self.never_matches {
            return Ok(false);
        }
        match &self.select_keys {
            Some(select_keys) => {
                for value in select_keys
                    .iter()
                    .filter_map(|key| event.data.get(key))
                    .filter_map(|value| value.as_str())
                {
                    if self.value_matches(value, check)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            // `values` holds the same strings (in the same order) that the
            // previous `event.data.values()` iteration produced.
            None => {
                for value in &values.0 {
                    if self.value_matches(value, check)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn needs_values(&self) -> bool {
        self.select_keys.is_none()
    }
}

impl From<Regex> for Rule {
    fn from(re: Regex) -> Self {
        // Mirror `RegexRule::new`: derive `never_matches` from the source
        // pattern so an empty regex passed through this public conversion
        // cannot silently match every event.
        let never_matches = re.as_str().is_empty();
        Rule::Regex(RegexRule {
            regex: Arc::new(re),
            select_keys: None,
            never_matches,
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
pub fn categorize(events: Vec<Event>, rules: &[CategoryRule]) -> Vec<Event> {
    into_ok(categorize_checked(events, rules, no_check))
}

/// Like [`categorize`], but calls `check` before every event (cache hits
/// included), every rule and every regex match, and once more at the end.
/// The first error `check` returns stops the work and is returned unchanged.
///
/// This bounds the work between regex engine calls, not inside one: a single
/// `fancy_regex` match or compilation cannot be interrupted, so a caller's
/// deadline can be overshot by up to one engine call.
pub fn categorize_checked<E>(
    mut events: Vec<Event>,
    rules: &[CategoryRule],
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<Vec<Event>, E> {
    let ranked_rules = _ranked_rules(rules);
    // Cache: serialized event data → assigned category
    let mut category_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut classified_events = Vec::with_capacity(events.len());
    for mut event in events.drain(..) {
        check()?;
        // Key on the full event data. serde_json::Map preserves insertion order, so
        // events with the same fields in the same order produce the same key — which
        // is the normal case for heartbeat-based watchers.
        let cache_key = serde_json::to_string(&event.data).unwrap_or_default();
        let category = match category_cache.get(&cache_key) {
            Some(category) => category.clone(),
            None => {
                let category = _pick_category(&event, &ranked_rules, &mut check)?;
                category_cache.insert(cache_key, category.clone());
                category
            }
        };
        event
            .data
            .insert("$category".into(), serde_json::json!(category));
        classified_events.push(event);
    }
    check()?;
    Ok(classified_events)
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

fn _pick_category<E>(
    event: &Event,
    ranked_rules: &[RankedRule<'_>],
    check: &mut impl FnMut() -> Result<(), E>,
) -> Result<Vec<String>, E> {
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
        check()?;
        if ranked.rule.rule.matches(event, &values, check)? {
            return Ok(ranked.rule.category.clone());
        }
    }
    Ok(vec!["Uncategorized".into()])
}

/// Tags a list of events
///
/// An event can have many tags (as opposed to only one category) which will be put into the `$tags` key of
/// the event data object.
pub fn tag(events: Vec<Event>, rules: &[(String, Rule)]) -> Vec<Event> {
    into_ok(tag_checked(events, rules, no_check))
}

/// Like [`tag`], but calls `check` before every event, every rule and every
/// regex match, and once more at the end; see [`categorize_checked`] for the
/// error and overshoot semantics.
pub fn tag_checked<E>(
    mut events: Vec<Event>,
    rules: &[(String, Rule)],
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<Vec<Event>, E> {
    // Whether any rule needs shared values does not depend on the event,
    // so compute it once instead of scanning the rule set per event.
    let needs_values = rules.iter().any(|(_, rule)| rule.needs_values());
    let mut events_tagged = Vec::new();
    for event in events.drain(..) {
        check()?;
        events_tagged.push(tag_one(event, rules, needs_values, &mut check)?);
    }
    check()?;
    Ok(events_tagged)
}

fn tag_one<E>(
    mut event: Event,
    rules: &[(String, Rule)],
    needs_values: bool,
    check: &mut impl FnMut() -> Result<(), E>,
) -> Result<Event, E> {
    let values = if needs_values {
        MatchValues::from_event(&event)
    } else {
        MatchValues::none()
    };
    let mut tags: Vec<String> = Vec::new();
    for (cls, rule) in rules {
        check()?;
        if rule.matches(&event, &values, check)? {
            tags.push(cls.clone());
        }
    }
    drop(values);
    tags.sort_unstable();
    tags.dedup();
    event.data.insert("$tags".into(), serde_json::json!(tags));
    Ok(event)
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
    into_ok(rule.matches(event, &MatchValues::from_event(event), &mut no_check))
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
fn test_empty_regex_never_matches() {
    // An empty pattern compiles to a match-all regex in fancy_regex. aw-core's
    // Python `Rule` deliberately never matches on an empty regex (it would
    // "erroneously match everything"); aw-server-rust must agree, or the two
    // servers categorize the same rule set differently.
    let mut event = Event::default();
    event
        .data
        .insert("test".into(), serde_json::json!("just a test"));

    let no_select_keys = Rule::Regex(RegexRule::new("", false, None).unwrap());
    assert!(!rule_matches(&no_select_keys, &event));

    let with_select_keys =
        Rule::Regex(RegexRule::new("", false, Some(vec!["test".into()])).unwrap());
    assert!(!rule_matches(&with_select_keys, &event));

    // The public `Rule::from(Regex)` conversion must not bypass the guard:
    // `Regex::new("")` compiles to a match-all regex, so the derived flag has
    // to come from the source pattern, not default to `false`.
    let via_from = Rule::from(Regex::new("").unwrap());
    assert!(!rule_matches(&via_from, &event));

    // Also verify it never matches via the public `categorize` entry point.
    let events = categorize(
        vec![event],
        &[CategoryRule::new(
            vec!["Everything".into()],
            Rule::Regex(RegexRule::new("", false, None).unwrap()),
        )],
    );
    assert_eq!(
        category_of(&events),
        &serde_json::json!(vec!["Uncategorized"])
    );
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
    let mut events: Vec<Event> = std::iter::repeat_n(base.clone(), 50)
        .chain(std::iter::once(other.clone()))
        .chain(std::iter::repeat_n(base.clone(), 50))
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
            let rule = Rule::Regex(RegexRule::new(pattern, ignore_case, select_keys).unwrap());
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
mod checked_tests {
    use super::*;

    /// Counts calls and fails on call number `fail_at` (never, if 0).
    fn counting_check(calls: &mut usize, fail_at: usize) -> impl FnMut() -> Result<(), usize> + '_ {
        move || {
            *calls += 1;
            if *calls == fail_at {
                Err(*calls)
            } else {
                Ok(())
            }
        }
    }

    fn event(pairs: &[(&str, &str)]) -> Event {
        let mut e = Event::default();
        for (key, value) in pairs {
            e.data.insert((*key).into(), serde_json::json!(value));
        }
        e
    }

    fn category_rules(select_keys: Option<Vec<String>>) -> Vec<CategoryRule> {
        vec![
            CategoryRule::new(
                vec!["Never".into()],
                Rule::Regex(RegexRule::new("nomatch", false, select_keys.clone()).unwrap()),
            ),
            CategoryRule::new(
                vec!["Also".into(), "Never".into()],
                Rule::Regex(RegexRule::new("nope", false, select_keys).unwrap()),
            ),
        ]
    }

    fn categorize_calls(events: Vec<Event>, rules: &[CategoryRule]) -> usize {
        let mut calls = 0;
        categorize_checked(events, rules, counting_check(&mut calls, 0)).unwrap();
        calls
    }

    #[test]
    fn categorize_checked_checks_every_event_rule_and_value() {
        let e = event(&[("app", "a"), ("title", "b"), ("url", "c")]);
        // All values: 1 event + 2 rules × (1 rule + 3 values) + 1 final.
        assert_eq!(categorize_calls(vec![e.clone()], &category_rules(None)), 10);
        // select_keys reads only the named string values that exist:
        // 1 event + 2 rules × (1 rule + 2 values) + 1 final.
        let keys = Some(vec!["app".into(), "missing".into(), "url".into()]);
        assert_eq!(categorize_calls(vec![e.clone()], &category_rules(keys)), 8);
        // Cache hits skip the matching but are still checked once each.
        assert_eq!(
            categorize_calls(vec![e.clone(), e.clone(), e], &category_rules(None)),
            12
        );
    }

    #[test]
    fn categorize_checked_propagates_every_cancellation_point() {
        let e = event(&[("app", "a"), ("title", "b")]);
        let events = vec![e.clone(), e, event(&[("title", "other")])];
        for keys in [None, Some(vec!["title".to_string()])] {
            let rules = category_rules(keys);
            let total = categorize_calls(events.clone(), &rules);
            for fail_at in 1..=total {
                let mut calls = 0;
                let result =
                    categorize_checked(events.clone(), &rules, counting_check(&mut calls, fail_at));
                // Stops at the failing check: no later checks, no swallowed error.
                assert_eq!(result.err(), Some(fail_at));
                assert_eq!(calls, fail_at);
            }
        }
    }

    #[test]
    fn tag_checked_propagates_every_cancellation_point() {
        let events = vec![event(&[("app", "a"), ("title", "b")]), event(&[("x", "y")])];
        let rules: Vec<(String, Rule)> = vec![
            (
                "t1".into(),
                Rule::Regex(RegexRule::new("a", false, None).unwrap()),
            ),
            (
                "t2".into(),
                Rule::Regex(RegexRule::new("b", false, Some(vec!["title".into()])).unwrap()),
            ),
        ];
        let mut total = 0;
        let tagged = tag_checked(events.clone(), &rules, counting_check(&mut total, 0)).unwrap();
        assert_eq!(tagged[0].data["$tags"], serde_json::json!(["t1", "t2"]));
        // 2 events × (1 event + 2 rules) + the values each rule tries + 1 final.
        assert!(total > 2 * 3 + 1, "{total}");
        for fail_at in 1..=total {
            let mut calls = 0;
            let result = tag_checked(events.clone(), &rules, counting_check(&mut calls, fail_at));
            assert_eq!(result.err(), Some(fail_at));
            assert_eq!(calls, fail_at);
        }
    }

    #[test]
    fn unchecked_apis_match_checked_results() {
        let events = vec![event(&[("app", "nomatch")]), event(&[("app", "other")])];
        let rules = category_rules(None);
        let checked = categorize_checked(events.clone(), &rules, || Ok::<(), ()>(())).unwrap();
        assert_eq!(categorize(events, &rules), checked);
    }
}
