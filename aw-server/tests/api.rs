#[macro_use]
extern crate log;

extern crate rocket;

extern crate aw_datastore;
extern crate aw_server;

// TODO: Validate return data on more places

#[cfg(test)]
mod api_tests {
    use chrono::{DateTime, Utc};
    use rocket::http::{ContentType, Header, Status};
    use std::path::PathBuf;
    use std::sync::Mutex;

    use aw_server::config;
    use aw_server::endpoints;

    use aw_models::KeyValue;
    use aw_models::{Bucket, BucketsExport};
    use rocket::local::Client;

    fn setup_testserver() -> rocket::Rocket {
        let state = endpoints::ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_path: PathBuf::from("aw-webui/dist"),
        };
        let aw_config = config::AWConfig::default();
        endpoints::build_rocket(state, &aw_config)
    }

    #[test]
    fn test_bucket() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Get empty list of buckets
        let mut res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try to fetch non-existing bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Create bucket
        res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(
                r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try to create bucket which already exists
        res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(
                r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotModified);

        // Get list of buckets (1 bucket)
        res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        // TODO: assert data
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get newly created bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        // Validate output
        let bucket: Bucket = serde_json::from_str(&res.body_string().unwrap()).unwrap();
        assert_eq!(bucket.id, "id");
        assert_eq!(bucket._type, "type");
        assert_eq!(bucket.client, "client");
        assert_eq!(bucket.hostname, "hostname");
        assert_eq!(bucket.events, None);
        assert_eq!(bucket.metadata.start, None);
        assert_eq!(bucket.metadata.end, None);

        // Get non-existing bucket
        res = client
            .get("/api/0/buckets/invalid_bucket")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Delete bucket
        res = client
            .delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try (and fail) to get deleted bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);
    }

    #[test]
    fn test_events() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Create bucket
        let mut res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(
                r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Insert a single event
        res = client
            .post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .body(
                r#"[{
                "timestamp": "2018-01-01T01:01:01Z",
                "duration": 1.0,
                "data": {}
            }]"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(
            res.body_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":1.0,"data":{}}]"#
        );
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get inserted event
        res = client
            .get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(
            res.body_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":1.0,"data":{}}]"#
        );
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Heartbeat
        res = client
            .post("/api/0/buckets/id/heartbeat?pulsetime=2")
            .header(ContentType::JSON)
            .body(
                r#"{
                "timestamp": "2018-01-01T01:01:02Z",
                "duration": 1.0,
                "data": {}
            }"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.body_string().unwrap(),
            r#"{"id":null,"timestamp":"2018-01-01T01:01:01Z","duration":2.0,"data":{}}"#
        );

        // Get heartbeat event
        res = client
            .get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(
            res.body_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":2.0,"data":{}}]"#
        );
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Delete event
        client.delete("/api/0/buckets/id/events/1").dispatch();

        // Get eventcount
        res = client
            .get("/api/0/buckets/id/events/count")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), "0");
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Delete bucket
        res = client
            .delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
    }

    #[test]
    fn test_import_export() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Import bucket
        let mut res = client
            .post("/api/0/import")
            .header(ContentType::JSON)
            .body(
                r#"{"buckets":
            {"id1": {
                "id": "id1",
                "type": "type",
                "client": "client",
                "hostname": "hostname",
                "events": [{
                    "timestamp":"2000-01-01T00:00:00Z",
                    "duration":1.0,
                    "data": {}
                }]
            }}}"#,
            )
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Export single created bucket
        let mut res = client
            .get("/api/0/buckets/id1/export")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export: BucketsExport = serde_json::from_str(&res.body_string().unwrap()).unwrap();

        // Delete bucket so we can import it again
        res = client
            .delete("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Import exported bucket again but with multipart/form-data
        // NOTE: DO NOT TOUCH START AND END VARS!
        // They are byte-perfect and this was really cumbersome to fix, modifying them will most
        // likely break the test
        let start = b"--a\r\nContent-Disposition: form-data; name=\"test\"\r\n\r\n";
        let content = serde_json::to_vec(&export).unwrap();
        let end = b"--a--";
        let sum = [&start[..], &content[..], &end[..]].concat();
        let mut res = client
            .post("/api/0/import")
            .header(Header::new(
                "Content-Type",
                "multipart/form-data; boundary=a",
            ))
            .body(&sum[..])
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get created bucket
        let mut res = client
            .get("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Export all buckets
        let mut res = client
            .get("/api/0/export")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export: BucketsExport = serde_json::from_str(&res.body_string().unwrap()).unwrap();
        let mut buckets = export.buckets;
        assert_eq!(buckets.len(), 1);
        let b = buckets.remove("id1").unwrap();
        assert_eq!(b.events.unwrap().len(), 1);

        assert_eq!(buckets.len(), 0);
    }

    #[test]
    fn test_query() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Minimal query
        let mut res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return 1;"]
            }"#,
            )
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[1.0]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Create bucket to query later
        let mut res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(
                r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Insert a event to query later
        res = client
            .post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .body(
                r#"[{
                "timestamp": "2018-01-01T01:01:01Z",
                "duration": 1.0,
                "data": {}
            }]"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Query events
        let mut res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return query_bucket(\"id\");"]
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.body_string().unwrap(),
            r#"[[{"data":{},"duration":1.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]]"#
        );

        // Test error
        let mut res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": [""]
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::InternalServerError);
        assert_eq!(
            res.body_string().unwrap(),
            r#"{"message":"EmptyQuery","reason":"Internal Server Error (Query Error)","status":500}"#
        );
    }

    fn set_setting_request(client: &Client, key: &str, value: &str) -> Status {
        let body = serde_json::to_string(&KeyValue {
            key: key.to_string(),
            value: value.to_string(),
            timestamp: None,
        })
        .unwrap();
        let res = client
            .post("/api/0/settings/")
            .header(ContentType::JSON)
            .body(body)
            .dispatch();
        res.status()
    }

    /// Asserts that 2 KeyValues are otherwise equal and first keyvalues timestamp is within
    /// or equal with timestamp and second.timestamp
    fn _equal_and_timestamp_in_range(before: DateTime<Utc>, first: KeyValue, second: KeyValue) {
        assert_eq!(first.key, second.key);
        assert_eq!(first.value, second.value);
        // Compare with second, not millisecond accuracy
        assert!(
            first.timestamp.unwrap().timestamp() >= before.timestamp(),
            "{} wasn't after {}",
            first.timestamp.unwrap().timestamp(),
            before.timestamp()
        );
        assert!(
            first.timestamp < second.timestamp,
            "{} wasn't before {}",
            first.timestamp.unwrap(),
            second.timestamp.unwrap()
        );
    }

    #[test]
    fn test_illegally_long_key() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Test getting not found (getting nonexistent key)
        let res = set_setting_request(&client, "thisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongk", "");
        assert_eq!(res, rocket::http::Status::BadRequest);
    }

    #[test]
    fn test_setting_setting() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Test value creation
        let response_status = set_setting_request(&client, "test_key", "test_value");
        assert_eq!(response_status, rocket::http::Status::Created);
    }

    #[test]
    fn test_getting_not_found_value() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Test getting not found (getting nonexistent key)
        let res = client.get("/api/0/settings/non_existent_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);
    }

    #[test]
    fn settings_list_get() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        let response1_status = set_setting_request(&client, "test_key", "");
        assert_eq!(response1_status, rocket::http::Status::Created);
        let response2_status = set_setting_request(&client, "test_key_2", "");
        assert_eq!(response2_status, rocket::http::Status::Created);

        let mut res = client.get("/api/0/settings/").dispatch();

        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.body_string().unwrap(),
            r#"[{"key":"settings.test_key"},{"key":"settings.test_key_2"}]"#
        );
    }

    #[test]
    fn test_getting_setting() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        let timestamp = Utc::now();
        let response_status = set_setting_request(&client, "test_key", "test_value");
        assert_eq!(response_status, rocket::http::Status::Created);

        // Test getting
        let mut res = client.get("/api/0/settings/test_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.body_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new("settings.test_key", "test_value", Utc::now()),
        );
    }

    #[test]
    fn test_updating_setting() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        let timestamp = Utc::now();
        let post_1_status = set_setting_request(&client, "test_key", "test_value");
        assert_eq!(post_1_status, rocket::http::Status::Created);

        let mut res = client.get("/api/0/settings/test_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.body_string().unwrap()).unwrap();

        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new("settings.test_key", "test_value", Utc::now()),
        );

        let timestamp_2 = Utc::now();
        let post_2_status = set_setting_request(&client, "test_key", "changed_test_value");
        assert_eq!(post_2_status, rocket::http::Status::Created);

        let mut res = client.get("/api/0/settings/test_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        let new_deserialized: KeyValue = serde_json::from_str(&res.body_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp_2,
            new_deserialized,
            KeyValue::new("settings.test_key", "changed_test_value", Utc::now()),
        );
    }

    #[test]
    fn test_deleting_setting() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        let response_status = set_setting_request(&client, "test_key", "");
        assert_eq!(response_status, rocket::http::Status::Created);

        // Test deleting
        let res = client.delete("/api/0/settings/test_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        let res = client.get("/api/0/settings/test_key").dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);
    }
}
