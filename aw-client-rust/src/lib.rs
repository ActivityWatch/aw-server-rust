extern crate aw_models;
extern crate chrono;
extern crate gethostname;
extern crate reqwest;
extern crate serde_json;
extern crate tokio;

pub mod blocking;
pub mod classes;
pub mod config;
pub mod queries;
pub mod queue;
pub mod single_instance;

use std::{collections::HashMap, error::Error};

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Map};
use single_instance::SingleInstance;
use std::time::Duration;

pub use aw_models::{Bucket, BucketMetadata, Event};

pub struct AwClient {
    client: reqwest::Client,
    #[allow(dead_code)]
    single_instance: SingleInstance,
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
    gethostname::gethostname().to_string_lossy().to_string()
}

fn build_client(api_key: Option<String>) -> Result<reqwest::Client, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    if let Some(api_key) = api_key {
        let mut header_value = HeaderValue::from_str(&format!("Bearer {api_key}"))?;
        header_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, header_value);
    }

    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .default_headers(headers)
        .build()?)
}

/// Build the name used for the single-instance lock.
///
/// `localhost` is normalized to `127.0.0.1` so that a client given either spelling
/// takes the same lock, and so the name matches aw-client-python, which defaults
/// its server hostname to `127.0.0.1`.
fn single_instance_name(name: &str, host: &str, port: u16) -> String {
    let host = if host.eq_ignore_ascii_case("localhost") {
        "127.0.0.1"
    } else {
        host
    };
    format!("{}-at-{}-on-{}", name, host, port)
}

impl AwClient {
    async fn send_success(
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, reqwest::Error> {
        request.send().await?.error_for_status()
    }

    /// Build an API URL from path segments, percent-encoding each one so bucket IDs
    /// and setting keys containing `/`, `?` or `#` stay a single segment.
    fn api_url(&self, segments: &[&str]) -> reqwest::Url {
        let mut url = self.baseurl.clone();
        // A base URL without path segments (e.g. `data:`) is never http(s), so leave it
        // as is and let reqwest reject it when the request is sent.
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(["api", "0"]).extend(segments);
        }
        url
    }

    pub fn new(host: &str, port: u16, name: &str) -> Result<AwClient, Box<dyn Error>> {
        Self::new_with_api_key(host, port, name, None)
    }

    /// Connect to the server set in `aw-client.toml` (see [`config`]), the file the Python
    /// client reads. `testing` selects the `[server-testing]` section, whose default port
    /// is 5666.
    pub fn from_config(
        name: &str,
        testing: bool,
        api_key: Option<String>,
    ) -> Result<AwClient, Box<dyn Error>> {
        let config = config::load_config(testing);
        Self::new_with_api_key(&config.hostname, config.port, name, api_key)
    }

    pub fn new_with_api_key(
        host: &str,
        port: u16,
        name: &str,
        api_key: Option<String>,
    ) -> Result<AwClient, Box<dyn Error>> {
        let baseurl = reqwest::Url::parse(&format!("http://{}:{}", host, port))?;
        let hostname = get_hostname();
        let client = build_client(api_key)?;
        let instance_name = single_instance_name(name, host, port);
        let single_instance = single_instance::SingleInstance::new(instance_name.as_str())?;

        Ok(AwClient {
            client,
            single_instance,
            baseurl,
            name: name.to_string(),
            hostname,
        })
    }

    /// Start an offline request queue (see [`queue`]) backed by the default queue file for
    /// this client name, `testing` selecting a separate file, like `queued=True` in the
    /// Python client.
    pub fn request_queue(&self, testing: bool) -> std::io::Result<queue::RequestQueue> {
        let path = queue::default_queue_path(&self.name, testing).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no data directory for the request queue",
            )
        })?;
        let queue = self.request_queue_at(path)?;
        if testing {
            queue.set_commit_interval(queue::DEFAULT_COMMIT_INTERVAL_TESTING);
        }
        Ok(queue)
    }

    /// Like [`request_queue`](Self::request_queue), with an explicit queue file.
    pub fn request_queue_at(
        &self,
        path: std::path::PathBuf,
    ) -> std::io::Result<queue::RequestQueue> {
        queue::RequestQueue::start(
            queue::Transport {
                client: self.client.clone(),
                baseurl: self.baseurl.clone(),
                name: self.name.clone(),
                hostname: self.hostname.clone(),
            },
            path,
        )
    }

    pub async fn get_bucket(&self, bucketname: &str) -> Result<Bucket, reqwest::Error> {
        let url = format!("{}api/0/buckets/{}", self.baseurl, bucketname);
        let bucket = Self::send_success(self.client.get(url))
            .await?
            .json()
            .await?;
        Ok(bucket)
    }

    pub async fn get_buckets(&self) -> Result<HashMap<String, Bucket>, reqwest::Error> {
        let url = format!("{}api/0/buckets/", self.baseurl);
        Self::send_success(self.client.get(url)).await?.json().await
    }

    pub async fn create_bucket(&self, bucket: &Bucket) -> Result<(), reqwest::Error> {
        let url = format!("{}api/0/buckets/{}", self.baseurl, bucket.id);
        Self::send_success(self.client.post(url).json(bucket)).await?;
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
        let url = format!("{}api/0/buckets/{}", self.baseurl, bucketname);
        Self::send_success(self.client.delete(url)).await?;
        Ok(())
    }

    pub async fn query(
        &self,
        query: &str,
        timeperiods: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    ) -> Result<Vec<serde_json::Value>, reqwest::Error> {
        let url = reqwest::Url::parse(format!("{}api/0/query", self.baseurl).as_str()).unwrap();

        // Format timeperiods as ISO8601 strings, separated by /
        let timeperiods_str: Vec<String> = timeperiods
            .iter()
            .map(|(start, stop)| (start.to_rfc3339(), stop.to_rfc3339()))
            .map(|(start, stop)| format!("{}/{}", start, stop))
            .collect();

        // Result is a sequence, one element per timeperiod
        Self::send_success(self.client.post(url).json(&json!({
            "query": query.split('\n').collect::<Vec<&str>>(),
            "timeperiods": timeperiods_str,
        })))
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
            format!("{}api/0/buckets/{}/events", self.baseurl, bucketname).as_str(),
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
        Self::send_success(self.client.get(url)).await?.json().await
    }

    /// Fetch a single event by id. Returns `Ok(None)` when the server responds 404.
    pub async fn get_event(
        &self,
        bucketname: &str,
        event_id: i64,
    ) -> Result<Option<Event>, reqwest::Error> {
        let url = format!(
            "{}api/0/buckets/{}/events/{}",
            self.baseurl, bucketname, event_id
        );
        let response = self.client.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.json().await?))
    }

    pub async fn insert_event(
        &self,
        bucketname: &str,
        event: &Event,
    ) -> Result<(), reqwest::Error> {
        let url = format!("{}api/0/buckets/{}/events", self.baseurl, bucketname);
        let eventlist = vec![event.clone()];
        Self::send_success(self.client.post(url).json(&eventlist)).await?;
        Ok(())
    }

    pub async fn insert_events(
        &self,
        bucketname: &str,
        events: Vec<Event>,
    ) -> Result<(), reqwest::Error> {
        let url = format!("{}api/0/buckets/{}/events", self.baseurl, bucketname);
        Self::send_success(self.client.post(url).json(&events)).await?;
        Ok(())
    }

    pub async fn heartbeat(
        &self,
        bucketname: &str,
        event: &Event,
        pulsetime: f64,
    ) -> Result<(), reqwest::Error> {
        let url = format!(
            "{}api/0/buckets/{}/heartbeat?pulsetime={}",
            self.baseurl, bucketname, pulsetime
        );
        Self::send_success(self.client.post(url).json(&event)).await?;
        Ok(())
    }

    pub async fn delete_event(
        &self,
        bucketname: &str,
        event_id: i64,
    ) -> Result<(), reqwest::Error> {
        let url = format!(
            "{}api/0/buckets/{}/events/{}",
            self.baseurl, bucketname, event_id
        );
        Self::send_success(self.client.delete(url)).await?;
        Ok(())
    }

    pub async fn get_event_count(&self, bucketname: &str) -> Result<i64, reqwest::Error> {
        let url = format!("{}api/0/buckets/{}/events/count", self.baseurl, bucketname);
        let res = Self::send_success(self.client.get(url))
            .await?
            .text()
            .await?;
        let count: i64 = match res.trim().parse() {
            Ok(count) => count,
            Err(err) => panic!("could not parse get_event_count response: {err:?}"),
        };
        Ok(count)
    }

    pub async fn get_info(&self) -> Result<aw_models::Info, reqwest::Error> {
        let url = format!("{}api/0/info", self.baseurl);
        Self::send_success(self.client.get(url)).await?.json().await
    }

    pub async fn get_setting(&self, setting: &str) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}api/0/settings/{}", self.baseurl, setting);
        Self::send_success(self.client.get(url)).await?.json().await
    }

    pub async fn get_settings(&self) -> Result<aw_models::Settings, reqwest::Error> {
        let url = format!("{}api/0/settings", self.baseurl);
        Self::send_success(self.client.get(url)).await?.json().await
    }

    /// Wait up to ten seconds for the server to answer `GET /api/0/info` with its server info.
    ///
    /// Refused, dropped or unanswered connections are retried until the deadline. An HTTP
    /// error status, or a successful response that isn't ActivityWatch server info (another
    /// program on the port), is returned immediately. `/api/0/info` doesn't require the API
    /// key, so a wrong key is only reported by the first authenticated request.
    /// Requires a Tokio runtime with networking and time enabled.
    pub async fn wait_for_start(&self) -> Result<(), Box<dyn Error>> {
        wait_for_server(
            &self.client,
            self.api_url(&["info"]),
            Duration::from_secs(10),
        )
        .await
    }
}

async fn wait_for_server(
    client: &reqwest::Client,
    info_url: reqwest::Url,
    max_wait: Duration,
) -> Result<(), Box<dyn Error>> {
    let attempts = async {
        let mut retry_delay = Duration::from_millis(100);
        loop {
            // Bound each attempt, so a server that accepts connections but doesn't
            // answer yet is retried rather than waited on for the client's full timeout.
            let attempt = client
                .get(info_url.clone())
                .timeout(retry_delay.max(Duration::from_millis(500)))
                .send()
                .await;
            match attempt {
                Ok(response) => {
                    // A body cut short or stalled past the attempt timeout while the server
                    // is starting fails `bytes()`; that falls through to a retry.
                    if let Ok(body) = response.error_for_status()?.bytes().await {
                        // Decoding checks that it's an ActivityWatch server answering.
                        serde_json::from_slice::<aw_models::Info>(&body)?;
                        return Ok(());
                    }
                }
                // Refused, closed before responding (is_request), or unanswered: the server
                // may still be starting, so retry.
                Err(err) if err.is_connect() || err.is_request() || err.is_timeout() => {}
                Err(err) => return Err(err.into()),
            }
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(1));
        }
    };
    tokio::time::timeout(max_wait, attempts)
        .await
        .map_err(|_| -> Box<dyn Error> {
            format!(
                "Server at {} not responding after {} seconds of retrying",
                info_url.origin().ascii_serialization(),
                max_wait.as_secs_f64()
            )
            .into()
        })?
}

#[cfg(test)]
mod tests {
    use super::single_instance_name;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn info_url(addr: std::net::SocketAddr) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://{addr}/api/0/info")).unwrap()
    }

    const INFO_BODY: &str =
        r#"{"hostname":"host","version":"v0.0.0","testing":true,"device_id":"device"}"#;

    /// Answer the first connection that sends a request with `status_line` and `body`.
    ///
    /// Connections closed before sending a request are skipped: on Windows a connect to a
    /// port that isn't listening yet can hang instead of being refused, so an attempt the
    /// client already abandoned may be the first one accepted.
    async fn answer_once(listener: &tokio::net::TcpListener, status_line: &str, body: &str) {
        let (mut stream, request) = loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf).await {
                    Ok(n) if n > 0 => request.extend_from_slice(&buf[..n]),
                    _ => break,
                }
            }
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break (stream, request);
            }
        };
        assert!(request.starts_with(b"GET /api/0/info "));
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    #[test]
    fn test_wait_for_start_success() {
        runtime().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = info_url(listener.local_addr().unwrap());
            let server = async { answer_once(&listener, "200 OK", INFO_BODY).await };
            let client = reqwest::Client::new();
            let wait = super::wait_for_server(&client, url, std::time::Duration::from_secs(2));
            let (_, result) = tokio::join!(server, wait);
            result.unwrap();
        });
    }

    #[test]
    fn test_wait_for_start_retries_without_blocking_executor() {
        runtime().block_on(async {
            // Reserve a port without listening, then start the server on this same runtime.
            let socket = tokio::net::TcpSocket::new_v4().unwrap();
            socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let url = info_url(socket.local_addr().unwrap());
            let server = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                let listener = socket.listen(8).unwrap();
                answer_once(&listener, "200 OK", INFO_BODY).await;
            });
            let client = reqwest::Client::new();
            super::wait_for_server(&client, url, std::time::Duration::from_secs(3))
                .await
                .unwrap();
            server.await.unwrap();
        });
    }

    #[test]
    fn test_wait_for_start_retries_accepted_but_unanswered_connection() {
        runtime().block_on(async {
            // A listener that accepts (via the backlog) but never answers, like a server
            // that has bound its port but isn't serving HTTP yet.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = info_url(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (_silent, _) = listener.accept().await.unwrap();
                answer_once(&listener, "200 OK", INFO_BODY).await;
            });
            let client = reqwest::Client::new();
            super::wait_for_server(&client, url, std::time::Duration::from_secs(3))
                .await
                .unwrap();
            server.await.unwrap();
        });
    }

    #[test]
    fn test_wait_for_start_retries_connection_closed_before_response() {
        runtime().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = info_url(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                // Read the request, then hang up without answering.
                let (mut closed, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 1024];
                let _ = closed.read(&mut buf).await.unwrap();
                drop(closed);
                answer_once(&listener, "200 OK", INFO_BODY).await;
            });
            let client = reqwest::Client::new();
            super::wait_for_server(&client, url, std::time::Duration::from_secs(3))
                .await
                .unwrap();
            server.await.unwrap();
        });
    }

    #[test]
    fn test_wait_for_start_retries_truncated_body() {
        runtime().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = info_url(listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                // Send the headers and half of the body, then hang up.
                let (mut truncated, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 1024];
                let _ = truncated.read(&mut buf).await.unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
                    INFO_BODY.len()
                );
                truncated.write_all(head.as_bytes()).await.unwrap();
                truncated
                    .write_all(&INFO_BODY.as_bytes()[..INFO_BODY.len() / 2])
                    .await
                    .unwrap();
                drop(truncated);
                answer_once(&listener, "200 OK", INFO_BODY).await;
            });
            let client = reqwest::Client::new();
            super::wait_for_server(&client, url, std::time::Duration::from_secs(3))
                .await
                .unwrap();
            server.await.unwrap();
        });
    }

    async fn assert_fails_immediately(status_line: &str, body: &str) -> Box<dyn std::error::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = info_url(listener.local_addr().unwrap());
        let server = async { answer_once(&listener, status_line, body).await };
        let client = reqwest::Client::new();
        let started = std::time::Instant::now();
        let wait = super::wait_for_server(&client, url, std::time::Duration::from_secs(5));
        let (_, result) = tokio::join!(server, wait);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        result.unwrap_err()
    }

    #[test]
    fn test_wait_for_start_returns_http_errors_immediately() {
        runtime().block_on(async {
            let error = assert_fails_immediately("500 Internal Server Error", "{}").await;
            let error = error.downcast_ref::<reqwest::Error>().unwrap();
            assert_eq!(
                error.status(),
                Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
            );
        });
    }

    #[test]
    fn test_wait_for_start_rejects_other_services() {
        runtime().block_on(async {
            let error = assert_fails_immediately("200 OK", r#"{"status":"ok"}"#).await;
            assert!(
                error.downcast_ref::<serde_json::Error>().is_some(),
                "expected a JSON decode error, got {error:?}"
            );
        });
    }

    #[test]
    fn test_wait_for_start_deadline() {
        runtime().block_on(async {
            let socket = tokio::net::TcpSocket::new_v4().unwrap();
            socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let url = info_url(socket.local_addr().unwrap());
            let client = reqwest::Client::new();
            let started = std::time::Instant::now();
            let budget = std::time::Duration::from_millis(150);
            let error = super::wait_for_server(&client, url, budget)
                .await
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("not responding after 0.15 seconds"));
            assert!(started.elapsed() >= budget);
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        });
    }

    #[test]
    fn test_single_instance_name_normalizes_localhost() {
        // Both spellings must take the same lock, otherwise two instances of the
        // same watcher can run against the same server.
        assert_eq!(
            single_instance_name("aw-watcher-afk", "localhost", 5600),
            single_instance_name("aw-watcher-afk", "127.0.0.1", 5600),
        );
        assert_eq!(
            single_instance_name("aw-watcher-afk", "localhost", 5600),
            "aw-watcher-afk-at-127.0.0.1-on-5600",
        );

        // Hostnames are case-insensitive, so case variants must take the same
        // lock rather than each getting one of their own.
        for variant in ["LOCALHOST", "LocalHost", "localHost"] {
            assert_eq!(
                single_instance_name("aw-watcher-afk", variant, 5600),
                "aw-watcher-afk-at-127.0.0.1-on-5600",
                "host spelling {variant:?} should normalize"
            );
        }
    }

    #[test]
    fn test_single_instance_name_keeps_other_hosts() {
        assert_eq!(
            single_instance_name("aw-watcher-afk", "192.168.1.2", 5600),
            "aw-watcher-afk-at-192.168.1.2-on-5600",
        );
    }
}
