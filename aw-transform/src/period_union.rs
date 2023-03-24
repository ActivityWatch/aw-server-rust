use super::sort::sort_by_timestamp;
use aw_models::Event;
use std::collections::VecDeque;

/// Takes a list of two events and returns a new list of events covering the union
/// of the timeperiods contained in the eventlists with no overlapping events.
///
/// aw-core implementation: https://github.com/ActivityWatch/aw-core/blob/b11fbe08a0405dec01380493f7b3261163cc6878/aw_transform/filter_period_intersect.py#L92
///
/// WARNING: This function strips all data from events as it cannot keep it consistent.
///
///
/// # Example
/// ```ignore
///   events1   |   -------       --------- |
///   events2   | ------  ---  --    ----   |
///   result    | -----------  -- --------- |
/// ```
pub fn period_union(events1: &[Event], events2: &[Event]) -> Vec<Event> {
    let mut sorted_events: VecDeque<Event> = VecDeque::new();
    sorted_events.extend(sort_by_timestamp([events1, events2].concat()));

    let mut events_union = Vec::new();

    if !sorted_events.is_empty() {
        events_union.push(sorted_events.pop_front().unwrap())
    }

    for e in sorted_events {
        let last_event = events_union.last().unwrap();

        let e_p = e.interval();
        let le_p = last_event.interval();

        match e_p.union(&le_p) {
            Some(new_period) => {
                // If no gap and could be unioned, modify last event
                let mut e_mod = events_union.pop().unwrap();
                e_mod.duration = new_period.duration();
                events_union.push(e_mod);
            }
            None => {
                // If gap and could not be unioned, push event
                events_union.push(e);
            }
        }
    }

    events_union
        .drain(..)
        .map(|mut e| {
            e.data = json_map! {};
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;
    use serde_json::json;

    use aw_models::Event;

    use super::period_union;

    #[test]
    fn test_period_union_empty() {
        let e_result = period_union(&[], &[]);
        assert_eq!(e_result.len(), 0);
    }

    #[test]
    fn test_period_union() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        let mut e2 = e1.clone();
        e2.timestamp = DateTime::from_str("2000-01-01T00:00:02Z").unwrap();

        let e_result = period_union(&[e1], &[e2]);
        assert_eq!(e_result.len(), 1);

        let dt: DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:01.000Z").unwrap();
        assert_eq!(e_result[0].timestamp, dt);
        assert_eq!(e_result[0].duration, Duration::milliseconds(2000));
    }

    /// Make sure nothing gets done when nothing to union (gaps present)
    #[test]
    fn test_period_union_nop() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        let mut e2 = e1.clone();
        e2.timestamp = DateTime::from_str("2000-01-01T00:00:03Z").unwrap();

        let e_result = period_union(&[e1], &[e2]);
        assert_eq!(e_result.len(), 2);
    }

    #[test]
    fn test_period_union_2nd_empty() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        let e_result = period_union(&[e1], &[]);
        assert_eq!(e_result.len(), 1);
    }

    #[test]
    fn test_period_union_1st_empty() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        let e_result = period_union(&[], &[e1]);
        assert_eq!(e_result.len(), 1);
    }
}
