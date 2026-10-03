use aw_models::Event;
use aw_transform::classify::categorize;
use aw_transform::merge_events_by_keys;
use chrono::Duration;
use serde_json::json;

#[test]
fn manual_category_wins_and_survives_preaggregation() {
    let mut first = Event::default();
    first.duration = Duration::seconds(10);
    first.data.insert("app".into(), json!("browser"));
    first
        .data
        .insert("$manual_category".into(), json!(["Work", "Client"]));
    let mut second = first.clone();
    second.duration = Duration::seconds(20);
    second
        .data
        .insert("$manual_category".into(), json!(["Uncategorized"]));
    let mut automatic = first.clone();
    automatic.duration = Duration::seconds(30);
    automatic.data.remove("$manual_category");
    let merged = merge_events_by_keys(vec![first, second, automatic], vec!["app".into()]);
    assert_eq!(
        merged.len(),
        3,
        "preaggregation must not erase manual categories"
    );
    let result = categorize(merged, &[]);
    assert_eq!(result[0].data["$category"], json!(["Work", "Client"]));
    assert_eq!(result[1].data["$category"], json!(["Uncategorized"]));
    assert_eq!(result[2].data["$category"], json!(["Uncategorized"]));
    assert_eq!(result[0].duration, Duration::seconds(10));
    assert_eq!(result[1].duration, Duration::seconds(20));
    assert_eq!(result[2].duration, Duration::seconds(30));
    // Repeated categorization must retain the override, not reapply rules.
    assert_eq!(categorize(result.clone(), &[]), result);
}

#[test]
fn invalid_manual_marker_does_not_override_rules() {
    for value in [
        json!([]),
        json!([""]),
        json!([" "]),
        json!("Work"),
        json!([7]),
    ] {
        let mut event = Event::default();
        event.data.insert("$manual_category".into(), value);
        let result = categorize(vec![event], &[]);
        assert_eq!(result[0].data["$category"], json!(["Uncategorized"]));
    }
}

#[test]
fn clipping_afk_filter_and_union_carry_source_annotations() {
    let start = "2026-01-01T10:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let mut manual = Event::default();
    manual.timestamp = start;
    manual.duration = Duration::seconds(30);
    manual
        .data
        .insert("$manual_category".into(), json!(["Work"]));
    let mut active = Event::default();
    active.timestamp = start + Duration::seconds(10);
    active.duration = Duration::seconds(10);
    let filtered =
        aw_transform::filter_period_intersect(vec![manual.clone()], vec![active.clone()]);
    let classified = categorize(filtered, &[]);
    assert_eq!(classified.len(), 1);
    assert_eq!(classified[0].duration, Duration::seconds(10));
    assert_eq!(classified[0].data["$category"], json!(["Work"]));
    // Stopwatch/first stream wins overlap; surviving manual prefixes and
    // suffixes keep their source category, never the overlapping stream's.
    let united = aw_transform::union_no_overlap(vec![active], vec![manual]);
    let classified = categorize(united, &[]);
    assert_eq!(classified.len(), 3);
    assert_eq!(classified[0].data["$category"], json!(["Work"]));
    assert_eq!(classified[1].data["$category"], json!(["Uncategorized"]));
    assert_eq!(classified[2].data["$category"], json!(["Work"]));
    assert_eq!(
        classified
            .iter()
            .map(|e| e.duration.num_seconds())
            .sum::<i64>(),
        30
    );
}

#[test]
fn legacy_chunking_does_not_erase_manual_category_boundaries() {
    let mut automatic = Event::default();
    automatic.duration = Duration::seconds(10);
    automatic.data.insert("app".into(), json!("editor"));
    let mut manual = automatic.clone();
    manual
        .data
        .insert("$manual_category".into(), json!(["Work"]));
    let result = aw_transform::chunk_events_by_key(vec![automatic, manual], "app");
    assert_eq!(result.len(), 2);
    assert_eq!(
        categorize(result, &[])[1].data["$category"],
        json!(["Work"])
    );
}

#[test]
fn merging_by_category_sums_manual_and_automatic_events() {
    // Once events are classified, the manual marker must not keep a manually
    // assigned event and an automatically classified event in the same
    // category apart: a category-totals merge (`merge_events_by_keys(["$category"])`)
    // has to return one summed row.
    let mut automatic = Event::default();
    automatic.duration = Duration::seconds(30);
    automatic.data.insert("app".into(), json!("browser"));
    let mut manual = automatic.clone();
    manual.duration = Duration::seconds(10);
    manual
        .data
        .insert("$manual_category".into(), json!(["Uncategorized"]));
    let classified = categorize(vec![automatic, manual], &[]);
    assert_eq!(classified[0].data["$category"], json!(["Uncategorized"]));
    assert_eq!(classified[1].data["$category"], json!(["Uncategorized"]));
    let merged = merge_events_by_keys(classified, vec!["$category".into()]);
    assert_eq!(
        merged.len(),
        1,
        "manual and automatic events in the same category must sum into one row"
    );
    assert_eq!(merged[0].duration, Duration::seconds(40));
}

#[test]
fn app_summary_keeps_manual_and_automatic_categories_apart() {
    // Desktop app/title summaries merge by app/title *after* categorization,
    // so their events carry `$category` and the merge is not grouping by
    // category. A manually assigned event must then not be folded into an
    // automatically classified one for the same app/title — that merge would
    // keep only the first event's category and misattribute the summed time.
    let mut automatic = Event::default();
    automatic.duration = Duration::seconds(30);
    automatic.data.insert("app".into(), json!("browser"));
    automatic.data.insert("title".into(), json!("page"));
    let mut manual = automatic.clone();
    manual.duration = Duration::seconds(10);
    manual
        .data
        .insert("$manual_category".into(), json!(["Work"]));
    let classified = categorize(vec![automatic, manual], &[]);
    assert_eq!(classified[0].data["$category"], json!(["Uncategorized"]));
    assert_eq!(classified[1].data["$category"], json!(["Work"]));
    let merged = merge_events_by_keys(classified, vec!["app".into()]);
    assert_eq!(
        merged.len(),
        2,
        "app summary must not fold events with different manual categories together"
    );
    assert_eq!(merged[0].duration, Duration::seconds(30));
    assert_eq!(merged[0].data["$category"], json!(["Uncategorized"]));
    assert_eq!(merged[1].duration, Duration::seconds(10));
    assert_eq!(merged[1].data["$category"], json!(["Work"]));
}
