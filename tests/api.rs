#[macro_use]
extern crate log;

extern crate rocket;

extern crate aw_server;

#[cfg(test)]
mod api_tests {
    use std::path::{PathBuf};
    use rocket::http::ContentType;

    use aw_server::datastore;
    use aw_server::endpoints;

    fn setup_testserver() -> rocket::Rocket {
        let state = endpoints::ServerState {
            datastore: datastore::Datastore::new_in_memory(),
            asset_path: PathBuf::from("aw-webui/dist"),
        };
        endpoints::rocket(state, None)
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
    fn test_import() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Import single bucket
        let mut res = client.post("/api/0/import")
            .header(ContentType::JSON)
            .body(r#"{
                "id": "id1",
                "type": "type",
                "client": "client",
                "hostname": "hostname",
                "events": [{
                    "timestamp":"2000-01-01T00:00:00.000000+00:00",
                    "duration":1.0,
                    "data": {}
                }]
            }"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get created bucket
        res = client.get("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Import multiple buckets
        let mut res = client.post("/api/0/import")
            .header(ContentType::JSON)
            .body(r#"{"id2": {
                "id": "id2",
                "type": "type",
                "client": "client",
                "hostname": "hostname",
                "events": [{
                    "timestamp":"2000-01-01T00:00:00Z",
                    "duration":1.0,
                    "data": {}
                }]
            }}"#)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get created bucket
        res = client.get("/api/0/buckets/id1")
            .header(ContentType::JSON)
            .dispatch();
        debug!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
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
