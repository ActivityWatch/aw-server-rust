use aw_models::Event;
use aw_transform::classify::{tag, RegexRule, Rule};
use serde_json::json;

#[test]
fn rejects_oversized_compiled_rules_with_all_matching_options() {
    // Short source text can expand into a large compiled automaton.
    for ignore_case in [false, true] {
        for select_keys in [None, Some(vec!["title".to_owned()])] {
            assert!(
                RegexRule::new("a{200000}z", ignore_case, select_keys.clone()).is_err(),
                "accepted oversized regex: ignore_case={ignore_case}, select_keys={select_keys:?}"
            );
        }
    }
}

#[test]
fn ordinary_and_fancy_rules_still_compile_and_match() {
    for (pattern, text) in [
        ("^(editor|terminal)$", "terminal"),
        ("^foo(?=bar)", "foobar"),
        (r"^(\w+) \1$", "same same"),
    ] {
        for ignore_case in [false, true] {
            for select_keys in [None, Some(vec!["title".to_owned()])] {
                let rule = RegexRule::new(pattern, ignore_case, select_keys).unwrap();
                let mut event = Event::default();
                event.data.insert(
                    "title".into(),
                    json!(if ignore_case {
                        text.to_uppercase()
                    } else {
                        text.to_owned()
                    }),
                );
                let tagged = tag(vec![event], &[("matched".into(), Rule::Regex(rule))]);
                assert_eq!(tagged[0].data["$tags"], json!(["matched"]), "{pattern}");
            }
        }
    }
}
