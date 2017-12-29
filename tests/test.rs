extern crate aw_client_rust;

#[cfg(test)]
mod test {
    use aw_client_rust::AwClient;

    #[test]
    fn test_full() {
        let ip = String::from("127.0.0.1");
        let port = String::from("5666");
        let clientname = String::from("aw-client-rust-test");
        let mut client : AwClient = AwClient::new(ip, port, clientname);

        let info = AwClient::get_info(&mut client);
        assert!(info.testing == true);

        let bucketname = format!("aw-client-rust-test_{}", client.hostname);
        let buckettype = String::from("test-type");
        AwClient::create_bucket(&mut client, &bucketname, &buckettype);

        let bucket = AwClient::get_bucket(&mut client, &bucketname);
        assert!(bucket.id == bucketname);
        println!("{}", bucket.id);

        // TODO:
        // Insert events to bucket
        // Fetch events from bucket
        // Delete bucket
    }
}
