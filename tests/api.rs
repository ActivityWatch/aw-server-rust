#[macro_use]
extern crate log;

extern crate rocket;

extern crate aw_server;

#[cfg(test)]
mod api_tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use rocket::http::Header;
    use rocket::http::ContentType;

    use aw_server::config;
    use aw_server::datastore;
    use aw_server::endpoints;

    use aw_server::models::BucketsExport;

    fn setup_testserver() -> rocket::Rocket {
        let state = endpoints::ServerState {
            datastore: Mutex::new(datastore::Datastore::new_in_memory()),
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
        let mut res = client.get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try to fetch non-existing bucket
        res = client.get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Create bucket
        res = client.post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try to create bucket which already exists
        res = client.post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotModified);

        // Get list of buckets (1 bucket)
        res = client.get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        // TODO: assert data
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get newly created bucket
        res = client.get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get non-existing bucket
        res = client.get("/api/0/buckets/invalid_bucket")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Delete bucket
        res = client.delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try (and fail) to get deleted bucket
        res = client.get("/api/0/buckets/id")
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
        let mut res = client.post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Insert a single event
        res = client.post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .body(r#"[{
                "timestamp": "2018-01-01T01:01:01Z",
                "duration": 1.0,
                "data": {}
            }]"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get inserted event
        res = client.get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":1.0,"data":{}}]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Heartbeat
        res = client.post("/api/0/buckets/id/heartbeat?pulsetime=2")
            .header(ContentType::JSON)
            .body(r#"{
                "timestamp": "2018-01-01T01:01:02Z",
                "duration": 1.0,
                "data": {}
            }"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get heartbeat event
        res = client.get("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[{"id":1,"timestamp":"2018-01-01T01:01:01Z","duration":2.0,"data":{}}]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get eventcount
        res = client.get("/api/0/buckets/id/events/count")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), "1");
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Delete bucket
        res = client.delete("/api/0/buckets/id")
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
        let mut res = client.post("/api/0/import")
            .header(ContentType::JSON)
            .body(r#"{"buckets":
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
            }}}"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Export single created bucket
        let mut res = client.get("/api/0/buckets/id1/export")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export : BucketsExport = serde_json::from_str(&res.body_string().unwrap()).unwrap();

        // Delete bucket so we can import it again
        res = client.delete("/api/0/buckets/id1")
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
        let mut res = client.post("/api/0/import")
            .header(Header::new("Content-Type", "multipart/form-data; boundary=a"))
            .body(&sum[..])
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get created bucket
        let mut res = client.get("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Export all buckets
        let mut res = client.get("/api/0/export")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
        let export : BucketsExport = serde_json::from_str(&res.body_string().unwrap()).unwrap();
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
        let mut res = client.post("/api/0/query")
            .header(ContentType::JSON)
            .body(r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return 1;"]
            }"#)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[1.0]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Create bucket to query later
        let mut res = client.post("/api/0/buckets/id")
            .header(ContentType::JSON)
            .body(r#"{
                "id": "id",
                "type": "type",
                "client": "client",
                "hostname": "hostname"
            }"#)
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Insert a event to query later
        res = client.post("/api/0/buckets/id/events")
            .header(ContentType::JSON)
            .body(r#"[{
                "timestamp": "2018-01-01T01:01:01Z",
                "duration": 1.0,
                "data": {}
            }]"#)
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Query events
        let mut res = client.post("/api/0/query")
            .header(ContentType::JSON)
            .body(r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": ["return query_bucket(\"id\");"]
            }"#)
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::Ok);
        assert_eq!(res.body_string().unwrap(), r#"[[{"data":{},"duration":1.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]]"#);

        // Test error
        let mut res = client.post("/api/0/query")
            .header(ContentType::JSON)
            .body(r#"{
                "timeperiods": ["2000-01-01T00:00:00Z/2020-01-01T00:00:00Z"],
                "query": [""]
            }"#)
            .dispatch();
        assert_eq!(res.status(), rocket::http::Status::InternalServerError);
        assert_eq!(res.body_string().unwrap(), r#"{"message":"EmptyQuery","reason":"Internal Server Error (Query Error)","status":500}"#);
    }

}
