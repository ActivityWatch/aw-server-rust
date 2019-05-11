extern crate aw_client_rust;
extern crate serde_json;

#[cfg(test)]
mod test {
    use aw_client_rust::AwClient;
    use aw_client_rust::Event;
    use serde_json::Map;

    #[test]
    fn test_full() {
        let ip = String::from("127.0.0.1");
        let port = String::from("5666");
        let clientname = String::from("aw-client-rust-test");
        let mut client : AwClient = AwClient::new(ip, port, clientname);

        let info = AwClient::get_info(&mut client).unwrap();
        assert!(info.testing == true);

        let bucketname = format!("aw-client-rust-test_{}", client.hostname);
        let buckettype = String::from("test-type");
        AwClient::create_bucket(&mut client, &bucketname, &buckettype).unwrap();

        let bucket = AwClient::get_bucket(&mut client, &bucketname).unwrap();
        assert!(bucket.id == bucketname);
        println!("{}", bucket.id);

        let buckets = AwClient::get_buckets(&mut client).unwrap();
        println!("Buckets: {:?}", buckets);
        let event = Event {
            id: None,
            timestamp: String::from("2017-12-30T01:00:00+00:00"),
            duration: 1.0,
            data: Map::new()
        };
        println!("{:?}", event);
        AwClient::insert_event(&mut client, &bucketname, &event).unwrap();

        let events = AwClient::get_events(&mut client, &bucketname).unwrap();
        println!("Events: {:?}", events);

        AwClient::delete_bucket(&mut client, &bucketname).unwrap();

        // Uncomment to see stdout from "cargo test"
        // assert!(1==2);
    }
}
