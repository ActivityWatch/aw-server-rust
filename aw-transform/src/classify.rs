/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py
use aw_models::Event;
use regex::{Regex, RegexBuilder};

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
    regex: Regex,
}

impl RegexRule {
    pub fn new(regex_str: &str, ignore_case: bool) -> Result<RegexRule, regex::Error> {
        let mut regex_builder = RegexBuilder::new(regex_str);
        regex_builder.case_insensitive(ignore_case);
        let regex = regex_builder.build()?;
        Ok(RegexRule { regex })
    }
}

/// This struct defines the rules for classification.
/// For now it just needs to contain the regex to match with, but in the future it might contain a
/// glob-pattern, or other options for classifying.
/// It's puropse is to make the API easy to extend in the future without having to break backwards
/// compatibility (or have to maintain "old" query2 functions).
impl RuleTrait for RegexRule {
    fn matches(&self, event: &Event) -> bool {
        event
            .data
            .values()
            .filter(|val| val.is_string())
            .any(|val| {
                self.regex.is_match(val.as_str().unwrap())
            })
    }
}

impl From<Regex> for Rule {
    fn from(re: Regex) -> Self {
        Rule::Regex(RegexRule { regex: re })
    }
}

/// Categorizes a list of events
///
/// An event can only have one category, although the category may have a hierarchy,
/// for instance: "Work -> ActivityWatch -> aw-server-rust"
/// If multiple categories match, the deepest one will be chosen.
pub fn categorize(mut events: Vec<Event>, rules: &[(Vec<String>, Rule)]) -> Vec<Event> {
    let mut classified_events = Vec::new();
    for event in events.drain(..) {
        classified_events.push(categorize_one(event, rules));
    }
    classified_events
}

fn categorize_one(mut event: Event, rules: &[(Vec<String>, Rule)]) -> Event {
    let mut category: Vec<String> = vec!["Uncategorized".into()];
    for (cat, rule) in rules {
        if rule.matches(&event) {
            category = _pick_highest_ranking_category(category, &cat);
        }
    }
    event
        .data
        .insert("$category".into(), serde_json::json!(category));
    event
}

/// Tags a list of events
///
/// An event can have many tags (as opposed to only one category) which will be put into the `$tags` key of
/// the event data object.
pub fn tag(mut events: Vec<Event>, rules: &[(String, Rule)]) -> Vec<Event> {
    let mut events_tagged = Vec::new();
    for event in events.drain(..) {
        events_tagged.push(tag_one(event, &rules));
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

fn _pick_highest_ranking_category(acc: Vec<String>, item: &[String]) -> Vec<String> {
    if item.len() >= acc.len() {
        // If tag is category with greater or equal depth than current, then choose the new one instead.
        item.to_vec()
    } else {
        acc
    }
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
    let rule_from_new = Rule::Regex(RegexRule::new("test", false).unwrap());
    let rule_none = Rule::None;
    assert_eq!(rule_from_regex.matches(&e_match), true);
    assert_eq!(rule_from_new.matches(&e_match), true);
    assert_eq!(rule_from_regex.matches(&e_no_match), false);
    assert_eq!(rule_from_new.matches(&e_no_match), false);

    assert_eq!(rule_none.matches(&e_match), false);
}

#[test]
fn test_categorize() {
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let rules: Vec<(Vec<String>, Rule)> = vec![
        (
            vec!["Test".into()],
            Rule::from(Regex::new(r"test").unwrap()),
        ),
        (
            vec!["Test".into(), "Subtest".into()],
            Rule::from(Regex::new(r"test").unwrap()),
        ),
        (
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
    let rules: Vec<(Vec<String>, Rule)> = vec![(
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
