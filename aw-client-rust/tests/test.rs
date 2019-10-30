extern crate aw_client_rust;
extern crate serde_json;
extern crate chrono;

#[cfg(test)]
mod test {
    use aw_client_rust::AwClient;
    use aw_client_rust::Event;
    use serde_json::Map;
    use chrono::{DateTime, Utc, Duration};

    #[test]
    fn test_full() {
        let ip = "127.0.0.1";
        let port = "5666";
        let clientname = "aw-client-rust-test";
        let client : AwClient = AwClient::new(ip, port, clientname);

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
            timestamp: DateTime::from_utc(DateTime::parse_from_rfc3339("2017-12-30T01:00:00+00:00").unwrap().naive_utc(), Utc),
            duration: Duration::seconds(0),
            data: Map::new()
        };
        println!("{:?}", event);
        client.insert_event(&bucketname, &event).unwrap();
        // Ugly way to create a UTC from timestamp, see https://github.com/chronotope/chrono/issues/263
        event.timestamp = DateTime::from_utc(DateTime::parse_from_rfc3339("2017-12-30T01:00:01+00:00").unwrap().naive_utc(), Utc);
        client.heartbeat(&bucketname, &event, 10.0).unwrap();

        let events = client.get_events(&bucketname).unwrap();
        println!("Events: {:?}", events);
        assert!(events[0].duration == Duration::seconds(1));

        client.delete_bucket(&bucketname).unwrap();

        // Uncomment to see stdout from "cargo test"
        //assert!(1==2);
    }
}
