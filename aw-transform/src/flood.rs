use aw_models::Event;

use crate::sort_by_timestamp;

pub fn flood(events: Vec<Event>, pulsetime: chrono::Duration) -> Vec<Event> {
    let mut warned_negative_gap_safe = false;
    let mut warned_negative_gap_unsafe = false;
    let mut events_sorted = sort_by_timestamp(events);
    let mut e1_iter = events_sorted.drain(..).peekable();
    let mut new_events = Vec::new();
    let mut drop_next = false;
    let mut gap_prev: Option<chrono::Duration> = None;
    while let Some(mut e1) = e1_iter.next() {
        if drop_next {
            drop_next = false;
            continue;
        }
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

        if gap < pulsetime {
            if e1.data == e2.data {
                if chrono::Duration::seconds(0) > gap && !warned_negative_gap_safe {
                    warn!("Gap was of negative duration ({}s), but could be safely merged. This error will only show once per batch.", gap);
                    warned_negative_gap_safe = true;
                }
                // Choose the longest event and set the endtime to it
                // TODO: Also possibly extend to an e3 if that exists?
                let e1_endtime = e1.calculate_endtime();
                let e2_endtime = e2.calculate_endtime();
                if e2_endtime > e1_endtime {
                    e1.duration = e2_endtime - e1.timestamp;
                } else {
                    e1.duration = e1_endtime - e1.timestamp;
                }
                // Drop next event since they are merged and flooded into e1
                drop_next = true;
            } else {
                if chrono::Duration::seconds(0) > gap {
                    if !warned_negative_gap_unsafe {
                        warn!("Gap was of negative duration ({}s) and could NOT be safely merged. This error will only show once per batch.", gap);
                        warned_negative_gap_unsafe = true;
                    }
                }
                // Extend e1 to the middle between e1 and e2
                e1.duration = e1.duration + (gap / 2);
                // Make sure next event is pre-extended
                gap_prev = Some(gap);
            }
        }
        new_events.push(e1.clone());
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
    fn test_flood_same_timestamp() {
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
}
