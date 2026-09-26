use aw_models::Event;
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
    let mut events2 = events2.into_iter().peekable();
    // Keep a split remainder here instead of inserting it ahead of the entire
    // unprocessed suffix. Each step consumes an input or emits a fragment.
    let mut pending = events2.next();
    while let (Some(e1), Some(e2)) = (events1.peek(), pending.as_ref()) {
        // Compare positions directly rather than via `TimeInterval::intersects`,
        // which is false for zero-duration events. A zero-duration e1 inside e2
        // must still split e2, or e2 is emitted whole and overlaps later e1 events.
        let e1_end = e1.timestamp + e1.duration;
        let e2_end = e2.timestamp + e2.duration;
        if e2.timestamp < e1.timestamp {
            // e2 starts first: emit the part before e1, keep the rest pending.
            let split_at = e1.timestamp;
            let (prefix, remainder) = split_event(pending.take().unwrap(), split_at);
            events_union.push(prefix);
            pending = match remainder {
                Some(remainder) => {
                    // Points before e1 lie inside the emitted prefix.
                    while let Some(point) = events2.next_if(|e| is_point_before(e, split_at)) {
                        events_union.push(point);
                    }
                    Some(remainder)
                }
                None => events2.next(),
            };
        } else if e2.timestamp < e1_end {
            // e1 starts first (or together) and covers the start of e2.
            if e2_end <= e1_end {
                // e2 is fully covered. Keep e1 pending, it may cover more events.
                pending = events2.next();
                continue;
            }
            let remainder = pending.as_mut().unwrap();
            remainder.timestamp = e1_end;
            remainder.duration = e2_end - e1_end;
            remainder.id = None;
            // Points before e1_end are covered by e1 and dropped.
            while events2.next_if(|e| is_point_before(e, e1_end)).is_some() {}
            events_union.push(events1.next().unwrap());
        } else {
            // e1 ends before (or where) e2 starts.
            events_union.push(events1.next().unwrap());
        }
    }

    // Now we just need to add any remaining events
    events_union.extend(events1);
    events_union.extend(pending);
    events_union.extend(events2);

    events_union
}

/// Zero-duration events2 events may sit inside an earlier events2 event. When
/// that event's start moves forward, points before the new start must be
/// handled right away, or they would be emitted after it, out of order.
fn is_point_before(e: &Event, t: DateTime<Utc>) -> bool {
    e.duration.is_zero() && e.timestamp < t
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

    /// Builds sorted, internally non-overlapping events from `(start, duration)`
    /// pairs (in seconds), tagging each with `source`.
    fn events_from(now: DateTime<Utc>, spans: &[(i64, i64)], source: &str) -> Vec<Event> {
        events_from_unit(now, spans, Duration::seconds(1), source)
    }

    fn events_from_unit(
        now: DateTime<Utc>,
        spans: &[(i64, i64)],
        unit: Duration,
        source: &str,
    ) -> Vec<Event> {
        spans
            .iter()
            .map(|&(start, duration)| {
                let mut data = serde_json::Map::new();
                data.insert("source".into(), serde_json::json!(source));
                Event::new(now + unit * start as i32, unit * duration as i32, data)
            })
            .collect()
    }

    fn spans(events: &[Event], now: DateTime<Utc>) -> Vec<(i64, i64, String)> {
        events
            .iter()
            .map(|e| {
                (
                    (e.timestamp - now).num_seconds(),
                    e.duration.num_seconds(),
                    e.data["source"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Checks the union invariants: output is sorted, events with a duration
    /// never overlap, every `events1` event survives unchanged, zero-duration
    /// `events2` events are kept exactly when no `events1` event covers them, and every
    /// second is covered by the result exactly when either input covers it.
    fn assert_union_invariants(events1: &[Event], events2: &[Event], result: &[Event]) {
        let covers = |events: &[Event], t: DateTime<Utc>| {
            events
                .iter()
                .filter(|e| e.timestamp <= t && t < e.timestamp + e.duration)
                .count()
        };
        assert!(
            result.windows(2).all(|w| w[0].timestamp <= w[1].timestamp),
            "result not sorted"
        );
        let with_duration: Vec<_> = result.iter().filter(|e| !e.duration.is_zero()).collect();
        assert!(
            with_duration
                .windows(2)
                .all(|w| w[0].timestamp + w[0].duration <= w[1].timestamp),
            "result has overlapping events"
        );
        for e in events1 {
            assert!(result.contains(e), "events1 event missing from result");
        }
        let points: Vec<_> = result
            .iter()
            .filter(|e| e.duration.is_zero() && !events1.contains(e))
            .collect();
        let expected_points: Vec<_> = events2
            .iter()
            .filter(|e| e.duration.is_zero() && covers(events1, e.timestamp) == 0)
            .collect();
        assert_eq!(points, expected_points, "wrong events2 points kept");
        // Coverage is constant between consecutive event boundaries, so checking
        // one point inside each gap between boundaries is exact at any resolution.
        let mut bounds: Vec<DateTime<Utc>> = events1
            .iter()
            .chain(events2)
            .chain(result)
            .flat_map(|e| [e.timestamp, e.timestamp + e.duration])
            .collect();
        bounds.sort();
        bounds.dedup();
        for w in bounds.windows(2) {
            let t = w[0] + (w[1] - w[0]) / 2;
            let expected = if covers(events1, t) > 0 {
                1
            } else {
                covers(events2, t).min(1)
            };
            assert_eq!(covers(result, t), expected, "wrong coverage at {t}");
        }
    }

    #[test]
    fn edge_cases_keep_union_free_of_overlap() {
        let now = Utc::now();
        // (events1, events2) as (start, duration) in seconds
        type Spans = &'static [(i64, i64)];
        let cases: &[(Spans, Spans)] = &[
            // identical ranges
            (&[(0, 10)], &[(0, 10)]),
            // events1 contains events2, and vice versa
            (&[(0, 10)], &[(2, 3)]),
            (&[(2, 3)], &[(0, 10)]),
            // one events1 event covering several events2 events, followed by more events1
            (&[(0, 10), (20, 5)], &[(1, 2), (4, 2), (8, 5)]),
            // one events2 event spanning several events1 events
            (&[(2, 1), (5, 1), (8, 1)], &[(0, 12)]),
            // adjacent / touching boundaries
            (&[(0, 5)], &[(5, 5)]),
            (&[(5, 5)], &[(0, 5)]),
            (&[(0, 5), (5, 5)], &[(0, 10)]),
            // zero-duration events inside, at the start and at the end of the other list's events
            (&[(5, 0), (7, 1)], &[(0, 10)]),
            (&[(0, 0), (2, 1)], &[(0, 10)]),
            (&[(10, 0)], &[(0, 10)]),
            (&[(0, 10)], &[(5, 0), (12, 1)]),
            (&[(3, 0), (3, 2)], &[(0, 10)]),
        ];
        for (a, b) in cases {
            let events1 = events_from(now, a, "a");
            let events2 = events_from(now, b, "b");
            let result = union_no_overlap(events1.clone(), events2.clone());
            assert_union_invariants(&events1, &events2, &result);
        }
    }

    #[test]
    fn zero_duration_event_does_not_let_background_event_through_whole() {
        // A zero-duration events1 event inside an events2 event used to take the
        // "no intersection" branch, emitting the whole events2 event, so the later
        // events1 event at 7s overlapped it.
        let now = Utc::now();
        let result = union_no_overlap(
            events_from(now, &[(5, 0), (7, 1)], "a"),
            events_from(now, &[(0, 10)], "b"),
        );
        assert_eq!(
            spans(&result, now),
            vec![
                (0, 5, "b".into()),
                (5, 0, "a".into()),
                (5, 2, "b".into()),
                (7, 1, "a".into()),
                (8, 2, "b".into()),
            ]
        );
    }

    #[test]
    fn chained_unions_match_single_pass_priority() {
        // Mirrors how aw-webui combines devices: events = union_no_overlap(events, host_i)
        let now = Utc::now();
        let hosts = [
            events_from(now, &[(0, 4), (10, 0), (12, 6)], "a"),
            events_from(now, &[(2, 5), (9, 4), (20, 2)], "b"),
            events_from(now, &[(0, 30)], "c"),
        ];
        let mut result = Vec::new();
        for host in hosts {
            let next = union_no_overlap(result.clone(), host.clone());
            assert_union_invariants(&result, &host, &next);
            result = next;
        }
        assert_eq!(
            spans(&result, now)
                .into_iter()
                .filter(|(_, d, _)| *d > 0)
                .collect::<Vec<_>>(),
            vec![
                (0, 4, "a".into()),
                (4, 3, "b".into()),
                (7, 2, "c".into()),
                (9, 1, "b".into()),
                (10, 2, "b".into()),
                (12, 6, "a".into()),
                (18, 2, "c".into()),
                (20, 2, "b".into()),
                (22, 8, "c".into()),
            ]
        );
    }

    #[test]
    fn random_inputs_keep_union_invariants() {
        // Small deterministic LCG so the test needs no extra dependency.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) % n) as i64
        };
        let now = Utc::now();
        for _ in 0..2_000 {
            let mut lists = Vec::new();
            for source in ["a", "b"] {
                let mut spans = Vec::new();
                // Millisecond units so boundaries fall on fractional seconds.
                let mut t = next(3_000);
                for _ in 0..next(6) {
                    let duration = if next(4) == 0 { 0 } else { 1 + next(5_000) };
                    spans.push((t, duration));
                    if duration > 0 && next(4) == 0 {
                        // Zero-duration point inside the event just added.
                        spans.push((t + next(duration as u64), 0));
                    }
                    t += duration + next(3_000);
                }
                lists.push(events_from_unit(
                    now,
                    &spans,
                    Duration::milliseconds(1),
                    source,
                ));
            }
            let result = union_no_overlap(lists[0].clone(), lists[1].clone());
            assert_union_invariants(&lists[0], &lists[1], &result);
        }
    }

    #[test]
    fn zero_duration_events2_points_yield_to_events1() {
        // events2 keeps only what events1 does not cover: a point strictly inside
        // or at the start of an events1 event is dropped, one at its end is kept.
        let now = Utc::now();
        type Expected = Vec<(i64, i64, String)>;
        let cases: &[(i64, Expected)] = &[
            (5, vec![(0, 10, "a".into())]),
            (0, vec![(0, 10, "a".into())]),
            (10, vec![(0, 10, "a".into()), (10, 0, "b".into())]),
        ];
        for (point, expected) in cases {
            let result = union_no_overlap(
                events_from(now, &[(0, 10)], "a"),
                events_from(now, &[(*point, 0)], "b"),
            );
            assert_eq!(&spans(&result, now), expected, "point at {point}s");
        }
    }

    #[test]
    fn fractional_second_boundaries_split_exactly() {
        let now = Utc::now();
        let ms = Duration::milliseconds(1);
        let result = union_no_overlap(
            events_from_unit(now, &[(1_250, 0), (1_500, 250)], ms, "a"),
            events_from_unit(now, &[(1_000, 1_000)], ms, "b"),
        );
        let got: Vec<_> = result
            .iter()
            .map(|e| {
                (
                    (e.timestamp - now).num_milliseconds(),
                    e.duration.num_milliseconds(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (1_000, 250),
                (1_250, 0),
                (1_250, 250),
                (1_500, 250),
                (1_750, 250)
            ]
        );
    }

    #[test]
    fn events2_points_inside_events2_stay_sorted() {
        let now = Utc::now();
        type Spans = &'static [(i64, i64)];
        type Expected = Vec<(i64, i64, String)>;
        let cases: &[(Spans, Spans, Expected)] = &[
            // point inside the part of an events2 event covered by events1: dropped
            (
                &[(0, 1)],
                &[(0, 3), (0, 0)],
                vec![(0, 1, "a".into()), (1, 2, "b".into())],
            ),
            // point in the part emitted before events1: kept, in order
            (
                &[(5, 1)],
                &[(0, 10), (2, 0)],
                vec![
                    (0, 5, "b".into()),
                    (2, 0, "b".into()),
                    (5, 1, "a".into()),
                    (6, 4, "b".into()),
                ],
            ),
            // point in the part after events1: kept, in order
            (
                &[(2, 1)],
                &[(0, 10), (7, 0)],
                vec![
                    (0, 2, "b".into()),
                    (2, 1, "a".into()),
                    (3, 7, "b".into()),
                    (7, 0, "b".into()),
                ],
            ),
        ];
        for (a, b, expected) in cases {
            let events1 = events_from(now, a, "a");
            let events2 = events_from(now, b, "b");
            let result = union_no_overlap(events1.clone(), events2.clone());
            assert_eq!(&spans(&result, now), expected);
            assert_union_invariants(&events1, &events2, &result);
        }
    }

    #[test]
    fn events1_event_containing_several_events2_events() {
        // Regression test for the containment bug fixed in #674 (still present in
        // v0.13.2 and v0.14.0b5/b6): once the first events1 event had been emitted,
        // the second contained events2 event (5-6 min) was emitted on top of it.
        let now = Utc::now();
        let min = Duration::minutes(1);
        let result = union_no_overlap(
            events_from_unit(now, &[(0, 10), (20, 10)], min, "A"),
            events_from_unit(now, &[(2, 1), (5, 1)], min, "b"),
        );
        assert_eq!(
            spans(&result, now),
            vec![(0, 600, "A".into()), (1200, 600, "A".into())]
        );
    }
}
