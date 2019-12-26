extern crate chrono;
extern crate serde_json;

#[macro_use]
extern crate aw_server;

#[cfg(test)]
mod query_tests {
    use chrono;
    use chrono::Duration;
    use serde_json::json;
    use std::convert::TryFrom;

    use aw_server::query;
    use aw_server::query::QueryError;
    use aw_server::query::DataType;

    use aw_datastore::Datastore;

    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;
    use aw_models::TimeInterval;

    static TIME_INTERVAL : &str = "1980-01-01T00:00:00Z/2080-01-02T00:00:00Z";
    static BUCKET_ID : &str = "testid";

    fn setup_datastore_empty() -> Datastore {
        return Datastore::new_in_memory();
    }

    fn setup_datastore_with_bucket() -> Datastore {
        let ds = setup_datastore_empty();
        // Create bucket
        let bucket = Bucket {
            bid: None,
            id: BUCKET_ID.to_string(),
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: Some(chrono::Utc::now()),
            data: json_map!{},
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        };
        ds.create_bucket(&bucket).unwrap();
        return ds;
    }

    fn setup_datastore_populated() -> Datastore {
        let ds = setup_datastore_with_bucket();
        // Insert events
        let e1 = Event {
            id: None,
            timestamp: chrono::Utc::now(),
            duration: Duration::seconds(0),
            data: json_map!{"key": json!("value")}
        };
        let mut e2 = e1.clone();
        e2.timestamp = chrono::Utc::now();
        let mut e_replace = e2.clone();
        e_replace.data = json_map!{"key": json!("value2")};
        e_replace.duration = Duration::seconds(2);

        let mut event_list = Vec::new();
        event_list.push(e1.clone());
        event_list.push(e2.clone());

        ds.insert_events(&BUCKET_ID, &event_list).unwrap();

        return ds;
    }

    #[test]
    fn test_bool() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("True;False;a=True;True;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, true),
            ref data => panic!("Wrong datatype, {:?}", data)
        };
    }

    #[test]
    fn test_number() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("1;1.;1.1;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 1.1),
            ref data => panic!("Wrong datatype, {:?}", data)
        };
    }

    #[test]
    fn test_equals() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        // number comparison true
        let code = String::from("1==1;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, true),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // number comparison false
        let code = String::from("2==1;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, false),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // string comparison true
        let code = String::from(r#""a"=="a";"#);
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, true),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // string comparison false
        let code = String::from(r#""a"=="b";"#);
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, false),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // bool comparison true
        let code = String::from("True==True;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, true),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // bool comparison false
        let code = String::from("False==True;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Bool(b) => assert_eq!(b, false),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // different types comparison (should raise an error)
        let code = String::from("True==1;");
        match query::query(&code, &interval, &ds) {
            Err(query::QueryError::InvalidType(_)) => (),
            r => panic!("Expected an InvalidType error, got {:?}", r),
        };
    }

    #[test]
    fn test_return() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("return 1;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 1.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        let code = String::from("return 1+1;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 2.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };
    }

    #[test]
    fn test_if() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        // Test hardcoded True
        let code = String::from("
            n=1;
            if True { n=2; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 2.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test hardcoded False
        let code = String::from("
            n=1;
            if False { n=2; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 1.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test expression True
        let code = String::from("
            a=True; n=1;
            if a { n=2; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 2.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test expression False
        let code = String::from("
            a=False; n=1;
            if a { n=2; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 1.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test if else
        let code = String::from("
            a=False; n=1;
            if a { }
            else { n=3; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 3.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test if else if
        let code = String::from("
            a=False; b=True; n=1;
            if a { n=2; }
            elif b { n=3; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 3.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test if else if else
        let code = String::from("
            a=False; b=True; n=1;
            if a { n=2; }
            elif a { n=3; }
            else { n=4; }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 4.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };

        // Test if inside if
        let code = String::from("
            a=True; n=1;
            if a { if a { n = 2; } }
            return n;");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::Number(n) => assert_eq!(n, 2.0),
            ref data => panic!("Wrong datatype, {:?}", data)
        };
    }

    #[test]
    fn test_function() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("print(1);");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("print(1, 2);");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("no_such_function(1);");
        match query::query(&code, &interval, &ds) {
            Ok(ok) => panic!(format!("Expected QueryError, got {:?}", ok)),
            Err(e) => match e {
                QueryError::VariableNotDefined(qe) => assert_eq!(qe, "no_such_function"),
                qe => panic!(format!("Expected QueryError::VariableNotDefined, got {:?}", qe))
            }
        }

        let code = String::from("invalid_type=1; invalid_type(1);");
        match query::query(&code, &interval, &ds) {
            Ok(ok) => panic!(format!("Expected QueryError, got {:?}", ok)),
            Err(e) => match e {
                QueryError::InvalidType(qe) => assert_eq!(qe, "invalid_type"),
                qe => panic!(format!("Expected QueryError::VariableNotDefined, got {:?}", qe))
            }
        }
    }

    #[test]
    fn test_all_functions() {
        let ds = setup_datastore_populated();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("query_bucket(\"testid\");");
        query::query(&code, &interval, &ds).unwrap();

        let code = format!(
            r#"
            events = query_bucket("{}");
            events = flood(events);
            events = sort_by_duration(events);
            events = limit_events(events, 10000);
            events = sort_by_timestamp(events);
            events = concat(events, query_bucket("{}"));
            events = categorize(events, [[["test"], {{ "regex": "value$" }}], [["test", "testing"], {{ "regex": "value$" }}]]);
            events = tag(events, [["testtag", {{ "regex": "test$" }}], ["another testtag", {{ "regex": "test-pat$" }}]]);
            total_duration = sum_durations(events);
            bucketnames = query_bucket_names();
            print("test", "test2");
            url_events = split_url_events (events);
            filtered_events = filter_period_intersect(events, events);
            filtered_events = filter_keyvals(events, "key", ["value"]);
            chunked_events = chunk_events_by_key(events, "key");
            merged_events = merge_events_by_keys(events, ["key"]);
            RETURN = merged_events;"#,
            "testid", "testid");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::List(l) => l,
            ref data => panic!("Wrong datatype, {:?}", data)
        };
        // TODO: assert_eq result
    }

    #[test]
    fn test_classify() {
        let ds = setup_datastore_populated();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("query_bucket(\"testid\");");
        query::query(&code, &interval, &ds).unwrap();

        let code = format!(
            r#"
            events = query_bucket("{}");
            events = categorize(events, [[["Test", "Subtest"], {{ "regex": "^value$" }}]]);
            events = tag(events, [["testtag", {{ "regex": "value$" }}], ["another testtag", {{ "regex": "value$" }}]]);
            test = {{}};
            RETURN = events;"#,
            "testid"
        );
        let result: DataType = query::query(&code, &interval, &ds).unwrap();
        let events: Vec<Event> = Vec::try_from(&result).unwrap();

        let event = events.first().unwrap();
        let tags = event.data.get("$tags").unwrap().as_array().unwrap();
        let cats = event.data.get("$category").unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(cats, &serde_json::json!(vec!["Test", "Subtest"]));
    }

    #[test]
    fn test_string() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("a=\"test \\\" with escaped quote\";");
        match query::query(&code, &interval, &ds).unwrap() {
            query::DataType::String(s) => assert_eq!(s, "test \" with escaped quote"),
            _ => panic!("Wrong datatype")
        }
    }

    #[test]
    fn test_list() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("[];");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("[1];");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("[1+1];");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("[1,1];");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("[1,1+2];");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("[1,1+1,1+2+3,4/3,[1+2]];");
        query::query(&code, &interval, &ds).unwrap();
    }

    #[test]
    fn test_comment() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("1;# testing 123");
        query::query(&code, &interval, &ds).unwrap();
    }

    #[test]
    fn test_dict() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("return {};");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("return {\"test\": 2};");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("return {\"test\": 2, \"test2\": \"teststr\"};");
        query::query(&code, &interval, &ds).unwrap();

        let code = String::from("return {\"test\": {\"test\": \"test\"}};");
        query::query(&code, &interval, &ds).unwrap();
    }

    #[test]
    fn test_concat() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        // Append lists
        let code = String::from("return [1]+[2];");
        let res = query::query(&code, &interval, &ds).unwrap();
        let mut v = Vec::new();
        v.push(DataType::Number(1.0));
        v.push(DataType::Number(2.0));
        assert_eq!(res, DataType::List(v));

        // Append strings
        let code = String::from(r#"return "a"+"b";"#);
        let res = query::query(&code, &interval, &ds).unwrap();
        assert_eq!(res, DataType::String("ab".to_string()));
    }

    #[test]
    fn test_contains() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        // test list true
        let code = String::from(r#"a = ["b", "a"]; return contains(a, "a");"#);
        let res = query::query(&code, &interval, &ds).unwrap();
        assert_eq!(res, DataType::Bool(true));

        // test list false
        let code = String::from(r#"a = ["b", "a"]; return contains(a, "c");"#);
        let res = query::query(&code, &interval, &ds).unwrap();
        assert_eq!(res, DataType::Bool(false));

        // test dict true
        let code = String::from(r#"a = {"a": 1}; return contains(a, "a");"#);
        let res = query::query(&code, &interval, &ds).unwrap();
        assert_eq!(res, DataType::Bool(true));

        // test dict false
        let code = String::from(r#"a = {"b": 1}; return contains(a, "a");"#);
        let res = query::query(&code, &interval, &ds).unwrap();
        assert_eq!(res, DataType::Bool(false));
    }

    #[test]
    fn test_math() {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();

        let code = String::from("1+1;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 2.0),
            num => panic!("Expected number, got {:?}", num)
        };

        let code = String::from("1-1;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 0.0),
            num => panic!("Expected number, got {:?}", num)
        };

        let code = String::from("3*5;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 15.0),
            num => panic!("Expected number, got {:?}", num)
        };

        let code = String::from("4/2;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 2.0),
            num => panic!("Expected number, got {:?}", num)
        };

        let code = String::from("1/0;");
        match query::query(&code, &interval, &ds) {
            Err(e) => match e {
                QueryError::MathError(_) => (),
                qe => panic!("Expected division by zero error, got {:?}", qe)
            }
            Ok(res) => panic!("Expected division by zero error, got {:?}", res)
        };

        let code = String::from("2.5%1;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 0.5),
            num => panic!("Expected number, got {:?}", num)
        };

        let code = String::from("1+1+0+1;");
        match query::query(&code, &interval, &ds).unwrap() {
            DataType::Number(n) => assert_eq!(n, 3.0),
            num => panic!("Expected number, got {:?}", num)
        };
    }
}
