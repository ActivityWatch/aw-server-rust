use aw_models::Event;

use crate::sort_by_timestamp;

/// Floods event to the nearest neighbouring event if within the specified pulsetime
///
/// Also merges events if they have the same data and are within the pulsetime
///
/// Python implementation:
///     https://github.com/ActivityWatch/aw-core/blob/master/aw_transform/flood.py
/// Python re-implementation using generators (different spec):
///     https://github.com/ErikBjare/copilot-testing/blob/master/playground/flooding.py
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
pub fn flood(events: Vec<Event>, pulsetime: chrono::Duration) -> Vec<Event> {
    let mut new_events = Vec::new();
    let mut events_sorted = sort_by_timestamp(events);
    let mut e1_iter = events_sorted.drain(..).peekable();

    let mut gap_prev: Option<chrono::Duration> = None;
    let mut retry_e: Option<Event> = None;

    // If negative gaps are smaller than this, prune them to become zero
    let negative_gap_trim_thres = chrono::Duration::milliseconds(100);

    let mut warned_negative_gap_safe = false;
    let mut warned_negative_gap_unsafe = false;

    while let Some(mut e1) = match retry_e {
        Some(e) => {
            retry_e = None;
            Some(e)
        }
        None => e1_iter.next(),
    } {
        if let Some(gap) = gap_prev {
            e1.timestamp = e1.timestamp - (gap / 2);
            e1.duration = e1.duration + (gap / 2);
            gap_prev = None;
        }
        let e2 = match e1_iter.peek() {
            Some(e) => e,
            None => {
                new_events.push(e1);
                break;
            }
        };

        let gap = e2.timestamp - e1.calculate_endtime();

        // Python implementation:
        //
        // if gap < timedelta(0) and e1.data == e2.data:
        //     start = min(e1.timestamp, e2.timestamp)
        //     end = max(e1.timestamp + e1.duration, e2.timestamp + e2.duration)
        //     e1.timestamp, e1.duration = start, (end - start)
        //     e2.timestamp, e2.duration = end, timedelta(0)
        //     if not warned_about_negative_gap_safe:
        //         logger.warning(
        //             "Gap was of negative duration but could be safely merged ({}s). This message will only show once per batch.".format(
        //                 gap.total_seconds()
        //             )
        //         )
        //         warned_about_negative_gap_safe = True

        // Sanity check in case events overlap
        if gap < chrono::Duration::seconds(0) && e1.data == e2.data {
            // Gap was negative and could be safely merged
            if !warned_negative_gap_safe {
                warn!("Gap was of negative duration ({}s), but could be safely merged. This error will only show once per batch.", gap);
                warned_negative_gap_safe = true;
            }
            let start = std::cmp::min(e1.timestamp, e2.timestamp);
            let end = std::cmp::max(e1.timestamp + e1.duration, e2.timestamp + e2.duration);
            e1.timestamp = start;
            e1.duration = end - start;
            // Drop next event since they are merged and flooded into e1
            e1_iter.next();
            // Retry this event again to give it a change to merge e1
            // with 'e3'
            retry_e = Some(e1);
            continue;
        }
        // Python implementation:
        //
        // elif gap < -negative_gap_trim_thres and not warned_about_negative_gap_unsafe:
        //     # Events with negative gap but differing data cannot be merged safely
        //     logger.warning(
        //         "Gap was of negative duration and could NOT be safely merged ({}s). This warning will only show once per batch.".format(
        //             gap.total_seconds()
        //         )
        //     )
        //     warned_about_negative_gap_unsafe = True

        // If gap is negative beyond the trim thres, throw a warning
        else if gap < -negative_gap_trim_thres && !warned_negative_gap_unsafe {
            // Events with negative gap but differing data cannot be merged safely
            warn!("Gap was of negative duration and could NOT be safely merged ({}s). This warning will only show once per batch.", gap);
            warned_negative_gap_unsafe = true;
        } else if gap < pulsetime {
            if e1.data == e2.data {
                // Choose the longest event and set the endtime to it
                let e1_endtime = e1.calculate_endtime();
                let e2_endtime = e2.calculate_endtime();
                if e2_endtime > e1_endtime {
                    e1.duration = e2_endtime - e1.timestamp;
                } else {
                    e1.duration = e1_endtime - e1.timestamp;
                }
                // Drop next event since they are merged and flooded into e1
                e1_iter.next();
                // Retry this event again to give it a change to merge e1
                // with 'e3'
                retry_e = Some(e1);
                // Since we are retrying on this event we don't want to push it
                // to the new_events vec
                continue;
            } else {
                if gap < chrono::Duration::seconds(0) {
                    if !warned_negative_gap_unsafe {
                        warn!("Gap was of negative duration ({}s) and could NOT be safely merged. This error will only show once per batch.", gap);
                        warned_negative_gap_unsafe = true;
                    }
                } else {
                    // Extend e1 to the middle between e1 and e2
                    e1.duration = e1.duration + (gap / 2);
                }
                // Make sure next event is pre-extended
                gap_prev = Some(gap);
            }
        }
        new_events.push(e1);
    }
    new_events
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
    fn test_flood() {
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
        let res = flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e_expected);

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
        let res = flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(2, res.len());
        assert_eq!(&res[0], &e1_expected);
        assert_eq!(&res[1], &e2_expected);
    }

    #[test]
    fn test_flood_containing() {
        // tests flooding an identical event contained within another event
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
        let res = flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e1);
    }

    #[test]
    fn test_flood_containing_diff() {
        // Tests flooding an event with different data contained within another event
        // Events should pass unmodified.
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
        assert_eq!(&res[0], &e1);
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
            vec![e1.clone(), e2.clone(), e3.clone(), e4.clone()],
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
            vec![e1.clone(), e2.clone(), e3.clone(), e4.clone(), e5.clone()],
            Duration::seconds(5),
        );
        assert_eq!(3, res.len());
        assert_eq!(&res[0], &e1);
        assert_eq!(&res[1], &e4);
        assert_eq!(&res[2], &e5);
    }
}
