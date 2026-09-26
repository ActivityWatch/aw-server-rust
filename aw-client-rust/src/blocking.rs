use std::future::Future;
use std::{collections::HashMap, error::Error};

use chrono::{DateTime, Utc};

use aw_models::{Bucket, Event};

use super::AwClient as AsyncAwClient;

pub struct AwClient {
    client: AsyncAwClient,
    pub baseurl: reqwest::Url,
    pub name: String,
    pub hostname: String,
}

impl std::fmt::Debug for AwClient {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "AwClient(baseurl={:?})", self.client.baseurl)
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build shell runtime")
        .block_on(f)
}

macro_rules! proxy_method
{
    ($name:tt, $ret:ty, $($v:ident: $t:ty),*) => {
        pub fn $name(&self, $($v: $t),*) -> Result<$ret, reqwest::Error>
        { block_on(self.client.$name($($v),*)) }
    };
}

impl AwClient {
    pub fn new(host: &str, port: u16, name: &str) -> Result<AwClient, Box<dyn Error>> {
        Self::new_with_api_key(host, port, name, None)
    }

    pub fn from_config(
        name: &str,
        testing: bool,
        api_key: Option<String>,
    ) -> Result<AwClient, Box<dyn Error>> {
        let config = crate::config::load_config(testing);
        Self::new_with_api_key(&config.hostname, config.port, name, api_key)
    }

    pub fn new_with_api_key(
        host: &str,
        port: u16,
        name: &str,
        api_key: Option<String>,
    ) -> Result<AwClient, Box<dyn Error>> {
        let async_client = AsyncAwClient::new_with_api_key(host, port, name, api_key)?;

        Ok(AwClient {
            baseurl: async_client.baseurl.clone(),
            name: async_client.name.clone(),
            hostname: async_client.hostname.clone(),
            client: async_client,
        })
    }

    proxy_method!(get_bucket, Bucket, bucketname: &str);
    proxy_method!(get_buckets, HashMap<String, Bucket>,);
    proxy_method!(create_bucket, (), bucket: &Bucket);
    proxy_method!(create_bucket_simple, (), bucketname: &str, buckettype: &str);
    proxy_method!(delete_bucket, (), bucketname: &str);
    proxy_method!(
        get_events,
        Vec<Event>,
        bucketname: &str,
        start: Option<DateTime<Utc>>,
        stop: Option<DateTime<Utc>>,
        limit: Option<u64>
    );
    proxy_method!(
        query,
        Vec<serde_json::Value>,
        query: &str,
        timeperiods: Vec<(DateTime<Utc>, DateTime<Utc>)>
    );
    proxy_method!(get_event, Option<Event>, bucketname: &str, event_id: i64);
    proxy_method!(insert_event, (), bucketname: &str, event: &Event);
    proxy_method!(insert_events, (), bucketname: &str, events: Vec<Event>);
    proxy_method!(
        heartbeat,
        (),
        bucketname: &str,
        event: &Event,
        pulsetime: f64
    );
    proxy_method!(delete_event, (), bucketname: &str, event_id: i64);
    proxy_method!(get_event_count, i64, bucketname: &str);
    proxy_method!(get_info, aw_models::Info,);
    proxy_method!(get_setting, serde_json::Value, setting: &str);
    proxy_method!(
        set_setting,
        (),
        setting: &str,
        value: &serde_json::Value
    );
    proxy_method!(get_settings, aw_models::Settings,);

    pub fn request_queue(&self, testing: bool) -> std::io::Result<crate::queue::RequestQueue> {
        self.client.request_queue(testing)
    }

    pub fn request_queue_at(
        &self,
        path: std::path::PathBuf,
    ) -> std::io::Result<crate::queue::RequestQueue> {
        self.client.request_queue_at(path)
    }

    pub fn wait_for_start(&self) -> Result<(), Box<dyn Error>> {
        block_on(self.client.wait_for_start())
    }
}

#[test]
fn test_wait_for_start_blocking_wrapper() {
    use std::io::{BufRead, BufReader, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server =
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            stream
            .write_all(concat!(
                "HTTP/1.1 200 OK\r\nContent-Length: 74\r\nConnection: close\r\n\r\n",
                r#"{"hostname":"host","version":"v0.0.0","testing":true,"device_id":"device"}"#
            ).as_bytes())
            .unwrap();
        });
    let client = AwClient::new("127.0.0.1", port, "test-wait-for-start-blocking").unwrap();
    client.wait_for_start().unwrap();
    server.join().unwrap();
}
