/// Transforms for classifying (tagging and categorizing) events.
///
/// Based on code in aw_research: https://github.com/ActivityWatch/aw-research/blob/master/aw_research/classify.py

use regex::Regex;
use serde_json;

use std::collections::HashSet;
use crate::models::Event;

pub fn classify(mut events: Vec<Event>, classes: &Vec<(String, Regex)>) -> Vec<Event> {
    let mut classified_events = Vec::new();
    for event in events.drain(..) {
        classified_events.push(classify_one(event, classes));
    }
    return classified_events;
}

fn classify_one(mut event: Event, classes: &Vec<(String, Regex)>) -> Event {
    let mut tags: HashSet<String> = HashSet::new();
    for (cls, re) in classes {
        for val in event.data.values() {
            // TODO: Recurse if value is object/array
            if val.is_string() && re.is_match(val.as_str().unwrap()) {
                tags.insert(cls.clone());
                break;
            }
        }
    }

    // An event can have many tags
    event.data.insert("$tags".into(), serde_json::json!(tags));

    // An event can only have one category, although the category may have a hierarchy,
    // for instance: "Work -> ActivityWatch -> aw-server-rust"
    // A category is chosed out of the tags used some rule (such as picking the one that's deepest in the hierarchy)
    let category = choose_category(&tags);
    event.data.insert("$category".into(), serde_json::json!(category));
    event
}

fn choose_category(tags: &HashSet<String>) -> String {
    tags.iter().fold(&"Uncategorized".to_string(), |acc: &String, item: &String| {
        if item.starts_with('#') {
            // If tag is not a category, then skip.
            acc
        } else if item.matches("->").count() >= acc.matches("->").count() {
            // If tag is category with greater or equal depth than current, then choose the new one instead.
            item
        } else {
            acc
        }
    }).clone()
}

#[test]
fn test_classify() {
    let mut e = Event::default();
    e.data.insert("test".into(), serde_json::json!("just a test"));

    let events = vec!(e);
    let classes: Vec<(String, Regex)> = vec!(
        ("#test-tag".into(), Regex::new(r"test").unwrap()),
        ("Test".into(), Regex::new(r"test").unwrap()),
        ("Test -> Subtest".into(), Regex::new(r"test").unwrap()),
        ("Other".into(), Regex::new(r"nonmatching").unwrap()),
    );
    let events_classified = classify(events, &classes);

    assert_eq!(events_classified.len(), 1);
    assert_eq!(events_classified.first().unwrap().data.get("$tags").unwrap().as_array().unwrap().len(), 3);
    assert_eq!(events_classified.first().unwrap().data.get("$category").unwrap(), &serde_json::json!("Test -> Subtest"));
}
