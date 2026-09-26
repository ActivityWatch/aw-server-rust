use std::cmp::{max, min};

use aw_models::Event;
use chrono::Duration;

/// Fills short gaps between events and merges nearby events with the same data.
///
/// This is a port of aw-core's `flood()` and must stay in sync with it, since the Python and Rust
/// servers are expected to return the same results for the same query:
///     https://github.com/ActivityWatch/aw-core/blob/master/aw_transform/flood.py
///
/// "Flooding" performs two similar, but slightly different tasks:
///  - Flooding: removes gaps of at most `pulsetime` between events with different data, by
///    extending both events to the middle of the gap.
///  - Merging: merges overlapping events, and events separated by a gap of at most `pulsetime`,
///    that have the same data.
///
/// Events are processed in (timestamp, duration) order. The output never contains overlapping
/// events: where events with different data overlap, the later event takes precedence and the
/// earlier one is cut off at its start (see ActivityWatch/activitywatch#1369). Zero-duration
/// events are dropped.
///
/// # Example
///
/// Example with forward-fill:
///
/// ```ignore
/// pulsetime: 1 second (one space)
/// input:  [a] [b] [c]
/// output: [a ][b ][c]
/// ```
///
/// Example with forward-fill and event merging:
///
/// ```ignore
/// pulsetime: 1 second (one space)
/// input:  [a] [a] [b ][b]
/// output: [a    ] [b    ]
/// ```
pub fn flood(events: Vec<Event>, pulsetime: Duration) -> Vec<Event> {
    let mut events = events;
    // Sort by (timestamp, duration) so shorter same-timestamp events come first.
    events.sort_by_key(|e| (e.timestamp, e.duration));

    // If negative gaps are smaller than this, prune them to become zero
    let negative_gap_trim_thres = Duration::milliseconds(100);
    let zero = Duration::zero();

    let mut warned_negative_gap_safe = false;
    let mut warned_negative_gap_unsafe = false;

    // Pairwise pass: each event is compared with the next one, which may be modified before it is
    // compared with its own successor.
    for i in 1..events.len() {
        let (head, tail) = events.split_at_mut(i);
        let e1 = &mut head[i - 1];
        let e2 = &mut tail[0];

        let e1_end = e1.calculate_endtime();
        let e2_end = e2.calculate_endtime();
        let gap = e2.timestamp - e1_end;

        if gap < zero && e1.data == e2.data {
            // Events with negative gap but same data can safely be merged
            if !warned_negative_gap_safe {
                warn!("Gap was of negative duration ({}s), but could be safely merged. This warning will only show once per batch.", gap);
                warned_negative_gap_safe = true;
            }
            let start = min(e1.timestamp, e2.timestamp);
            let end = max(e1_end, e2_end);
            e1.timestamp = start;
            e1.duration = end - start;
            e2.timestamp = end;
            e2.duration = zero;
        } else if gap < -negative_gap_trim_thres {
            // Events with negative gap but differing data cannot be merged here, they are
            // resolved by the normalization pass below.
            if !warned_negative_gap_unsafe {
                warn!("Gap was of negative duration and could NOT be safely merged ({}s). This warning will only show once per batch.", gap);
                warned_negative_gap_unsafe = true;
            }
        } else if gap > -negative_gap_trim_thres && gap <= pulsetime {
            if e1.data == e2.data {
                // Keep the longer event and extend it over the gap and the other event.
                if e1.duration >= e2.duration {
                    e1.duration = e2_end - e1.timestamp;
                    e2.timestamp = e2_end;
                    e2.duration = zero;
                } else {
                    e2.timestamp = e1.timestamp;
                    e2.duration = e2_end - e2.timestamp;
                    e1.duration = zero;
                }
            } else {
                // The gap is an interval of uncertainty: without evidence that either neighbour
                // owns more of it, split it at the midpoint.
                let midpoint = e1_end + gap / 2;
                e1.duration = midpoint - e1.timestamp;
                e2.timestamp = midpoint;
                e2.duration = e2_end - midpoint;
            }
        }
        // else: nothing to do, events not near each other
    }

    // The pairwise pass can modify an event after it was compared with its predecessor, and does
    // not resolve overlapping events with different data. Normalize the result so that it never
    // contains overlapping events, and adjacent same-data events are merged. For differing data,
    // the later event wins.
    let mut normalized: Vec<Event> = Vec::with_capacity(events.len());
    for event in events.into_iter().filter(|e| e.duration > zero) {
        let mut merged = false;
        while let Some(previous) = normalized.last_mut() {
            let previous_end = previous.calculate_endtime();
            let same_data = previous.data == event.data;
            // Merge same-data events that touch, not only those that overlap: the pairwise pass
            // leaves a merged-away event as a zero-duration placeholder at the end of the merged
            // event, so a chain of same-data events can come out as adjacent pieces.
            if previous_end < event.timestamp || (previous_end == event.timestamp && !same_data) {
                break;
            }
            if same_data {
                let end = max(previous_end, event.calculate_endtime());
                previous.duration = end - previous.timestamp;
                merged = true;
                break;
            }
            previous.duration = event.timestamp - previous.timestamp;
            if previous.duration > zero {
                break;
            }
            normalized.pop();
        }
        if !merged {
            normalized.push(event);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use super::flood;

    #[test]
    fn test_flood_merge() {
        // Test merging of events with the same data
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let e_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(4),
            data: json_map! {"test": json!(1)},
        };
        let res = flood(vec![e1, e2], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e_expected);
    }

    #[test]
    fn test_flood_meet_in_middle() {
        // Test flood gap between two different events which should meet in the middle
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(2)},
        };
        let e1_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(2),
            data: json_map! {"test": json!(1)},
        };
        let e2_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
            duration: Duration::seconds(2),
            data: json_map! {"test": json!(2)},
        };
        let res = flood(vec![e1, e2], Duration::seconds(5));
        assert_eq!(2, res.len());
        assert_eq!(&res[0], &e1_expected);
        assert_eq!(&res[1], &e2_expected);
    }

    #[test]
    fn test_flood_partial_overlap() {
        // Tests flooding an identical event contained within another event
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(10),
            data: json_map! {"type": "a"},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:05Z").unwrap(),
            duration: Duration::seconds(10),
            data: json_map! {"type": "a"},
        };
        let e1_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(15),
            data: json_map! {"type": "a"},
        };
        let res = flood(vec![e1, e2], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e1_expected);
    }

    #[test]
    fn test_flood_containing() {
        // Tests flooding an identical event contained within another event
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(10),
            data: json_map! {"type": "a"},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(5),
            data: json_map! {"type": "a"},
        };
        let res = flood(vec![e1.clone(), e2], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e1);
    }

    #[test]
    fn test_flood_containing_diff() {
        // An event with different data contained within another event takes precedence from its
        // start, so that the result has no overlap (ActivityWatch/activitywatch#1369).
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(10),
            data: json_map! {"type": "a"},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(5),
            data: json_map! {"type": "b"},
        };
        let res = flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(2, res.len());
        let mut e1_expected = e1;
        e1_expected.duration = Duration::seconds(1);
        assert_eq!(&res[0], &e1_expected);
        assert_eq!(&res[1], &e2);
    }

    #[test]
    fn test_flood_same_timestamp() {
        // e1, stay same
        // e2, base merge (longest duration, this should be the duration selected)
        // e3, merge with e2
        // e4, stay same
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "afk"},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(5),
            data: json_map! {"status": "not-afk"},
        };
        let e3 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "not-afk"},
        };
        let e4 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:06Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "afk"},
        };
        let res = flood(
            vec![e1.clone(), e2.clone(), e3, e4.clone()],
            Duration::seconds(5),
        );
        assert_eq!(3, res.len());
        assert_eq!(&res[0], &e1);
        assert_eq!(&res[1], &e2);
        assert_eq!(&res[2], &e4);
    }

    #[test]
    fn test_flood_same_timestamp_duplicates() {
        // e1, stay same
        // e2, base merge
        // e3, merge with e2
        // e4, merge with e2 (longest duration, this should be the duration selected)
        // e5, stay same
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "afk"},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(5),
            data: json_map! {"status": "not-afk"},
        };
        let e3 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "not-afk"},
        };
        let e4 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(10),
            data: json_map! {"status": "not-afk"},
        };
        let e5 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:11Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"status": "afk"},
        };
        let res = flood(
            vec![e1.clone(), e2, e3, e4.clone(), e5.clone()],
            Duration::seconds(5),
        );
        assert_eq!(3, res.len());
        assert_eq!(&res[0], &e1);
        assert_eq!(&res[1], &e4);
        assert_eq!(&res[2], &e5);
    }

    /// Event starting `start` seconds after 10:00:00Z, lasting `duration` seconds, with data
    /// `{"app": app}`.
    fn ev(start: f64, duration: f64, app: &str) -> Event {
        let base: DateTime<chrono::Utc> = DateTime::from_str("2026-01-01T10:00:00Z").unwrap();
        Event {
            id: None,
            timestamp: base + Duration::milliseconds((start * 1000.0) as i64),
            duration: Duration::milliseconds((duration * 1000.0) as i64),
            data: json_map! {"app": json!(app)},
        }
    }

    fn assert_no_overlap(events: &[Event]) {
        for pair in events.windows(2) {
            assert!(
                pair[0].calculate_endtime() <= pair[1].timestamp,
                "{:?} overlaps {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    // The following tests are the repros from ActivityWatch/aw-server-rust#746, where Rust and
    // aw-core disagreed. Expected results are aw-core's.

    #[test]
    fn test_flood_overlap_diff_data() {
        // activitywatch#1369: overlapping events with different data must not count time twice
        let res = flood(
            vec![ev(0., 10., "x"), ev(9., 10., "y")],
            Duration::seconds(5),
        );
        assert_eq!(res, vec![ev(0., 9., "x"), ev(9., 10., "y")]);
    }

    #[test]
    fn test_flood_drops_zero_duration() {
        let res = flood(
            vec![ev(0., 0., "x"), ev(60., 10., "y")],
            Duration::seconds(5),
        );
        assert_eq!(res, vec![ev(60., 10., "y")]);
    }

    #[test]
    fn test_flood_gap_equal_to_pulsetime() {
        // A gap of exactly pulsetime is filled, meeting in the middle
        let res = flood(
            vec![ev(0., 10., "x"), ev(15., 10., "y")],
            Duration::seconds(5),
        );
        assert_eq!(res, vec![ev(0., 12.5, "x"), ev(12.5, 12.5, "y")]);

        // A gap just above pulsetime is left alone
        let events = vec![ev(0., 10., "x"), ev(15.001, 10., "y")];
        assert_eq!(flood(events.clone(), Duration::seconds(5)), events);
    }

    #[test]
    fn test_flood_adjacent_same_data() {
        let res = flood(
            vec![ev(0., 10., "x"), ev(10., 10., "x")],
            Duration::seconds(5),
        );
        assert_eq!(res, vec![ev(0., 20., "x")]);
    }

    #[test]
    fn test_flood_merge_chains() {
        // Chains of same-data events merge into one event, whether adjacent or within pulsetime
        let pt = Duration::seconds(5);
        let adjacent = vec![ev(0., 10., "x"), ev(10., 10., "x"), ev(20., 10., "x")];
        assert_eq!(flood(adjacent, pt), vec![ev(0., 30., "x")]);

        let gaps = vec![ev(0., 10., "x"), ev(12., 1., "x"), ev(14., 1., "x")];
        assert_eq!(flood(gaps, pt), vec![ev(0., 15., "x")]);

        let then_other = vec![ev(0., 10., "x"), ev(12., 1., "x"), ev(14., 1., "y")];
        assert_eq!(
            flood(then_other, pt),
            vec![ev(0., 13.5, "x"), ev(13.5, 1.5, "y")]
        );
    }

    #[test]
    fn test_flood_merges_adjacent_randomized() {
        let mut seed: u64 = 0x6a09_e667_f3bc_c908;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for _ in 0..500 {
            let n = next(12) as usize;
            let events: Vec<Event> = (0..n)
                .map(|_| {
                    let app = ["a", "b"][next(2) as usize];
                    ev(next(60) as f64, next(15) as f64, app)
                })
                .collect();
            let once = flood(events.clone(), Duration::seconds(5));
            // No two adjacent same-data events are left unmerged
            for pair in once.windows(2) {
                assert!(
                    pair[0].data != pair[1].data || pair[0].calculate_endtime() < pair[1].timestamp,
                    "{events:?} -> {once:?}"
                );
            }
        }
    }

    #[test]
    fn test_flood_pulsetime() {
        let events = vec![ev(0., 10., "x"), ev(20., 10., "y")];
        assert_eq!(flood(events.clone(), Duration::seconds(5)), events);
        assert_eq!(
            flood(events, Duration::seconds(10)),
            vec![ev(0., 15., "x"), ev(15., 15., "y")]
        );
    }

    #[test]
    fn test_flood_small_negative_gap_diff_data() {
        // Overlaps smaller than 100ms between differing events are split in the middle
        let res = flood(
            vec![ev(0., 10., "x"), ev(9.95, 10., "y")],
            Duration::seconds(5),
        );
        assert_eq!(res, vec![ev(0., 9.975, "x"), ev(9.975, 9.975, "y")]);
    }

    #[test]
    fn test_flood_later_event_wins() {
        // A later event overlapping several earlier ones truncates or removes them all
        let res = flood(
            vec![ev(0., 10., "x"), ev(2., 10., "y"), ev(2., 20., "z")],
            Duration::seconds(5),
        );
        assert_no_overlap(&res);
        assert_eq!(res, vec![ev(0., 2., "x"), ev(2., 20., "z")]);
    }

    #[test]
    fn test_flood_no_overlap_randomized() {
        // Deterministic pseudo-random inputs; the output must be sorted and free of overlap.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for _ in 0..500 {
            let n = next(12) as usize;
            let events: Vec<Event> = (0..n)
                .map(|_| {
                    let app = ["a", "b", "c"][next(3) as usize];
                    ev(next(60) as f64, next(15) as f64, app)
                })
                .collect();
            let res = flood(events, Duration::seconds(5));
            assert_no_overlap(&res);
            assert!(res.iter().all(|e| e.duration > Duration::zero()));
        }
    }
}
