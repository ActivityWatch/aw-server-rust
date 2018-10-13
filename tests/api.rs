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
            datastore: Mutex::new(datastore::DatastoreInstance::new_in_memory())
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
    }
}
