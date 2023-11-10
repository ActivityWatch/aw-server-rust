use aw_models::Event;
use chrono::Duration;

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
    if events.len() == 0 || filter_events.len() == 0 {
        return Vec::new();
    }

    let mut filtered_events = Vec::new();
    let events = sort_by_timestamp(events);
    let filter_events = sort_by_timestamp(filter_events);

    let mut events_iter = events.into_iter();
    let mut filter_events_iter = filter_events.into_iter();
    let mut cur_event = events_iter.next().unwrap();
    let mut cur_filter_event = filter_events_iter.next().unwrap();

    loop {
        let event_endtime = cur_event.calculate_endtime();
        let filter_endtime = cur_filter_event.calculate_endtime();
        if cur_event.duration == Duration::seconds(0) || event_endtime <= cur_filter_event.timestamp
        {
            match events_iter.next() {
                Some(e) => {
                    cur_event = e;
                    continue;
                }
                None => return filtered_events,
            }
        }
        if cur_event.timestamp >= cur_filter_event.calculate_endtime() {
            match filter_events_iter.next() {
                Some(e) => {
                    cur_filter_event = e;
                    continue;
                }
                None => return filtered_events,
            }
        }

        let mut e = cur_event.clone();
        e.timestamp = std::cmp::max(e.timestamp, cur_filter_event.timestamp);
        let endtime = std::cmp::min(event_endtime, filter_endtime);
        e.duration = endtime - e.timestamp;

        // trim current event
        let old_timestamp = cur_event.timestamp;
        cur_event.timestamp = e.timestamp + e.duration;
        cur_event.duration = old_timestamp + cur_event.duration - cur_event.timestamp;

        filtered_events.push(e);
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
}
