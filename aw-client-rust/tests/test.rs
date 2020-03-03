extern crate aw_client_rust;
extern crate aw_datastore;
extern crate aw_server;
extern crate chrono;
extern crate serde_json;

#[cfg(test)]
mod test {
    use aw_client_rust::AwClient;
    use aw_client_rust::Event;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::Map;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::thread;

    // A random port, but still not guaranteed to not be bound
    // FIXME: Bind to a port that is free for certain and use that for the client instead
    static PORT: u16 = 41293;

    fn wait_for_server(timeout_s: u32, client: &AwClient) -> () {
        for i in 0.. {
            match client.get_info() {
                Ok(_) => break,
                Err(err) => {
                    if i == timeout_s-1 {
                        panic!(
                            "Timed out starting aw-server after {}s: {:?}",
                            timeout_s, err
                        );
                    }
                }
            }
            use std::time;
            let duration = time::Duration::from_secs(1);
            thread::sleep(duration);
        }
    }

    fn setup_testserver() -> () {
        // Start testserver and wait 10s for it to start up
        // TODO: Properly shutdown
        use aw_server::endpoints::ServerState;
        let state = ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_path: PathBuf::from("."), // webui won't be used, so it's invalidly set
        };
        let mut aw_config = aw_server::config::AWConfig::default();
        aw_config.port = PORT;
        let server = aw_server::endpoints::build_rocket(state, &aw_config);

        thread::spawn(move || {
            server.launch();
        });
    }

    #[test]
    fn test_full() {
        let ip = "127.0.0.1";
        let port: String = PORT.to_string();
        let clientname = "aw-client-rust-test";
        let client: AwClient = AwClient::new(ip, &port, clientname);

        setup_testserver();

        wait_for_server(20, &client);

        let info = client.get_info().unwrap();
        assert!(info.testing == true);

        let bucketname = format!("aw-client-rust-test_{}", client.hostname);
        let buckettype = "test-type";
        client.create_bucket(&bucketname, &buckettype).unwrap();

        let bucket = client.get_bucket(&bucketname).unwrap();
        assert!(bucket.id == bucketname);
        println!("{}", bucket.id);

        let buckets = client.get_buckets().unwrap();
        println!("Buckets: {:?}", buckets);
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
        println!("{:?}", event);
        client.insert_event(&bucketname, &event).unwrap();
        // Ugly way to create a UTC from timestamp, see https://github.com/chronotope/chrono/issues/263
        event.timestamp = DateTime::from_utc(
            DateTime::parse_from_rfc3339("2017-12-30T01:00:01+00:00")
                .unwrap()
                .naive_utc(),
            Utc,
        );
        client.heartbeat(&bucketname, &event, 10.0).unwrap();

        let events = client.get_events(&bucketname).unwrap();
        println!("Events: {:?}", events);
        assert!(events[0].duration == Duration::seconds(1));

        client
            .delete_event(&bucketname, events[0].id.unwrap())
            .unwrap();

        let count = client.get_event_count(&bucketname).unwrap();
        assert_eq!(count, 0);

        client.delete_bucket(&bucketname).unwrap();
    }
}
