use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use chrono::{DateTime, Utc};

use aw_models::Event;

/// Split each event in `base_events` on overlapping subwatcher boundaries and
/// copy the named `keys` from the matching subwatcher event into each
/// segment's `data`.
///
/// Port of aw-core's `aw_transform.merge_subwatcher_fields`. The output covers
/// exactly the same time as `base_events`, so app/title/duration aggregations
/// stay correct (unlike the `concat` workaround); only the segmentation changes
/// where subwatcher fields actually change.
///
/// This lets clients categorize by subwatcher fields (browser `url`/`$domain`;
/// editor `project`/`file`/`language`) without per-watcher client code.
///
/// * Pass the original `subwatcher_events`; boundaries are clipped here.
///   `filter_period_intersect` can discard newer overlapping subwatcher events.
/// * `conflict` is `"base_wins"` (existing base keys are never overwritten) or
///   `"sub_wins"` (subwatcher fields overwrite base fields).
///
/// If several subwatcher events cover the same slice, the one with the latest
/// start wins (ties: the one ending last), so a later transition is not
/// smeared backward by an older, longer pulse. Adjacent segments with equal
/// data are collapsed.
pub fn merge_subwatcher_fields(
    base_events: Vec<Event>,
    subwatcher_events: Vec<Event>,
    keys: &[String],
    conflict: &str,
) -> Result<Vec<Event>, String> {
    if conflict != "base_wins" && conflict != "sub_wins" {
        return Err(format!(
            "conflict must be 'base_wins' or 'sub_wins', got {conflict:?}"
        ));
    }
    if subwatcher_events.is_empty() || keys.is_empty() {
        return Ok(base_events);
    }

    let mut sub_sorted: Vec<(&Event, DateTime<Utc>)> = subwatcher_events
        .iter()
        .map(|e| (e, e.calculate_endtime()))
        .collect();
    sub_sorted.sort_by_key(|(e, _)| e.timestamp);

    // Sweep in start order so expired subwatcher events are removed once.
    // Keep per-base output slots to restore the caller's original order.
    let mut bases: Vec<_> = base_events.into_iter().enumerate().collect();
    bases.sort_by_key(|(_, base)| base.timestamp);
    let mut by_base = vec![Vec::new(); bases.len()];
    let mut next_sub = 0;
    let mut active = BTreeSet::new();
    let mut endings = BinaryHeap::new();
    for (base_index, base) in bases {
        let base_start = base.timestamp;
        let base_end = base.calculate_endtime();

        while next_sub < sub_sorted.len() && sub_sorted[next_sub].0.timestamp <= base_start {
            active.insert(next_sub);
            endings.push(Reverse((sub_sorted[next_sub].1, next_sub)));
            next_sub += 1;
        }
        while let Some(&Reverse((end, index))) = endings.peek() {
            if end > base_start {
                break;
            }
            endings.pop();
            active.remove(&index);
        }

        let mut overlapping: Vec<(&Event, DateTime<Utc>)> = Vec::new();
        let mut boundaries = vec![base_start, base_end];
        let candidates = active.iter().map(|&i| &sub_sorted[i]).chain(
            sub_sorted[next_sub..]
                .iter()
                .take_while(|(sub, _)| sub.timestamp < base_end),
        );
        for &(sub, sub_end) in candidates {
            if sub_end <= base_start {
                continue;
            }
            let start = sub.timestamp.max(base_start);
            let end = sub_end.min(base_end);
            if start < end {
                overlapping.push((sub, sub_end));
                boundaries.push(start);
                boundaries.push(end);
            }
        }

        if overlapping.is_empty() {
            by_base[base_index].push(base);
            continue;
        }

        boundaries.sort();
        boundaries.dedup();

        let mut segments: Vec<Event> = Vec::new();
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);

            let mut best: Option<(&Event, DateTime<Utc>)> = None;
            for &(sub, sub_end) in &overlapping {
                if sub.timestamp.max(start) >= sub_end.min(end) {
                    continue;
                }
                let newer = match best {
                    None => true,
                    Some((b, b_end)) => {
                        sub.timestamp > b.timestamp
                            || (sub.timestamp == b.timestamp && sub_end > b_end)
                    }
                };
                if newer {
                    best = Some((sub, sub_end));
                }
            }

            let mut enriched = base.clone();
            enriched.timestamp = start;
            enriched.duration = end - start;
            if let Some((sub, _)) = best {
                for key in keys {
                    if let Some(value) = sub.data.get(key) {
                        if conflict == "base_wins" && enriched.data.contains_key(key) {
                            continue;
                        }
                        enriched.data.insert(key.clone(), value.clone());
                    }
                }
            }

            match segments.last_mut() {
                Some(prev)
                    if prev.calculate_endtime() == enriched.timestamp
                        && prev.data == enriched.data =>
                {
                    prev.duration += enriched.duration;
                }
                _ => segments.push(enriched),
            }
        }
        by_base[base_index] = segments;
    }

    Ok(by_base.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::{DateTime, Duration, Utc};
    use serde_json::json;

    use aw_models::Event;

    use super::merge_subwatcher_fields;

    fn now() -> DateTime<Utc> {
        DateTime::from_str("2024-01-01T12:00:00Z").unwrap()
    }

    fn ev(start: Duration, duration: Duration, data: serde_json::Value) -> Event {
        Event {
            id: None,
            timestamp: now() + start,
            duration,
            data: data.as_object().unwrap().clone(),
        }
    }

    fn keys(ks: &[&str]) -> Vec<String> {
        ks.iter().map(|k| k.to_string()).collect()
    }

    fn total(events: &[Event]) -> Duration {
        events
            .iter()
            .fold(Duration::zero(), |acc, e| acc + e.duration)
    }

    fn sum_where(events: &[Event], key: &str, value: Option<&str>) -> Duration {
        events
            .iter()
            .filter(|e| e.data.get(key).and_then(|v| v.as_str()) == value)
            .fold(Duration::zero(), |acc, e| acc + e.duration)
    }

    fn m(n: i64) -> Duration {
        Duration::minutes(n)
    }

    #[test]
    fn test_basic() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim", "title": "file.py"}))];
        let sub = vec![ev(
            m(0),
            m(60),
            json!({"project": "myproject", "file": "file.py", "language": "python"}),
        )];
        let r = merge_subwatcher_fields(
            base,
            sub,
            &keys(&["project", "file", "language"]),
            "base_wins",
        )
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].data["app"], json!("vim"));
        assert_eq!(r[0].data["title"], json!("file.py"));
        assert_eq!(r[0].data["project"], json!("myproject"));
        assert_eq!(r[0].data["language"], json!("python"));
        assert_eq!(r[0].timestamp, now());
        assert_eq!(r[0].duration, m(60));
    }

    #[test]
    fn test_partial_overlap_splits_base() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![ev(m(15), m(30), json!({"project": "myproject"}))];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(r.len(), 3);
        let durations: Vec<_> = r.iter().map(|e| e.duration).collect();
        assert_eq!(durations, vec![m(15), m(30), m(15)]);
        let starts: Vec<_> = r.iter().map(|e| e.timestamp).collect();
        assert_eq!(starts, vec![now(), now() + m(15), now() + m(45)]);
        assert!(!r[0].data.contains_key("project"));
        assert_eq!(r[1].data["project"], json!("myproject"));
        assert!(!r[2].data.contains_key("project"));
        assert_eq!(total(&r), m(60));
    }

    #[test]
    fn test_overlapping_subwatchers_prefer_latest_start() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![
            ev(m(0), m(40), json!({"project": "alpha"})),
            ev(m(20), m(40), json!({"project": "beta"})),
        ];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].timestamp, now());
        assert_eq!(r[1].timestamp, now() + m(20));
        assert_eq!(r[0].duration, m(20));
        assert_eq!(r[1].duration, m(40));
        assert_eq!(r[0].data["project"], json!("alpha"));
        assert_eq!(r[1].data["project"], json!("beta"));
        assert_eq!(total(&r), m(60));
    }

    #[test]
    fn test_no_overlap() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![ev(m(120), m(60), json!({"project": "other"}))];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(r.len(), 1);
        assert!(!r[0].data.contains_key("project"));
        assert_eq!(r[0].data["app"], json!("vim"));
    }

    #[test]
    fn test_base_wins_conflict() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim", "file": "base.py"}))];
        let sub = vec![ev(m(0), m(60), json!({"file": "sub.py", "project": "p"}))];
        let r =
            merge_subwatcher_fields(base, sub, &keys(&["file", "project"]), "base_wins").unwrap();
        assert_eq!(r[0].data["file"], json!("base.py"));
        assert_eq!(r[0].data["project"], json!("p"));
    }

    #[test]
    fn test_sub_wins_conflict() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim", "file": "base.py"}))];
        let sub = vec![ev(m(0), m(60), json!({"file": "sub.py"}))];
        let r = merge_subwatcher_fields(base, sub, &keys(&["file"]), "sub_wins").unwrap();
        assert_eq!(r[0].data["file"], json!("sub.py"));
    }

    #[test]
    fn test_multiple_subsegments_preserve_duration() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![
            ev(m(0), m(15), json!({"project": "alpha"})),
            ev(m(15), m(15), json!({"project": "beta"})),
            ev(m(30), m(15), json!({"project": "alpha"})),
        ];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(sum_where(&r, "project", Some("alpha")), m(30));
        assert_eq!(sum_where(&r, "project", Some("beta")), m(15));
        assert_eq!(sum_where(&r, "project", None), m(15));
        assert_eq!(sum_where(&r, "app", Some("vim")), m(60));
        assert_eq!(total(&r), m(60));
    }

    #[test]
    fn test_empty_inputs() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![ev(m(0), m(60), json!({"project": "p"}))];

        let r = merge_subwatcher_fields(base.clone(), vec![], &keys(&["project"]), "base_wins")
            .unwrap();
        assert_eq!(r[0].data, base[0].data);

        let r = merge_subwatcher_fields(base.clone(), sub, &[], "base_wins").unwrap();
        assert!(!r[0].data.contains_key("project"));

        let r = merge_subwatcher_fields(base.clone(), vec![], &[], "base_wins").unwrap();
        assert_eq!(r[0].data, base[0].data);
    }

    #[test]
    fn test_large_nonoverlapping_streams() {
        let base: Vec<_> = (0..50_000)
            .map(|i| ev(m(i), m(1), json!({"app": "vim"})))
            .collect();
        let sub: Vec<_> = (0..50_000)
            .map(|i| ev(m(i), m(1), json!({"project": i})))
            .collect();
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(r.len(), 50_000);
        for (i, event) in r.iter().enumerate() {
            assert_eq!(event.data["project"], json!(i));
            assert_eq!(event.duration, m(1));
        }
    }

    #[test]
    fn test_nested_overlap_clips_without_prefilter() {
        let base = vec![ev(m(10), m(10), json!({"app": "vim"}))];
        let sub = vec![
            ev(m(0), m(30), json!({"project": "older"})),
            ev(m(5), m(10), json!({"project": "newer"})),
        ];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].data["project"], json!("newer"));
        assert_eq!(r[1].data["project"], json!("older"));
        assert_eq!(r[0].duration, m(5));
        assert_eq!(r[1].duration, m(5));
        assert_eq!(total(&r), m(10));
    }

    #[test]
    fn test_unsorted_overlapping_bases_keep_input_order() {
        let base = vec![
            ev(m(20), m(10), json!({"app": "late"})),
            ev(m(0), m(30), json!({"app": "long"})),
            ev(m(10), m(10), json!({"app": "middle"})),
        ];
        let sub = vec![
            ev(m(20), m(10), json!({"project": "c"})),
            ev(m(0), m(10), json!({"project": "a"})),
            ev(m(10), m(10), json!({"project": "b"})),
        ];
        let r = merge_subwatcher_fields(base, sub, &keys(&["project"]), "base_wins").unwrap();
        let values: Vec<_> = r
            .iter()
            .map(|e| (e.data["app"].clone(), e.data["project"].clone()))
            .collect();
        assert_eq!(
            values,
            vec![
                (json!("late"), json!("c")),
                (json!("long"), json!("a")),
                (json!("long"), json!("b")),
                (json!("long"), json!("c")),
                (json!("middle"), json!("b")),
            ]
        );
        assert_eq!(total(&r), m(50));
    }

    #[test]
    fn test_invalid_conflict() {
        let base = vec![ev(m(0), m(60), json!({"app": "vim"}))];
        let sub = vec![ev(m(0), m(60), json!({"project": "p"}))];
        let err = merge_subwatcher_fields(base, sub, &keys(&["project"]), "invalid").unwrap_err();
        assert!(err.contains("conflict must be"));
    }
}
