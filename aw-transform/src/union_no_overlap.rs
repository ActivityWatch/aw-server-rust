use aw_models::{Event, TimeInterval};
use chrono::{DateTime, Utc};

/// Merges two eventlists and removes overlap, the first eventlist will have precedence
///
/// aw-core implementation: https://github.com/ActivityWatch/aw-core/blob/master/aw_transform/union_no_overlap.py
///
/// # Example
/// ```ignore
///   events1  | xxx    xx     xxx     |
///   events1  |  ----     ------   -- |
///   result   | xxx--  xx ----xxx  -- |
/// ```
pub fn union_no_overlap(events1: Vec<Event>, events2: Vec<Event>) -> Vec<Event> {
    let mut events_union: Vec<Event> = Vec::new();
    let mut events1 = events1.into_iter().peekable();
    let mut events2 = events2.into_iter();
    // Keep a split remainder here instead of inserting it ahead of the entire
    // unprocessed suffix. Each step consumes an input or emits a fragment.
    let mut pending = events2.next();
    while let (Some(e1), Some(e2)) = (events1.peek(), pending.as_ref()) {
        let e1_p: TimeInterval = e1.into();
        let e2_p: TimeInterval = e2.into();

        if e1_p.intersects(&e2_p) {
            if e1.timestamp <= e2.timestamp {
                let end = e1.timestamp + e1.duration;
                let e2_end = e2.timestamp + e2.duration;
                if end > e2_end {
                    // This foreground event may cover more background events.
                    // Keep it pending until all of those have been removed.
                    pending = events2.next();
                    continue;
                }
                if e2.timestamp < end && end < e2_end {
                    let remainder = pending.as_mut().unwrap();
                    remainder.timestamp = end;
                    remainder.duration = e2_end - end;
                    remainder.id = None;
                } else {
                    pending = events2.next();
                }
                events_union.push(events1.next().unwrap());
            } else {
                let (prefix, remainder) = split_event(pending.take().unwrap(), e1.timestamp);
                events_union.push(prefix);
                pending = remainder.or_else(|| events2.next());
            }
        } else if e1.timestamp <= e2.timestamp {
            events_union.push(events1.next().unwrap());
        } else {
            events_union.push(pending.take().unwrap());
            pending = events2.next();
        }
    }

    // Now we just need to add any remaining events
    events_union.extend(events1);
    events_union.extend(pending);
    events_union.extend(events2);

    events_union
}

fn split_event(mut e: Event, timestamp: DateTime<Utc>) -> (Event, Option<Event>) {
    if e.timestamp < timestamp && timestamp < e.timestamp + e.duration {
        let e1 = Event::new(e.timestamp, timestamp - e.timestamp, e.data.clone());
        e.duration -= timestamp - e.timestamp;
        e.timestamp = timestamp;
        e.id = None;
        (e1, Some(e))
    } else {
        (e, None)
    }
}

// Some tests
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn union_matches_discrete_coverage_for_all_small_inputs() {
        let now = Utc::now();
        let events = |mask: u32, source: &str| {
            let mut result = Vec::new();
            let mut start = 0;
            while start < 6 {
                if mask & (1 << start) == 0 {
                    start += 1;
                    continue;
                }
                let mut end = start + 1;
                while end < 6 && mask & (1 << end) != 0 {
                    end += 1;
                }
                let mut data = serde_json::Map::new();
                data.insert("source".into(), serde_json::json!(source));
                result.push(Event::new(
                    now + Duration::seconds(start),
                    Duration::seconds(end - start),
                    data,
                ));
                start = end;
            }
            result
        };
        for a in 0..64 {
            for b in 0..64 {
                let result = union_no_overlap(events(a, "a"), events(b, "b"));
                for slot in 0..6 {
                    let time = now + Duration::seconds(slot);
                    let covering: Vec<_> = result
                        .iter()
                        .filter(|e| e.timestamp <= time && time < e.timestamp + e.duration)
                        .collect();
                    let expected = if a & (1 << slot) != 0 {
                        Some("a")
                    } else if b & (1 << slot) != 0 {
                        Some("b")
                    } else {
                        None
                    };
                    assert_eq!(
                        covering.len(),
                        usize::from(expected.is_some()),
                        "a={a}, b={b}, slot={slot}"
                    );
                    if let Some(source) = expected {
                        assert_eq!(covering[0].data["source"], source);
                    }
                }
                assert!(result
                    .windows(2)
                    .all(|pair| pair[0].timestamp + pair[0].duration <= pair[1].timestamp));
            }
        }
    }

    #[test]
    fn repeated_splits_preserve_precedence_payloads_and_ids() {
        let now = Utc::now();
        let mut background = Event::new(now, Duration::seconds(12), serde_json::Map::new());
        background.id = Some(99);
        background
            .data
            .insert("source".into(), serde_json::json!("background"));
        let foreground: Vec<_> = [2, 5, 8]
            .into_iter()
            .map(|start| {
                let mut e = Event::new(
                    now + Duration::seconds(start),
                    Duration::seconds(1),
                    serde_json::Map::new(),
                );
                e.id = Some(start);
                e.data
                    .insert("source".into(), serde_json::json!("foreground"));
                e
            })
            .collect();
        let result = union_no_overlap(foreground, vec![background]);
        let expected = [
            (0, 2, None),
            (2, 1, Some(2)),
            (3, 2, None),
            (5, 1, Some(5)),
            (6, 2, None),
            (8, 1, Some(8)),
            (9, 3, None),
        ];
        assert_eq!(result.len(), expected.len());
        for (event, (start, duration, id)) in result.iter().zip(expected) {
            assert_eq!(event.timestamp, now + Duration::seconds(start));
            assert_eq!(event.duration, Duration::seconds(duration));
            assert_eq!(event.id, id);
            assert_eq!(
                event.data["source"],
                if id.is_some() {
                    "foreground"
                } else {
                    "background"
                }
            );
        }
    }

    #[test]
    fn empty_disjoint_and_zero_duration_events_keep_ids() {
        let now = Utc::now();
        let mut event = Event::new(now, Duration::zero(), serde_json::Map::new());
        event.id = Some(12);
        assert_eq!(
            union_no_overlap(vec![], vec![event.clone()])[0].id,
            Some(12)
        );
        assert_eq!(
            union_no_overlap(vec![event.clone()], vec![])[0].id,
            Some(12)
        );
        let mut later = event.clone();
        later.timestamp += Duration::seconds(1);
        later.id = Some(13);
        let result = union_no_overlap(vec![event], vec![later]);
        assert_eq!(
            result.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![Some(12), Some(13)]
        );
    }

    #[test]
    fn test_split_event() {
        let now = Utc::now();
        let td1h = Duration::hours(1);
        let e = Event {
            id: None,
            timestamp: now,
            duration: Duration::hours(2),
            data: serde_json::Map::new(),
        };
        let (e1, e2_opt) = split_event(e.clone(), now + td1h);
        assert_eq!(e1.timestamp, now);
        assert_eq!(e1.duration, td1h);

        let e2 = e2_opt.unwrap();
        assert_eq!(e2.timestamp, now + td1h);
        assert_eq!(e2.duration, td1h);

        // Now a test which does not lead to a split
        let (e1, e2_opt) = split_event(e, now);
        assert_eq!(e1.timestamp, now);
        assert_eq!(e1.duration, Duration::hours(2));
        assert!(e2_opt.is_none());
    }

    #[test]
    fn test_union_no_overlap() {
        // A test without any actual overlap
        let now = Utc::now();
        let td1h = Duration::hours(1);
        let e1 = Event::new(now, td1h, serde_json::Map::new());
        let e2 = Event::new(now + td1h, td1h, serde_json::Map::new());
        let events1 = vec![e1.clone()];
        let events2 = vec![e2.clone()];
        let events_union = union_no_overlap(events1, events2);

        assert_eq!(events_union.len(), 2);
        assert_eq!(events_union[0].timestamp, now);
        assert_eq!(events_union[0].duration, td1h);
        assert_eq!(events_union[1].timestamp, now + td1h);
        assert_eq!(events_union[1].duration, td1h);

        // Now do in reverse order
        let events1 = vec![e2];
        let events2 = vec![e1];
        let events_union = union_no_overlap(events1, events2);

        // Resulting order should be the same, since there is no overlap.
        assert_eq!(events_union.len(), 2);
        assert_eq!(events_union[0].timestamp, now);
        assert_eq!(events_union[0].duration, td1h);
        assert_eq!(events_union[1].timestamp, now + td1h);
        assert_eq!(events_union[1].duration, td1h);
    }

    #[test]
    fn test_union_no_overlap_with_overlap() {
        // A test where the events overlap
        let now = Utc::now();
        let td1h = Duration::hours(1);
        let e1 = Event::new(now, td1h, serde_json::Map::new());
        let e2 = Event::new(now, Duration::hours(2), serde_json::Map::new());
        let events1 = vec![e1];
        let events2 = vec![e2];
        let events_union = union_no_overlap(events1, events2);

        assert_eq!(events_union.len(), 2);
        assert_eq!(events_union[0].timestamp, now);
        assert_eq!(events_union[0].duration, td1h);
        assert_eq!(events_union[1].timestamp, now + td1h);
        assert_eq!(events_union[1].duration, td1h);

        // Now test the case where e2 starts before e1
        let e1 = Event::new(now + td1h, td1h, serde_json::Map::new());
        let e2 = Event::new(now, Duration::hours(2), serde_json::Map::new());
        let events1 = vec![e1];
        let events2 = vec![e2];
        let events_union = union_no_overlap(events1, events2);

        assert_eq!(events_union.len(), 2);
        assert_eq!(events_union[0].timestamp, now);
        assert_eq!(events_union[0].duration, td1h);
        assert_eq!(events_union[1].timestamp, now + td1h);
        assert_eq!(events_union[1].duration, td1h);
    }
}
