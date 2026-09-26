use std::cmp::max;

use aw_models::Event;
use chrono::{DateTime, Utc};

use crate::sort_by_timestamp;

/// Removes events not intersecting with the provided filter_events
///
/// Usually used to filter buckets unaware if the user is making any activity with an bucket which
/// is aware if the user is at the computer or not.
/// For example the events from aw-watcher-window should be called with filter_period_intersect
/// with the "not-afk" events from aw-watcher-afk to give events with durations of only when the
/// user is at the computer.
///
/// # Example
/// ```ignore
/// events:        [a          ][b   ]
/// filter_events: [     ]  [      ]
/// output:        [a    ]  [a ][b ]
/// ```
pub fn filter_period_intersect(events: Vec<Event>, filter_events: Vec<Event>) -> Vec<Event> {
    // This is a port of aw-core's filter_period_intersect, and must stay in sync with it:
    //     https://github.com/ActivityWatch/aw-core/blob/master/aw_transform/filter_period_intersect.py
    //
    // Intervals are treated as closed when either of them has zero duration: a zero-duration event
    // lying within a filter event (boundaries included) is kept, and an event containing a
    // zero-duration filter event yields a zero-duration part at that point.
    let events = sort_by_timestamp(events);
    let filter_events = sort_by_timestamp(filter_events);

    let mut filtered_events = Vec::new();
    let mut i = 0;
    let mut j = 0;
    // Start of the part of events[i] that has not been emitted yet, so that overlapping filter
    // events don't emit the same time twice.
    let mut emitted_until: Option<DateTime<Utc>> = None;
    while i < events.len() && j < filter_events.len() {
        let event = &events[i];
        let filter = &filter_events[j];
        let e_start = match emitted_until {
            Some(t) => max(t, event.timestamp),
            None => event.timestamp,
        };
        let e_end = event.calculate_endtime();
        let (f_start, f_end) = (filter.timestamp, filter.calculate_endtime());

        match intersection(e_start, e_end, f_start, f_end) {
            Some((start, end)) => {
                let mut e = event.clone();
                e.timestamp = start;
                e.duration = end - start;
                filtered_events.push(e);
                if e_end <= f_end {
                    i += 1;
                    emitted_until = None;
                } else {
                    j += 1;
                    emitted_until = Some(end);
                }
            }
            // Event ended before filter event started
            None if e_end <= f_start => {
                i += 1;
                emitted_until = None;
            }
            // Event started after filter event ended
            None => j += 1,
        }
    }
    filtered_events
}

/// Intersection of the intervals `a` and `b`, with the semantics of `Timeslot.intersection` from
/// the `timeslot` Python package used by aw-core.
fn intersection(
    a_start: DateTime<Utc>,
    a_end: DateTime<Utc>,
    b_start: DateTime<Utc>,
    b_end: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    if a_start <= b_start && b_end <= a_end {
        // b is within a
        Some((b_start, b_end))
    } else if a_start <= b_start && b_start < a_end {
        // End of a intersects start of b
        Some((b_start, a_end))
    } else if a_start < b_end && b_end <= a_end {
        // Start of a intersects end of b
        Some((a_start, b_end))
    } else if b_start <= a_start && a_end <= b_end {
        // a is within b
        Some((a_start, a_end))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;
    use serde_json::json;

    use aw_models::Event;

    use super::filter_period_intersect;

    #[test]
    fn test_filter_period_intersect() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let mut e2 = e1.clone();
        e2.timestamp = DateTime::from_str("2000-01-01T00:00:02Z").unwrap();
        let mut e3 = e1.clone();
        e3.timestamp = DateTime::from_str("2000-01-01T00:00:03Z").unwrap();
        let mut e4 = e1.clone();
        e4.timestamp = DateTime::from_str("2000-01-01T00:00:04Z").unwrap();
        let mut e5 = e1.clone();
        e5.timestamp = DateTime::from_str("2000-01-01T00:00:05Z").unwrap();

        let filter_event = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02.5Z").unwrap(),
            duration: Duration::seconds(2),
            data: json_map! {"test": json!(1)},
        };

        let filtered_events =
            filter_period_intersect(vec![e1, e2, e3, e4, e5], vec![filter_event.clone()]);
        assert_eq!(filtered_events.len(), 3);
        assert_eq!(filtered_events[0].duration, Duration::milliseconds(500));
        assert_eq!(filtered_events[1].duration, Duration::milliseconds(1000));
        assert_eq!(filtered_events[2].duration, Duration::milliseconds(500));

        let dt: DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:02.500Z").unwrap();
        assert_eq!(filtered_events[0].timestamp, dt);
        let dt: DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:03.000Z").unwrap();
        assert_eq!(filtered_events[1].timestamp, dt);
        let dt: DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:04.000Z").unwrap();
        assert_eq!(filtered_events[2].timestamp, dt);

        let timestamp_01s = DateTime::from_str("2000-01-01T00:00:01Z").unwrap();
        let e = Event {
            id: None,
            timestamp: timestamp_01s,
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let mut f2 = filter_event.clone();
        f2.timestamp = DateTime::from_str("2000-01-01T00:00:00Z").unwrap();
        f2.duration = Duration::milliseconds(1500);
        let res = filter_period_intersect(vec![e.clone()], vec![f2]);
        assert_eq!(res[0].timestamp, timestamp_01s);
        assert_eq!(res[0].duration, Duration::milliseconds(500));

        let timestamp_01_5s = DateTime::from_str("2000-01-01T00:00:01.5Z").unwrap();
        let mut f3 = filter_event.clone();
        f3.timestamp = timestamp_01_5s;
        f3.duration = Duration::milliseconds(1000);
        let res = filter_period_intersect(vec![e.clone()], vec![f3]);
        assert_eq!(res[0].timestamp, timestamp_01_5s);
        assert_eq!(res[0].duration, Duration::milliseconds(500));

        let mut f4 = filter_event.clone();
        f4.timestamp = DateTime::from_str("2000-01-01T00:00:01.5Z").unwrap();
        f4.duration = Duration::milliseconds(100);
        let res = filter_period_intersect(vec![e.clone()], vec![f4]);
        assert_eq!(res[0].timestamp, timestamp_01_5s);
        assert_eq!(res[0].duration, Duration::milliseconds(100));

        let mut f5 = filter_event.clone();
        f5.timestamp = DateTime::from_str("2000-01-01T00:00:00Z").unwrap();
        f5.duration = Duration::seconds(10);
        let res = filter_period_intersect(vec![e.clone()], vec![f5]);
        assert_eq!(res[0].timestamp, timestamp_01s);
        assert_eq!(res[0].duration, Duration::milliseconds(1000));
    }

    /// Event starting `start` seconds after 10:00:00Z, lasting `duration` seconds.
    fn ev(start: i64, duration: i64, app: &str) -> Event {
        let base: DateTime<Utc> = DateTime::from_str("2026-01-01T10:00:00Z").unwrap();
        Event {
            id: None,
            timestamp: base + Duration::seconds(start),
            duration: Duration::seconds(duration),
            data: json_map! {"app": json!(app)},
        }
    }

    #[test]
    fn test_filter_period_intersect_zero_duration() {
        // Repro from ActivityWatch/aw-server-rust#747
        let res = filter_period_intersect(vec![ev(5, 0, "x")], vec![ev(0, 10, "")]);
        assert_eq!(res, vec![ev(5, 0, "x")]);

        // Boundaries are included, like in aw-core
        let res = filter_period_intersect(vec![ev(0, 0, "x")], vec![ev(0, 10, "")]);
        assert_eq!(res, vec![ev(0, 0, "x")]);
        let res = filter_period_intersect(vec![ev(10, 0, "x")], vec![ev(0, 10, "")]);
        assert_eq!(res, vec![ev(10, 0, "x")]);

        // Outside of and between filter events they are dropped
        let res = filter_period_intersect(
            vec![ev(0, 0, "a"), ev(15, 0, "b"), ev(30, 0, "c")],
            vec![ev(5, 5, ""), ev(20, 5, "")],
        );
        assert_eq!(res, vec![]);

        // At the boundary of two adjacent filter events they are emitted once
        let res =
            filter_period_intersect(vec![ev(10, 0, "x")], vec![ev(0, 10, ""), ev(10, 10, "")]);
        assert_eq!(res, vec![ev(10, 0, "x")]);

        // Mixed with events that have a duration
        let res = filter_period_intersect(
            vec![
                ev(0, 3, "a"),
                ev(4, 0, "b"),
                ev(5, 5, "a"),
                ev(12, 0, "c"),
                ev(20, 0, "d"),
            ],
            vec![ev(2, 4, ""), ev(8, 4, "")],
        );
        assert_eq!(
            res,
            vec![
                ev(2, 1, "a"),
                ev(4, 0, "b"),
                ev(5, 1, "a"),
                ev(8, 2, "a"),
                ev(12, 0, "c")
            ]
        );
    }

    #[test]
    fn test_filter_period_intersect_zero_duration_filter() {
        // Like aw-core, an event containing a zero-duration filter event yields a zero-duration
        // part at that point.
        let res = filter_period_intersect(vec![ev(0, 1, "x")], vec![ev(0, 0, ""), ev(0, 10, "")]);
        assert_eq!(res, vec![ev(0, 0, "x"), ev(0, 1, "x")]);

        let res = filter_period_intersect(vec![ev(0, 10, "x")], vec![ev(5, 0, "")]);
        assert_eq!(res, vec![ev(5, 0, "x")]);
    }

    #[test]
    fn test_filter_period_intersect_overlapping_filters() {
        // Time covered by several filter events is emitted once
        let res = filter_period_intersect(vec![ev(0, 10, "x")], vec![ev(0, 6, ""), ev(4, 4, "")]);
        assert_eq!(res, vec![ev(0, 6, "x"), ev(6, 2, "x")]);

        // A filter event contained in an earlier one adds nothing, also when they end at the
        // same time
        for filter in [ev(2, 2, ""), ev(2, 4, "")] {
            let res = filter_period_intersect(vec![ev(0, 10, "x")], vec![ev(0, 6, ""), filter]);
            assert_eq!(res, vec![ev(0, 6, "x")]);
        }

        // A zero-duration filter event within time that was already emitted adds nothing
        let res = filter_period_intersect(
            vec![ev(0, 10, "x")],
            vec![ev(0, 6, ""), ev(4, 4, ""), ev(5, 0, "")],
        );
        assert_eq!(res, vec![ev(0, 6, "x"), ev(6, 2, "x")]);
    }

    #[test]
    fn test_filter_period_intersect_randomized() {
        // Deterministic pseudo-random non-overlapping events (with some zero-duration ones) and
        // non-overlapping filter events, like window and not-afk events.
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let sequence = |n: u64, min_duration: u64, next: &mut dyn FnMut(u64) -> u64| {
            let mut t = 0;
            let mut events = Vec::new();
            for k in 0..n {
                t += next(4) as i64;
                let d = if next(4) == 0 {
                    0
                } else {
                    (min_duration + next(6)) as i64
                };
                events.push(ev(t, d, &k.to_string()));
                t += d;
            }
            events
        };
        for _ in 0..2000 {
            let n_events = next(8);
            let n_filters = next(5);
            let events = sequence(n_events, 0, &mut next);
            let filters: Vec<Event> = sequence(n_filters, 1, &mut next)
                .into_iter()
                .filter(|f| f.duration > Duration::zero())
                .collect();
            let res = filter_period_intersect(events.clone(), filters.clone());

            let ctx = format!("events: {events:?}\nfilters: {filters:?}\nresult: {res:?}");
            assert!(
                res.windows(2)
                    .all(|w| w[0].calculate_endtime() <= w[1].timestamp),
                "{ctx}"
            );
            // Every part lies within its event and within a filter event
            for r in &res {
                assert!(
                    events.iter().any(|e| e.data == r.data
                        && e.timestamp <= r.timestamp
                        && r.calculate_endtime() <= e.calculate_endtime()),
                    "{ctx}"
                );
                assert!(
                    filters.iter().any(|f| f.timestamp <= r.timestamp
                        && r.calculate_endtime() <= f.calculate_endtime()),
                    "{ctx}"
                );
            }
            // The total duration is the total overlap between events and filter events
            let overlap: Duration = events
                .iter()
                .flat_map(|e| filters.iter().map(move |f| (e, f)))
                .map(|(e, f)| {
                    let start = e.timestamp.max(f.timestamp);
                    let end = e.calculate_endtime().min(f.calculate_endtime());
                    (end - start).max(Duration::zero())
                })
                .sum();
            let total: Duration = res.iter().map(|e| e.duration).sum();
            assert_eq!(total, overlap, "{ctx}");
            // Zero-duration events are kept once if they lie within a filter event
            for e in events.iter().filter(|e| e.duration == Duration::zero()) {
                let inside = filters
                    .iter()
                    .any(|f| f.timestamp <= e.timestamp && e.timestamp <= f.calculate_endtime());
                let count = res.iter().filter(|r| *r == e).count();
                assert_eq!(count, usize::from(inside), "{e:?}\n{ctx}");
            }
        }
    }
}
