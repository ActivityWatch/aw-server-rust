/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py
use std::collections::HashSet;
use std::collections::HashMap;

use crate::models::Event;
use regex::Regex;
use serde_json;

/// This struct defines the rules for classification.
/// For now it just needs to contain the regex to match with, but in the future it might contain a
/// glob-pattern, or other options for classifying.
/// It's puropse is to make the API easy to extend in the future without having to break backwards
/// compatibility (or have to maintain "old" query2 functions).
pub struct Rule {
    regex: Option<Regex>,
}

impl Rule {
    fn matches(&self, event: &Event) -> bool {
        event
            .data
            .values()
            .filter(|val| val.is_string())
            .any(|val| {
                return match &self.regex {
                    Some(re) => re.is_match(val.as_str().unwrap()),
                    None => false,
                };
            })
    }
}

impl From<Regex> for Rule {
    fn from(re: Regex) -> Self {
        Self {
            regex: Some(re.clone()),
        }
    }
}

impl From<HashMap<String, String>> for Rule {
    fn from(obj: HashMap<String, String>) -> Self {
        Self {
            regex: Some(Regex::new(obj.get("regex").unwrap()).unwrap()),
        }
    }
}

/// Categorizes a list of events
///
/// An event can only have one category, although the category may have a hierarchy,
/// for instance: "Work -> ActivityWatch -> aw-server-rust"
/// A category is chosed out of the tags used some rule (such as picking the one that's deepest in the hierarchy)
pub fn categorize(mut events: Vec<Event>, rules: &Vec<(Vec<String>, Rule)>) -> Vec<Event> {
    let mut classified_events = Vec::new();
    for event in events.drain(..) {
        classified_events.push(categorize_one(event, rules));
    }
    return classified_events;
}

fn categorize_one(mut event: Event, rules: &Vec<(Vec<String>, Rule)>) -> Event {
    let mut category: Vec<String> = vec!["Uncategorized".into()];
    for (cat, rule) in rules {
        if rule.matches(&event) {
            category = _pick_highest_ranking_category(category, &cat);
        }
    }
    event
        .data
        .insert("$category".into(), serde_json::json!(category));
    return event;
}

pub fn autotag(mut events: Vec<Event>, rules: &Vec<(String, Rule)>) -> Vec<Event> {
    let mut events_tagged = Vec::new();
    for event in events.drain(..) {
        events_tagged.push(autotag_one(event, &rules));
    }
    return events_tagged;
}

fn autotag_one(mut event: Event, rules: &Vec<(String, Rule)>) -> Event {
    let mut tags: HashSet<String> = HashSet::new();
    for (cls, rule) in rules {
        if rule.matches(&event) {
            tags.insert(cls.clone());
        }
    }
    event.data.insert("$tags".into(), serde_json::json!(tags));
    event
}

fn _match(event: &Event, re: &Regex) -> bool {
    for val in event.data.values() {
        if val.is_string() && re.is_match(val.as_str().unwrap()) {
            return true;
        }
    }
    return false;
}

fn _pick_highest_ranking_category(acc: Vec<String>, item: &Vec<String>) -> Vec<String> {
    if item.len() >= acc.len() {
        // If tag is category with greater or equal depth than current, then choose the new one instead.
        item.clone()
    } else {
        acc
    }
}

fn _cat_format_to_vec(cat: String) -> Vec<String> {
    cat.split("->").map(|s| s.trim().into()).collect()
}

fn _cat_vec_to_format(cat: Vec<String>) -> String {
    cat.join(" -> ")
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
fn test_autotag() {
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
    events = autotag(events, &rules);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events
            .first()
            .unwrap()
            .data
            .get("$tags")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
