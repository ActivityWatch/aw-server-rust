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
    use std::cell::RefCell;
    use std::fs;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio_test::block_on;

    // A random port, but still not guaranteed to not be bound
    // FIXME: Bind to a port that is free for certain and use that for the client instead
    static PORT: u16 = 41293;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // Keep the listener alive until the server binds — prevents TOCTOU race in reserve_port
    thread_local! {
        static RESERVED_PORT: RefCell<Option<TcpListener>> = RefCell::new(None);
    }

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

    fn setup_testserver(port: u16, api_key: Option<&str>) -> rocket::Shutdown {
        use aw_server::endpoints::AssetResolver;
        use aw_server::endpoints::ServerState;

        let state = ServerState {
            datastore: Mutex::new(aw_datastore::Datastore::new_in_memory(false)),
            asset_resolver: AssetResolver::new(None),
            device_id: "test_id".to_string(),
        };
        let mut aw_config = aw_server::config::AWConfig::default();
        aw_config.port = port;
        aw_config.auth.api_key = api_key.map(str::to_owned);
        let server = aw_server::endpoints::build_rocket(state, aw_config);
        let server = block_on(server.ignite()).unwrap();
        let shutdown_handler = server.shutdown();

        thread::spawn(move || {
            let _ = block_on(server.launch()).unwrap();
        });

        shutdown_handler
    }

    fn reserve_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Keep the listener alive until the server binds — prevents TOCTOU race
        RESERVED_PORT.with(|cell| *cell.borrow_mut() = Some(listener));
        port
    }

    fn write_server_config(port: u16, api_key: Option<&str>) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_home = std::env::temp_dir().join(format!(
            "aw-client-rust-config-{}-{}",
            std::process::id(),
            unique
        ));
        let config_dir = config_home.join("activitywatch").join("aw-server-rust");
        fs::create_dir_all(&config_dir).unwrap();

        let mut content = format!("port = {port}\n");
        if let Some(api_key) = api_key {
            content.push_str("\n[auth]\n");
            content.push_str(&format!("api_key = \"{api_key}\"\n"));
        }
        fs::write(config_dir.join("config.toml"), content).unwrap();

        config_home
    }

    fn with_config_home<T>(config_home: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_value = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", config_home);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Some(old_value) = old_value {
            std::env::set_var("XDG_CONFIG_HOME", old_value);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = fs::remove_dir_all(config_home);
        result.unwrap()
    }

    #[test]
    fn test_full() {
        let clientname = "aw-client-rust-test";

        // Hold ENV_LOCK during client creation to prevent parallel-test interference
        // with test_reads_api_key_from_matching_server_config (which holds the lock
        // via with_config_home for the entire client+server lifetime).
        let client: AwClient = {
            let _guard = ENV_LOCK.lock().unwrap();
            AwClient::new("127.0.0.1", PORT, clientname).expect("Client creation failed")
        };

        let shutdown_handler = setup_testserver(PORT, None);

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

    // XDG_CONFIG_HOME is only respected by dirs::config_dir() on Linux.
    // On macOS it returns $HOME/Library/Application Support (ignoring XDG_CONFIG_HOME),
    // so this test would fail — gate it on Linux only.
    #[test]
    #[cfg(target_os = "linux")]
    fn test_reads_api_key_from_matching_server_config() {
        let clientname = "aw-client-rust-auth-test";
        let port = reserve_port();
        let config_home = write_server_config(port, Some("secret123"));

        with_config_home(&config_home, || {
            let client: AwClient =
                AwClient::new("127.0.0.1", port, clientname).expect("Client creation failed");
            // Drop the reserved listener before Rocket tries to bind the same port.
            RESERVED_PORT.with(|cell| *cell.borrow_mut() = None);
            let shutdown_handler = setup_testserver(port, Some("secret123"));

            wait_for_server(20, &client);

            let bucketname = format!("aw-client-rust-auth-test_{}", client.hostname);
            client
                .create_bucket_simple(&bucketname, "test-type")
                .unwrap();

            let bucket = client.get_bucket(&bucketname).unwrap();
            assert_eq!(bucket.id, bucketname);

            shutdown_handler.notify();
        });
    }
}
