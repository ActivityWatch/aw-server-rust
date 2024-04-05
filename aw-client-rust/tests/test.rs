extern crate aw_client_rust;
extern crate aw_datastore;
extern crate aw_server;
extern crate chrono;
extern crate rocket;
extern crate serde_json;
extern crate tokio_test;

#[cfg(test)]
mod test {
    use aw_client_rust::blocking::AwClient;
    use aw_client_rust::Event;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::Map;
    use std::sync::Mutex;
    use std::thread;
    use tokio_test::block_on;

    // A random port, but still not guaranteed to not be bound
    // FIXME: Bind to a port that is free for certain and use that for the client instead
    static PORT: u16 = 41293;

    fn wait_for_server(timeout_s: u32, client: &AwClient) {
        for i in 0.. {
            match client.get_info() {
                Ok(_) => break,
                Err(err) => {
                    if i == timeout_s - 1 {
                        panic!("Timed out starting aw-server after {timeout_s}s: {err:?}");
                    }
                }
            }
            use std::time;
            let duration = time::Duration::from_secs(1);
            thread::sleep(duration);
        }
    }

    fn setup_testserver() -> rocket::Shutdown {
        use aw_server::endpoints::AssetResolver;
        use aw_server::endpoints::ServerState;

        let state = ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_resolver: AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = aw_server::config::AWConfig::default();
        aw_config.port = PORT;
        let server = aw_server::endpoints::build_rocket(state, aw_config);
        let server = block_on(server.ignite()).unwrap();
        let shutdown_handler = server.shutdown();

        thread::spawn(move || {
            let _ = block_on(server.launch()).unwrap();
        });

        shutdown_handler
    }

    #[test]
    fn test_full() {
        let clientname = "aw-client-rust-test";

        let client: AwClient =
            AwClient::new("127.0.0.1", PORT, clientname).expect("Client creation failed");

        let shutdown_handler = setup_testserver();

        wait_for_server(20, &client);

        let info = client.get_info().unwrap();
        assert!(info.testing);

        let bucketname = format!("aw-client-rust-test_{}", client.hostname);
        let buckettype = "test-type";
        client
            .create_bucket_simple(&bucketname, buckettype)
            .unwrap();

        let bucket = client.get_bucket(&bucketname).unwrap();
        assert!(bucket.id == bucketname);
        println!("{}", bucket.id);

        let buckets = client.get_buckets().unwrap();
        println!("Buckets: {buckets:?}");
        let mut event = Event {
            id: None,
            timestamp: DateTime::from_utc(
                DateTime::parse_from_rfc3339("2017-12-30T01:00:00+00:00")
                    .unwrap()
                    .naive_utc(),
                Utc,
            ),
            duration: Duration::seconds(0),
            data: Map::new(),
        };
        println!("{event:?}");
        client.insert_event(&bucketname, &event).unwrap();
        // Ugly way to create a UTC from timestamp, see https://github.com/chronotope/chrono/issues/263
        event.timestamp = DateTime::from_utc(
            DateTime::parse_from_rfc3339("2017-12-30T01:00:01+00:00")
                .unwrap()
                .naive_utc(),
            Utc,
        );
        client.heartbeat(&bucketname, &event, 10.0).unwrap();

        let events = client.get_events(&bucketname, None, None, None).unwrap();
        println!("Events: {events:?}");
        assert!(events[0].duration == Duration::seconds(1));

        // Query
        let query = format!(
            "events = query_bucket(\"{}\");
RETURN = events;",
            bucket.id
        );
        let start: DateTime<Utc> = DateTime::parse_from_rfc3339("1996-12-19T00:00:00-08:00")
            .unwrap()
            .into();
        let end: DateTime<Utc> = DateTime::parse_from_rfc3339("2020-12-19T00:00:00-08:00")
            .unwrap()
            .into();
        let timeperiods = (start, end);
        let query_result = client.query(&query, vec![timeperiods]).unwrap();
        println!("Query result: {query_result:?}");

        client
            .delete_event(&bucketname, events[0].id.unwrap())
            .unwrap();

        let count = client.get_event_count(&bucketname).unwrap();
        assert_eq!(count, 0);

        client.delete_bucket(&bucketname).unwrap();

        shutdown_handler.notify();
    }
}
