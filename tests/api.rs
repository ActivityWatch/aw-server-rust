extern crate rocket;

extern crate aw_server;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rocket::http::ContentType;

    use aw_server::datastore;
    use aw_server::endpoints;

    fn setup_testserver() -> rocket::Rocket {
        let state = endpoints::ServerState {
            datastore: Mutex::new(datastore::Datastore::new_in_memory())
        };
        endpoints::rocket(state)
    }

    #[test]
    fn test_bucket() {
        let server = setup_testserver();
        let client = rocket::local::Client::new(server).expect("valid instance");

        // Get empty list of buckets
        let mut res = client.get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try to fetch non-existing bucket
        res = client.get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
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
        println!("{:?}", res.body_string());
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
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotModified);

        // Get list of buckets (1 bucket)
        res = client.get("/api/0/buckets/")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get newly created bucket
        res = client.get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get non-existing bucket
        res = client.get("/api/0/buckets/invalid_bucket")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::NotFound);

        // Delete bucket
        res = client.delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Try (and fail) to get deleted bucket
        res = client.get("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
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
        println!("{:?}", res.body_string());
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
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get inserted event
        res = client.get("/api/0/buckets/id/events?")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[{"data":{},"duration":1.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get inserted event
        res = client.get("/api/0/buckets/id/events?")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[{"data":{},"duration":1.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]"#);
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
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get heartbeat event
        res = client.get("/api/0/buckets/id/events?")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"[{"data":{},"duration":2.0,"id":1,"timestamp":"2018-01-01T01:01:01Z"}]"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Get eventcount
        res = client.get("/api/0/buckets/id/events/count")
            .header(ContentType::JSON)
            .dispatch();
        assert_eq!(res.body_string().unwrap(), r#"{"count":1}"#);
        assert_eq!(res.status(), rocket::http::Status::Ok);

        // Delete bucket
        res = client.delete("/api/0/buckets/id")
            .header(ContentType::JSON)
            .dispatch();
        println!("{:?}", res.body_string());
        assert_eq!(res.status(), rocket::http::Status::Ok);
    }
}
