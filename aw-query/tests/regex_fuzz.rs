//! Property-based fuzz tests for regex compilation in aw-query.
//!
//! These tests verify that invalid regex patterns fed into the query language
//! (via categorize, tag, or filter_keyvals_regex) are rejected with
//! QueryError::RegexCompileError rather than panicking or returning a
//! server-side 500-equivalent error.

use aw_datastore::Datastore;
use aw_models::{Bucket, BucketMetadata, TimeInterval};
use aw_query::QueryError;
use proptest::prelude::*;
use serde_json::json;

static TIME_INTERVAL: &str = "1980-01-01T00:00:00Z/2080-01-02T00:00:00Z";

fn setup_datastore_with_bucket() -> Datastore {
    let ds = Datastore::new_in_memory(false);
    let bucket = Bucket {
        bid: None,
        id: "testid".to_string(),
        _type: "testtype".to_string(),
        client: "testclient".to_string(),
        hostname: "testhost".to_string(),
        created: Some(chrono::Utc::now()),
        data: json!({}).as_object().unwrap().clone(),
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    };
    ds.create_bucket(&bucket).unwrap();
    ds
}

fn setup_interval() -> TimeInterval {
    TimeInterval::new_from_string(TIME_INTERVAL).unwrap()
}

/// Strategy that generates regex strings known to be invalid for fancy_regex.
fn invalid_regex_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Unclosed groups and character classes
        Just("(".to_string()),
        Just("[".to_string()),
        Just("(?P<name".to_string()),
        Just("(?i".to_string()),
        Just("(?<".to_string()),
        Just("(?P<".to_string()),
        Just("[abc".to_string()),
        // Lone or incomplete escape sequences
        Just("\\".to_string()),
        Just("\\k".to_string()),
        Just("\\x".to_string()),
        Just("\\u".to_string()),
        Just("\\Q".to_string()),
        // Quantifiers without targets (these are rejected at parse time)
        Just("*".to_string()),
        Just("?".to_string()),
        Just("+".to_string()),
        Just("***".to_string()),
        Just("???".to_string()),
        Just("+++".to_string()),
        Just("**?".to_string()),
        // Unclosed alternation / group combinations
        Just("(a|".to_string()),
        Just("[a-z".to_string()),
        Just("(?i)[".to_string()),
    ]
}

/// Strategy that generates arbitrary strings (including potentially valid regex).
/// Used for the "must not panic" property.
fn arbitrary_regex_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(any::<char>(), 0..64).prop_map(|v| v.into_iter().collect())
}

/// Build a query that uses categorize() with the given regex pattern.
///
/// Only quotes are escaped; backslashes are passed through as-is because
/// the aw-query lexer only handles `\"` escapes, not `\\`.
fn categorize_query(pattern: &str) -> String {
    format!(
        r#"
        events = query_bucket("testid");
        events = categorize(events, [[["TestCat"], {{ "type": "regex", "regex": "{}" }}]]);
        return events;
        "#,
        pattern.replace('"', "\\\"")
    )
}

/// Build a query that uses tag() with the given regex pattern.
fn tag_query(pattern: &str) -> String {
    format!(
        r#"
        events = query_bucket("testid");
        events = tag(events, [["testtag", {{ "type": "regex", "regex": "{}" }}]]);
        return events;
        "#,
        pattern.replace('"', "\\\"")
    )
}

/// Build a query that uses filter_keyvals_regex() with the given regex pattern.
fn filter_keyvals_regex_query(pattern: &str) -> String {
    format!(
        r#"
        events = query_bucket("testid");
        events = filter_keyvals_regex(events, "key", "{}");
        return events;
        "#,
        pattern.replace('"', "\\\"")
    )
}

proptest! {
    /// Any invalid regex pattern passed to categorize() must produce
    /// QueryError::RegexCompileError, never panic.
    #[test]
    fn categorize_rejects_invalid_regex(pattern in invalid_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = categorize_query(&pattern);

        let result = aw_query::query(&code, &interval, &ds);
        prop_assert!(
            matches!(result, Err(QueryError::RegexCompileError(_))),
            "Expected RegexCompileError for pattern {:?}, got {:?}",
            pattern, result
        );
    }

    /// Any invalid regex pattern passed to tag() must produce
    /// QueryError::RegexCompileError, never panic.
    #[test]
    fn tag_rejects_invalid_regex(pattern in invalid_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = tag_query(&pattern);

        let result = aw_query::query(&code, &interval, &ds);
        prop_assert!(
            matches!(result, Err(QueryError::RegexCompileError(_))),
            "Expected RegexCompileError for pattern {:?}, got {:?}",
            pattern, result
        );
    }

    /// Any invalid regex pattern passed to filter_keyvals_regex() must produce
    /// QueryError::RegexCompileError, never panic.
    #[test]
    fn filter_keyvals_regex_rejects_invalid_regex(pattern in invalid_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = filter_keyvals_regex_query(&pattern);

        let result = aw_query::query(&code, &interval, &ds);
        prop_assert!(
            matches!(result, Err(QueryError::RegexCompileError(_))),
            "Expected RegexCompileError for pattern {:?}, got {:?}",
            pattern, result
        );
    }

    /// Arbitrary strings passed to categorize() must never panic.
    /// The result may be Ok or Err, but it must not crash.
    #[test]
    fn categorize_never_panics(pattern in arbitrary_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = categorize_query(&pattern);

        // We only assert that this does not panic; the result itself is allowed
        // to be either Ok or Err (some patterns are valid regex).
        let _result = aw_query::query(&code, &interval, &ds);
    }

    /// Arbitrary strings passed to tag() must never panic.
    #[test]
    fn tag_never_panics(pattern in arbitrary_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = tag_query(&pattern);

        let _result = aw_query::query(&code, &interval, &ds);
    }

    /// Arbitrary strings passed to filter_keyvals_regex() must never panic.
    #[test]
    fn filter_keyvals_regex_never_panics(pattern in arbitrary_regex_strategy()) {
        let ds = setup_datastore_with_bucket();
        let interval = setup_interval();
        let code = filter_keyvals_regex_query(&pattern);

        let _result = aw_query::query(&code, &interval, &ds);
    }
}

/// Regression test: the specific pattern that caused ActivityWatch#1340
/// (`++` in category pattern). In aw-server-rust this is a valid possessive
/// quantifier, so it should NOT produce RegexCompileError. The original bug
/// only affected the Python aw-server.
#[test]
fn regression_notepad_plus_plus_is_valid_in_rust() {
    let ds = setup_datastore_with_bucket();
    let interval = setup_interval();
    let code = categorize_query("Notepad++");

    let result = aw_query::query(&code, &interval, &ds);
    assert!(
        result.is_ok(),
        "Expected 'Notepad++' to be accepted as valid possessive quantifier, got {:?}",
        result
    );
}

/// Regression test: the literal-escaped form of the #1340 pattern must also work.
#[test]
fn regression_notepad_escaped_plus_is_valid() {
    let ds = setup_datastore_with_bucket();
    let interval = setup_interval();
    let code = categorize_query(r"Notepad\+\+");

    let result = aw_query::query(&code, &interval, &ds);
    assert!(
        result.is_ok(),
        "Expected 'Notepad\\+\\+' to be accepted, got {:?}",
        result
    );
}
