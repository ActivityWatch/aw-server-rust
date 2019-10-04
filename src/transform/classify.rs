/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py
use std::collections::HashSet;

use crate::models::Event;
use regex::Regex;
use serde_json;

/// This struct defines the rules
/// For now it just needs to contain the regex to match with, but in the future it might contain a
/// glob-pattern, or other options for classifying.
/// It's puropse is to make the API easy to extend in the future without having to break backwards
/// compatibility (or have to maintain "old" query2 functions).
pub struct Rule {
    regex: Option<Regex>,
}

impl Rule {
    fn from_regex(re: &Regex) -> Self {
        Self {
            regex: Some(re.clone()),
        }
    }

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

// TODO: Deprecate in favor of `categorize` and `autotag`
pub fn classify(mut events: Vec<Event>, rules: &Vec<(String, Regex)>) -> Vec<Event> {
    let mut classified_events = Vec::new();
    let rules_new: Vec<(String, Rule)> = rules
        .iter()
        .map(|(cls, re)| (cls.clone(), Rule::from_regex(re)))
        .collect();
    for event in events.drain(..) {
        classified_events.push(classify_one(event, &rules_new));
    }
    return classified_events;
}

/// First tags and then selects the deepest matching tag as category (by counting number of "->" in name)
fn classify_one(mut event: Event, rules: &Vec<(String, Rule)>) -> Event {
    let mut tags: HashSet<String> = HashSet::new();
    for (cls, rule) in rules {
        if rule.matches(&event) {
            tags.insert(cls.clone());
        }
    }

    // An event can have many tags
    event.data.insert("$tags".into(), serde_json::json!(tags));

    // An event can only have one category, although the category may have a hierarchy,
    // for instance: "Work -> ActivityWatch -> aw-server-rust"
    // A category is chosed out of the tags used some rule (such as picking the one that's deepest in the hierarchy)
    let category = _choose_category(&tags);
    event
        .data
        .insert("$category".into(), serde_json::json!(category));
    event
}

/// Categorizes a list of events
///
/// An event can only have one category, although the category may have a hierarchy,
/// for instance: "Work -> ActivityWatch -> aw-server-rust"
/// A category is chosed out of the tags used some rule (such as picking the one that's deepest in the hierarchy)
// TODO: Classes should be &Vec<(String, Rule)>
pub fn categorize(mut events: Vec<Event>, rules: &Vec<(Vec<String>, Regex)>) -> Vec<Event> {
    let mut classified_events = Vec::new();
    for event in events.drain(..) {
        classified_events.push(categorize_one(event, rules));
    }
    return classified_events;
}

// TODO: Classes should be &Vec<(String, Rule)>
pub fn autotag(mut events: Vec<Event>, rules: &Vec<(String, Regex)>) -> Vec<Event> {
    let mut events_tagged = Vec::new();
    let new_rules: Vec<(String, Rule)> = rules
        .iter()
        .map(|(cls, re)| (cls.clone(), Rule::from_regex(re)))
        .collect();
    for event in events.drain(..) {
        events_tagged.push(classify_one(event, &new_rules));
    }
    return events_tagged;
}

fn categorize_one(mut event: Event, categories: &Vec<(Vec<String>, Regex)>) -> Event {
    let mut category: String = "Uncategorized".into();
    for (cat, re) in categories {
        if _match(&event, &re) {
            // TODO: This shouldn't be cat.join("->"), but if we do end up deciding on this API it'll be easy
            // to remove.
            category = _pick_highest_ranking_category(category, &cat.join("->"));
        }
    }
    event.data.insert(
        "$category".into(),
        serde_json::json!(_cat_format_to_vec(category)),
    );
    return event;
}

fn _match(event: &Event, re: &Regex) -> bool {
    for val in event.data.values() {
        if val.is_string() && re.is_match(val.as_str().unwrap()) {
            return true;
        }
    }
    return false;
}

fn _pick_highest_ranking_category(acc: String, item: &String) -> String {
    if item.matches("->").count() >= acc.matches("->").count() {
        // If tag is category with greater or equal depth than current, then choose the new one instead.
        item.clone()
    } else {
        acc
    }
}

fn _choose_category(tags: &HashSet<String>) -> String {
    tags.iter()
        .fold("Uncategorized".to_string(), |acc, item| {
            return _pick_highest_ranking_category(acc, &item);
        })
        .clone()
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
    let cats: Vec<(Vec<String>, Regex)> = vec![
        (vec!["Test".into()], Regex::new(r"test").unwrap()),
        (
            vec!["Test".into(), "Subtest".into()],
            Regex::new(r"test").unwrap(),
        ),
        (vec!["Other".into()], Regex::new(r"nonmatching").unwrap()),
    ];
    events = categorize(events, &cats);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().unwrap().data.get("$category").unwrap(),
        &serde_json::json!(vec!["Test", "Subtest"])
    );
}

#[test]
fn test_autotag() {
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let rules: Vec<(String, Regex)> = vec![
        ("test".into(), Regex::new(r"test").unwrap()),
        ("test-2".into(), Regex::new(r"test").unwrap()),
        ("nonmatching".into(), Regex::new(r"nonmatching").unwrap()),
    ];
    events = classify(events, &rules);

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

#[test]
fn test_classify() {
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let classes: Vec<(String, Regex)> = vec![
        ("#test-tag".into(), Regex::new(r"test").unwrap()),
        ("Test".into(), Regex::new(r"test").unwrap()),
        ("Test -> Subtest".into(), Regex::new(r"test").unwrap()),
        ("Other".into(), Regex::new(r"nonmatching").unwrap()),
    ];
    events = classify(events, &classes);

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
        3
    );
    assert_eq!(
        events.first().unwrap().data.get("$category").unwrap(),
        &serde_json::json!("Test -> Subtest")
    );
}

#[test]
fn test_classify_uncategorized() {
    // Checks that the category correctly becomes uncategorized when no category matches
    let mut e = Event::default();
    e.data
        .insert("test".into(), serde_json::json!("just a test"));

    let mut events = vec![e];
    let classes: Vec<(String, Regex)> = vec![(
        "Non-matching -> Test".into(),
        Regex::new(r"not going to match").unwrap(),
    )];
    events = classify(events, &classes);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().unwrap().data.get("$category").unwrap(),
        &serde_json::json!("Uncategorized")
    );
}
