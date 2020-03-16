use aw_models::Event;

pub fn filter_period_intersect(events: &[Event], filter_events: &[Event]) -> Vec<Event> {
    let mut filtered_events = Vec::new();
    for filter in filter_events {
        let filter_endtime = filter.calculate_endtime();
        for event in events {
            if event.timestamp > filter_endtime {
                continue;
            }
            let event_endtime = event.calculate_endtime();
            if event_endtime < filter.timestamp {
                continue;
            }
            let mut e = event.clone();
            e.timestamp = std::cmp::max(e.timestamp, filter.timestamp);
            let endtime = std::cmp::min(event_endtime, filter_endtime);
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
