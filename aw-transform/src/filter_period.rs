use aw_models::Event;
use chrono::{DateTime, Utc};

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
pub fn filter_period_intersect(events: &[Event], filter_events: &[Event]) -> Vec<Event> {
    let mut filtered_events = Vec::new();

    // Start with pre-calculating endtimes of events
    let mut events_with_endtimes: Vec<(&Event, DateTime<Utc>)> = Vec::new();
    for event in events {
        events_with_endtimes.push((event, event.calculate_endtime()));
    }

    // Do actual filtering
    for filter in filter_events {
        let filter_endtime = filter.calculate_endtime();
        for (event, event_endtime) in &events_with_endtimes {
            if event.timestamp > filter_endtime {
                continue;
            }
            if *event_endtime < filter.timestamp {
                continue;
            }
            let mut e = (*event).clone();
            e.timestamp = std::cmp::max(e.timestamp, filter.timestamp);
            let endtime = std::cmp::min(*event_endtime, filter_endtime);
            e.duration = endtime - e.timestamp;
            filtered_events.push(e);
        }
    }
    filtered_events
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
            filter_period_intersect(&vec![e1, e2, e3, e4, e5], &vec![filter_event]);
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
    }
}
