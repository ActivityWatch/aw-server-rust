use std::vec::Vec;

use serde_json::{Map, Value};

use aw_models::Event;

fn merge_value(a: &mut Value, b: &Value) {
    match (a, b) {
        (&mut Value::Object(ref mut a), &Value::Object(ref b)) => {
            for (kb, vb) in b {
                merge_value(a.entry(kb.clone()).or_insert(Value::Null), vb);
            }
        }
        (a, b) => {
            *a = b.clone();
        }
    }
}

fn merge_map(map1: &mut Map<String, Value>, map2: &Map<String, Value>) {
    for (k1, mut v1) in map1.iter_mut() {
        if let Some(v2) = map2.get(k1) {
            merge_value(&mut v1, &v2);
            println!("{:?}", v1);
        }
    }
    for (k2, v2) in map2.iter() {
        if !map1.contains_key(k2) {
            map1.insert(k2.to_string(), v2.clone());
        }
    }
}

/// events1 is the "master" list of events and if an event in events2
/// intersects it the intersecting part will be removed from the original
/// event and split into a new event and merges the data. It also differs from
/// a normal intersection in that the part that does not intersect from the
/// "master" events will still be kept, but if it also intersects that interval
/// will be removed.
///
/// NOTE: It is technically only a union of the event1, not event2.
///       Maybe we should improve that in the future?
///
/// Example:
/// ```ignore
///     |---------|--------------------|
///     | events1 |[a     ][b     ]    |
///     | events2 |    [c     ]    [d ]|
///     | result  |[a ][ac][bc][b ]    |
///     |---------|--------------------|
/// ```
pub fn union_events_split(events1: Vec<Event>, events2: &Vec<Event>) -> Vec<Event> {
    let mut events: Vec<Event> = Vec::new();

    'event1: for mut event1 in events1 {
        let event1_endtime = event1.calculate_endtime();
        'event2: for event2 in events2 {
            // Check that events intersect, otherwise skip
            if event2.timestamp > event1_endtime {
                continue 'event2;
            }
            let event2_endtime = event2.calculate_endtime();
            if event2_endtime < event1.timestamp {
                continue 'event2;
            }
            // Find the events common intersection
            let intersect_timestamp = std::cmp::max(event1.timestamp, event2.timestamp);
            let intersect_endtime = std::cmp::min(event1_endtime, event2_endtime);
            let intersect_duration = intersect_endtime - intersect_timestamp;

            // If event1 starts before event2, add that event
            if intersect_timestamp > event1.timestamp {
                let prepended_event = Event {
                    id: None,
                    timestamp: event1.timestamp,
                    duration: intersect_timestamp - event1.timestamp,
                    data: event1.data.clone(),
                };
                events.push(prepended_event);
            }

            // Add intersecting event
            let mut intersect_data = event1.data.clone();
            merge_map(&mut intersect_data, &event2.data);
            let intersecting_event = Event {
                id: None,
                timestamp: intersect_timestamp,
                duration: intersect_duration,
                data: intersect_data,
            };
            events.push(intersecting_event);

            // Update event1 to end at end of common event
            event1.timestamp = intersect_endtime;
            event1.duration = event1_endtime - intersect_endtime;
            if event1.duration.num_milliseconds() <= 0 {
                continue 'event1;
            }
        }
        events.push(event1);
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn test_merge_data() {
        /* test merge same */
        let mut d1 = json_map! {"test": json!(1)};
        let d2 = d1.clone();
        merge_map(&mut d1, &d2);
        assert_eq!(d1, d2);

        /* test merge different keys */
        let mut d1 = json_map! {"test1": json!(1)};
        let d2 = json_map! {"test2": json!(2)};
        merge_map(&mut d1, &d2);
        assert_eq!(d1, json_map! {"test1": json!(1), "test2": json!(2)});

        /* test merge intersecting objects */
        let mut d1 = json_map! {"test": json_map!{"a": json!(1)}};
        let d2 = json_map! {"test": json_map!{"b": json!(2)}};
        merge_map(&mut d1, &d2);
        assert_eq!(
            d1,
            json_map! {"test": json_map!{"a": json!(1), "b": json!(2)}}
        );

        /* test non-object conflict, prefer map1 value */
        // TODO: This does not work yet!
        //       It should be a pretty rare use-case anyway
        /*
        let mut d1 = json_map!{"test": json!(1)};
        let d1_orig = d1.clone();
        let d2 = json_map!{"test": json!(2)};
        merge_map(&mut d1, &d2);
        assert_eq!(d1, d1_orig);
        */
    }

    #[test]
    fn test_union_events_split() {
        // Test intersection, before and after
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(3),
            data: json_map! {"test": json!(1)},
        };
        let mut e2 = e1.clone();
        e2.timestamp = DateTime::from_str("2000-01-01T00:00:01Z").unwrap();
        e2.duration = Duration::seconds(1);

        let res = union_events_split(vec![e1.clone()], &vec![e2.clone()]);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].id, None);
        assert_eq!(res[0].timestamp, e1.timestamp);
        assert_eq!(res[0].duration, Duration::seconds(1));
        assert_eq!(res[0].data, json_map! {"test": json!(1)});
        assert_eq!(res[1].id, None);
        assert_eq!(res[1].timestamp, e2.timestamp);
        assert_eq!(res[1].duration, Duration::seconds(1));
        assert_eq!(res[1].data, json_map! {"test": json!(1)});
        assert_eq!(res[2].id, None);
        assert_eq!(res[2].timestamp, e2.timestamp + e2.duration);
        assert_eq!(res[2].duration, Duration::seconds(1));
        assert_eq!(res[2].data, json_map! {"test": json!(1)});
    }
}
