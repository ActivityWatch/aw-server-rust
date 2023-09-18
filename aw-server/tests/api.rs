extern crate log;

extern crate rocket;

extern crate aw_datastore;
extern crate aw_server;

#[cfg(test)]
mod api_tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use chrono::{DateTime, Utc};
    use rocket::http::{ContentType, Header, Status};
    use serde_json::{json, Value};

    use aw_server::config;
    use aw_server::endpoints;

    use aw_models::KeyValue;
    use aw_models::{Bucket, BucketsExport};
    use rocket::local::blocking::Client;

    fn setup_testserver() -> rocket::Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_resolver: endpoints::AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let aw_config = config::AWConfig::default();
        endpoints::build_rocket(state, aw_config)
    }

    #[test]
    fn test_bucket() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Get empty list of buckets
        let mut res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let buckets: HashMap<String, Bucket> =
            serde_json::from_str(&res.into_string().unwrap()).unwrap();
        assert_eq!(buckets.len(), 0);

        // Try to fetch non-existing bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Create bucket
        res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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

        // Try to create bucket which already exists
        res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotModified);
        assert_eq!(
            res.into_string().unwrap(),
            r#"{"message":"Bucket 'id' already exists"}"#
        );

        // Get list of buckets (1 bucket)
        res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let buckets: HashMap<String, Bucket> =
            serde_json::from_str(&res.into_string().unwrap()).unwrap();
        assert_eq!(buckets.len(), 1);
        let bucket = buckets.get("id").unwrap();
        assert_eq!(bucket.id, "id");
        assert_eq!(bucket._type, "type");
        assert_eq!(bucket.client, "client");
        assert_eq!(bucket.hostname, "hostname");
        assert!(bucket.events.is_none());
        assert_eq!(bucket.metadata.start, None);
        assert_eq!(bucket.metadata.end, None);

        // Get newly created bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        // Validate output
        let bucket: Bucket = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        assert_eq!(bucket.id, "id");
        assert_eq!(bucket._type, "type");
        assert_eq!(bucket.client, "client");
        assert_eq!(bucket.hostname, "hostname");
        assert!(bucket.events.is_none());
        assert_eq!(bucket.metadata.start, None);
        assert_eq!(bucket.metadata.end, None);

        // Get non-existing bucket
        res = client
            .get("/api/0/buckets/invalid_bucket")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Delete bucket
        res = client
            .delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try (and fail) to get deleted bucket
        res = client
            .get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Get empty list of buckets
        let res = client
            .get("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let buckets: HashMap<String, Bucket> =
            serde_json::from_str(&res.into_string().unwrap()).unwrap();
        assert_eq!(buckets.len(), 0);
    }

    #[test]
    fn test_events() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Create bucket
        let res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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

        // Insert a single event
        let res = client
            .post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"[{
                "timestamp": "2018-01-01T01:01:01Z",
                "duration": 1.0,
                "data": {}
            }]"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":1.0,"data":{}}]"#
        );

        // Get inserted event
        let res = client
            .get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":1.0,"data":{}}]"#
        );

        // Heartbeat
        let res = client
            .post("/api/0/buckets/id/heartbeat?pulsetime=2")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"{
                "timestamp": "2018-01-01T01:01:02Z",
                "duration": 1.0,
                "data": {}
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"{"id":null,"timestamp":"2018-01-01T01:01:01Z","duration":2.0,"data":{}}"#
        );

        // Get heartbeat event
        let res = client
            .get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":2.0,"data":{}}]"#
        );

        // Delete event
        client
            .delete("/api/0/buckets/id/events/1")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();

        // Get eventcount
        let res = client
            .get("/api/0/buckets/id/events/count")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(res.into_string().unwrap(), "0");

        // Delete bucket
        let res = client
            .delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
    }

    #[test]
    fn test_import_export() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Import bucket
        let res = client
            .post("/api/0/import")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // TODO: test more error cases
        // Import already existing bucket
        let res = client
            .post("/api/0/import")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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
        assert_eq!(res.status(), rocket::http::Status::InternalServerError);
        assert_eq!(
            res.into_string().unwrap(),
            r#"{"message":"Failed to import bucket: BucketAlreadyExists(\"id1\")"}"#
        );

        // Export single created bucket
        let res = client
            .get("/api/0/buckets/id1/export")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export: BucketsExport = serde_json::from_str(&res.into_string().unwrap()).unwrap();

        // Delete bucket so we can import it again
        let res = client
            .delete("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Import exported bucket again but with multipart/form-data
        let boundary_start = b"--a\r\n";
        let disposition = b"Content-Disposition: form-data; name=\"buckets.json\"\r\n";
        let content_type = b"Content-Type: application/json\r\n";
        let data_start = b"\r\n";
        let content = serde_json::to_vec(&export).unwrap();
        let boundary_end = b"\r\n--a--";
        let sum = [
            &boundary_start[..],
            &disposition[..],
            &content_type[..],
            &data_start[..],
            &content[..],
            &boundary_end[..],
        ]
        .concat();

        let res = client
            .post("/api/0/import")
            .header(Header::new(
                "Content-Type",
                "multipart/form-data; boundary=a",
            ))
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(&sum[..])
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get created bucket
        let res = client
            .get("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        println!("{:?}", res.into_string());

        // Export all buckets
        let res = client
            .get("/api/0/export")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export: BucketsExport = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        let mut buckets = export.buckets;
        assert_eq!(buckets.len(), 1);
        let b = buckets.remove("id1").unwrap();
        assert_eq!(b.events.unwrap().take_inner().len(), 1);

        assert_eq!(buckets.len(), 0);
    }

    #[test]
    fn test_query() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Minimal query
        let res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return 1;"]
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(res.into_string().unwrap(), r#"[1.0]"#);

        // Create bucket to query later
        let res = client
            .post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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
        let res = client
            .post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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
        let res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return query_bucket(\"id\");"]
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"[[{"data":{},"duration":1.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]]"#
        );

        // Test error
        let res = client
            .post("/api/0/query")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .body(
                r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": [""]
            }"#,
            )
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::InternalServerError);
        assert_eq!(res.into_string().unwrap(), r#"{"message":"EmptyQuery"}"#);
    }

    fn set_setting_request(client: &Client, key: &str, value: Value) -> Status {
        let body = serde_json::to_string(&KeyValue {
            key: key.to_string(),
            value,
            timestamp: None,
        })
        .unwrap();
        let res = client
            .post("/api/0/settings/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
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
        let client = Client::untracked(server).expect("valid instance");

        // Test getting not found (getting nonexistent key)
        let res = set_setting_request(&client, "thisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongkthisisaverylongk", json!(""));
        assert_eq!(res, rocket::http::Status::BadRequest);
    }

    #[test]
    fn test_setting_setting() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Test value creation
        let response_status = set_setting_request(&client, "test_key", json!("test_value"));
        assert_eq!(response_status, rocket::http::Status::Created);
    }

    #[test]
    fn test_getting_not_found_value() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        // Test getting not found (getting nonexistent key)
        let res = client
            .get("/api/0/settings/non_existent_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);
    }

    #[test]
    fn settings_list_get() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let response1_status = set_setting_request(&client, "test_key", json!(""));
        assert_eq!(response1_status, rocket::http::Status::Created);
        let response2_status = set_setting_request(&client, "test_key_2", json!(""));
        assert_eq!(response2_status, rocket::http::Status::Created);

        let res = client
            .get("/api/0/settings/")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();

        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(
            res.into_string().unwrap(),
            r#"[{"key":"settings.test_key"},{"key":"settings.test_key_2"}]"#
        );
    }

    #[test]
    fn test_getting_setting() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let timestamp = Utc::now();
        let response_status = set_setting_request(&client, "test_key", json!("test_value"));
        assert_eq!(response_status, rocket::http::Status::Created);

        // Test getting
        let res = client
            .get("/api/0/settings/test_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new("settings.test_key", "test_value", Utc::now()),
        );
    }

    #[test]
    fn test_getting_setting_multiple_types() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let timestamp = Utc::now();

        // Test array
        let response_status = set_setting_request(&client, "test_key_array", json!("[1,2,3]"));
        assert_eq!(response_status, rocket::http::Status::Created);

        let res = client
            .get("/api/0/settings/test_key_array")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new("settings.test_key_array", "[1,2,3]", Utc::now()),
        );

        // Test dict
        let response_status = set_setting_request(
            &client,
            "test_key_dict",
            json!("{key: 'value', another_key: 'another value'}"),
        );
        assert_eq!(response_status, rocket::http::Status::Created);

        let res = client
            .get("/api/0/settings/test_key_dict")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new(
                "settings.test_key_dict",
                "{key: 'value', another_key: 'another value'}",
                Utc::now(),
            ),
        );
    }

    #[test]
    fn test_updating_setting() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let timestamp = Utc::now();
        let post_1_status = set_setting_request(&client, "test_key", json!("test_value"));
        assert_eq!(post_1_status, rocket::http::Status::Created);

        let res = client
            .get("/api/0/settings/test_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let deserialized: KeyValue = serde_json::from_str(&res.into_string().unwrap()).unwrap();

        _equal_and_timestamp_in_range(
            timestamp,
            deserialized,
            KeyValue::new("settings.test_key", "test_value", Utc::now()),
        );

        let timestamp_2 = Utc::now();
        let post_2_status = set_setting_request(&client, "test_key", json!("changed_test_value"));
        assert_eq!(post_2_status, rocket::http::Status::Created);

        let res = client
            .get("/api/0/settings/test_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        let new_deserialized: KeyValue = serde_json::from_str(&res.into_string().unwrap()).unwrap();
        _equal_and_timestamp_in_range(
            timestamp_2,
            new_deserialized,
            KeyValue::new("settings.test_key", "changed_test_value", Utc::now()),
        );
    }

    #[test]
    fn test_deleting_setting() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let response_status = set_setting_request(&client, "test_key", json!(""));
        assert_eq!(response_status, rocket::http::Status::Created);

        // Test deleting
        let res = client
            .delete("/api/0/settings/test_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        let res = client
            .get("/api/0/settings/test_key")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::NotFound);
    }

    #[test]
    fn test_cors_catching() {
        let server = setup_testserver();
        let client = Client::untracked(server).expect("valid instance");

        let res = client
            .options("/api/0/buckets/")
            .header(ContentType::JSON)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
    }
}
