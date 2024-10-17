extern crate aw_models;
extern crate chrono;
extern crate gethostname;
extern crate reqwest;
extern crate serde_json;
extern crate tokio;

pub mod blocking;

use std::{collections::HashMap, error::Error};

use chrono::{DateTime, Utc};
use serde_json::{json, Map};
use std::net::TcpStream;
use std::time::Duration;

pub use aw_models::{Bucket, BucketMetadata, Event};

pub struct AwClient {
    client: reqwest::Client,
    pub baseurl: reqwest::Url,
    pub name: String,
    pub hostname: String,
}

impl std::fmt::Debug for AwClient {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "AwClient(baseurl={:?})", self.baseurl)
    }
}

fn get_hostname() -> String {
    return gethostname::gethostname().to_string_lossy().to_string();
}

impl AwClient {
    pub fn new(host: &str, port: u16, name: &str) -> Result<AwClient, Box<dyn Error>> {
        let baseurl = reqwest::Url::parse(&format!("http://{}:{}", host, port))?;
        let hostname = get_hostname();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        Ok(AwClient {
            client,
            baseurl,
            name: name.to_string(),
            hostname,
        })
    }

    pub async fn get_bucket(&self, bucketname: &str) -> Result<Bucket, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucketname);
        let bucket = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(bucket)
    }

    pub async fn get_buckets(&self) -> Result<HashMap<String, Bucket>, reqwest::Error> {
        let url = format!("{}/api/0/buckets/", self.baseurl);
        self.client.get(url).send().await?.json().await
    }

    pub async fn create_bucket(&self, bucket: &Bucket) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucket.id);
        self.client.post(url).json(bucket).send().await?;
        Ok(())
    }

    pub async fn create_bucket_simple(
        &self,
        bucketname: &str,
        buckettype: &str,
    ) -> Result<(), reqwest::Error> {
        let bucket = Bucket {
            bid: None,
            id: bucketname.to_string(),
            client: self.name.clone(),
            _type: buckettype.to_string(),
            hostname: self.hostname.clone(),
            data: Map::default(),
            metadata: BucketMetadata::default(),
            events: None,
            created: None,
            last_updated: None,
        };
        self.create_bucket(&bucket).await
    }

    pub async fn delete_bucket(&self, bucketname: &str) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucketname);
        self.client.delete(url).send().await?;
        Ok(())
    }

    pub async fn query(
        &self,
        query: &str,
        timeperiods: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    ) -> Result<Vec<serde_json::Value>, reqwest::Error> {
        let url = reqwest::Url::parse(format!("{}/api/0/query", self.baseurl).as_str()).unwrap();

        // Format timeperiods as ISO8601 strings, separated by /
        let timeperiods_str: Vec<String> = timeperiods
            .iter()
            .map(|(start, stop)| (start.to_rfc3339(), stop.to_rfc3339()))
            .map(|(start, stop)| format!("{}/{}", start, stop))
            .collect();

        // Result is a sequence, one element per timeperiod
        self.client
            .post(url)
            .json(&json!({
                "query": query.split('\n').collect::<Vec<&str>>(),
                "timeperiods": timeperiods_str,
            }))
            .send()
            .await?
            .json()
            .await
    }

    pub async fn get_events(
        &self,
        bucketname: &str,
        start: Option<DateTime<Utc>>,
        stop: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, reqwest::Error> {
        let mut url = reqwest::Url::parse(
            format!("{}/api/0/buckets/{}/events", self.baseurl, bucketname).as_str(),
        )
        .unwrap();

        // Must be a better way to build URLs
        if let Some(s) = start {
            url.query_pairs_mut()
                .append_pair("start", s.to_rfc3339().as_str());
        };
        if let Some(s) = stop {
            url.query_pairs_mut()
                .append_pair("end", s.to_rfc3339().as_str());
        };
        if let Some(s) = limit {
            url.query_pairs_mut()
                .append_pair("limit", s.to_string().as_str());
        };
        self.client.get(url).send().await?.json().await
    }

    pub async fn insert_event(
        &self,
        bucketname: &str,
        event: &Event,
    ) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", self.baseurl, bucketname);
        let eventlist = vec![event.clone()];
        self.client.post(url).json(&eventlist).send().await?;
        Ok(())
    }

    pub async fn insert_events(
        &self,
        bucketname: &str,
        events: Vec<Event>,
    ) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", self.baseurl, bucketname);
        self.client.post(url).json(&events).send().await?;
        Ok(())
    }

    pub async fn heartbeat(
        &self,
        bucketname: &str,
        event: &Event,
        pulsetime: f64,
    ) -> Result<(), reqwest::Error> {
        let url = format!(
            "{}/api/0/buckets/{}/heartbeat?pulsetime={}",
            self.baseurl, bucketname, pulsetime
        );
        self.client.post(url).json(&event).send().await?;
        Ok(())
    }

    pub async fn delete_event(
        &self,
        bucketname: &str,
        event_id: i64,
    ) -> Result<(), reqwest::Error> {
        let url = format!(
            "{}/api/0/buckets/{}/events/{}",
            self.baseurl, bucketname, event_id
        );
        self.client.delete(url).send().await?;
        Ok(())
    }

    pub async fn get_event_count(&self, bucketname: &str) -> Result<i64, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events/count", self.baseurl, bucketname);
        let res = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let count: i64 = match res.trim().parse() {
            Ok(count) => count,
            Err(err) => panic!("could not parse get_event_count response: {err:?}"),
        };
        Ok(count)
    }

    pub async fn get_info(&self) -> Result<aw_models::Info, reqwest::Error> {
        let url = format!("{}/api/0/info", self.baseurl);
        self.client.get(url).send().await?.json().await
    }

    // TODO: make async
    pub fn wait_for_start(&self) -> Result<(), Box<dyn Error>> {
        let socket_addrs = self.baseurl.socket_addrs(|| None)?;
        let socket_addr = socket_addrs
            .first()
            .ok_or("Unable to resolve baseurl into socket address")?;

        // Check if server is running with exponential backoff
        let mut retry_delay = Duration::from_millis(100);
        let max_wait = Duration::from_secs(10);
        let mut total_wait = Duration::from_secs(0);

        while total_wait < max_wait {
            match TcpStream::connect_timeout(socket_addr, retry_delay) {
                Ok(_) => break,
                Err(_) => {
                    std::thread::sleep(retry_delay);
                    total_wait += retry_delay;
                    retry_delay *= 2;
                }
            }
        }

        if total_wait >= max_wait {
            return Err(format!(
                "Local server {} not running after 10 seconds of retrying",
                socket_addr
            )
            .into());
        }

        Ok(())
    }
}
