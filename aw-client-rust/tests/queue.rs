//! Offline request queue behaviour against a scripted mock server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aw_client_rust::blocking::AwClient;
use aw_client_rust::Event;
use chrono::{TimeZone, Utc};

type Recorded = Arc<Mutex<Vec<(String, String)>>>;

/// Serves until dropped; `respond` picks the status code for each request line.
struct MockServer {
    port: u16,
    requests: Recorded,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(respond: impl FnMut(&str) -> u16 + Send + 'static) -> MockServer {
        Self::start_on(TcpListener::bind(("127.0.0.1", 0)).unwrap(), respond)
    }

    fn start_on(
        listener: TcpListener,
        mut respond: impl FnMut(&str) -> u16 + Send + 'static,
    ) -> MockServer {
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let requests: Recorded = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_requests, thread_stop) = (Arc::clone(&requests), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                stream.set_nonblocking(false).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut content_length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim().to_ascii_lowercase();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(len) = line.strip_prefix("content-length:") {
                        content_length = len.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                let request_line = request_line.trim().to_string();
                let status = respond(&request_line);
                thread_requests
                    .lock()
                    .unwrap()
                    .push((request_line, String::from_utf8(body).unwrap()));
                let body = if status == 200 {
                    r#"{"hostname":"h","version":"v","testing":true,"device_id":"d"}"#
                } else {
                    "{}"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        MockServer {
            port,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn request_lines(&self) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        requests.iter().map(|(line, _)| line.clone()).collect()
    }

    /// Request lines other than `/api/0/info`, which the worker may or may not send
    /// before a test registers its buckets.
    fn bucket_request_lines(&self) -> Vec<String> {
        self.request_lines()
            .into_iter()
            .filter(|line| !line.contains("/api/0/info"))
            .collect()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn unique(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "aw-client-rust-queue-{label}-{}-{nanos}",
        std::process::id()
    )
}

fn queue_path(label: &str) -> PathBuf {
    std::env::temp_dir()
        .join(unique(label))
        .join("queue.v1.jsonl")
}

fn event(second: u32, app: &str) -> Event {
    let mut data = serde_json::Map::new();
    data.insert("app".to_string(), serde_json::json!(app));
    Event {
        id: None,
        timestamp: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, second).unwrap(),
        duration: chrono::Duration::zero(),
        data,
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn creates_registered_buckets_then_delivers_heartbeats_in_order() {
    let server = MockServer::start(|_| 200);
    let client = AwClient::new("127.0.0.1", server.port, &unique("order")).unwrap();
    let path = queue_path("order");
    let queue = client.request_queue_at(path.clone()).unwrap();

    queue.register_bucket("bucket a", "currentwindow");
    for (second, app) in [(0, "one"), (1, "two"), (2, "three")] {
        queue
            .heartbeat("bucket a", &event(second, app), 5.0)
            .unwrap();
    }
    queue.flush().unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    assert!(queue.is_connected());
    queue.stop();

    let requests: Vec<_> = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(line, _)| !line.contains("/api/0/info"))
        .cloned()
        .collect();
    let lines: Vec<_> = requests.iter().map(|(line, _)| line.as_str()).collect();
    assert_eq!(
        lines,
        vec![
            "POST /api/0/buckets/bucket%20a HTTP/1.1",
            "POST /api/0/buckets/bucket%20a/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/bucket%20a/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/bucket%20a/heartbeat?pulsetime=5 HTTP/1.1",
        ]
    );
    let bucket: serde_json::Value = serde_json::from_str(&requests[0].1).unwrap();
    assert_eq!(bucket["type"], "currentwindow");
    let apps: Vec<_> = requests[1..]
        .iter()
        .map(|(_, body)| serde_json::from_str::<Event>(body).unwrap().data["app"].clone())
        .collect();
    assert_eq!(apps, vec!["one", "two", "three"]);

    // Drained: the queue file is emptied.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn keeps_heartbeats_while_server_is_down_and_sends_them_later() {
    let port = closed_port();
    let name = unique("offline");
    let path = queue_path("offline");
    let client = AwClient::new("127.0.0.1", port, &name).unwrap();

    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.heartbeat("bucket", &event(0, "one"), 5.0).unwrap();
    queue.heartbeat("bucket", &event(1, "two"), 5.0).unwrap();
    queue.flush().unwrap();
    assert_eq!(queue.len(), 2);
    queue.stop();

    // A new queue on the same file (e.g. after the watcher restarts) picks them up.
    let server = MockServer::start_on(TcpListener::bind(("127.0.0.1", port)).unwrap(), |_| 200);
    let queue = client.request_queue_at(path).unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();
    assert_eq!(
        server.request_lines(),
        vec![
            "GET /api/0/info HTTP/1.1",
            "POST /api/0/buckets/bucket/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/bucket/heartbeat?pulsetime=5 HTTP/1.1",
        ]
    );
}

#[test]
fn drops_bad_requests_and_retries_server_errors() {
    let mut heartbeats = 0;
    let server = MockServer::start(move |line| {
        if !line.contains("/heartbeat") {
            return 200;
        }
        heartbeats += 1;
        match heartbeats {
            1 => 400, // first heartbeat: rejected, dropped
            2 => 500, // second heartbeat: server error, retried
            _ => 200,
        }
    });
    let client = AwClient::new("127.0.0.1", server.port, &unique("errors")).unwrap();
    let queue = client.request_queue_at(queue_path("errors")).unwrap();

    queue
        .heartbeat("bucket", &event(0, "rejected"), 5.0)
        .unwrap();
    queue
        .heartbeat("bucket", &event(1, "retried"), 5.0)
        .unwrap();
    queue.flush().unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();

    let requests = server.requests.lock().unwrap().clone();
    let apps: Vec<_> = requests
        .iter()
        .filter(|(line, _)| line.contains("/heartbeat"))
        .map(|(_, body)| serde_json::from_str::<Event>(body).unwrap().data["app"].clone())
        .collect();
    assert_eq!(apps, vec!["rejected", "retried", "retried"]);
}

#[test]
fn reopening_skips_delivered_and_partial_lines() {
    let path = queue_path("reopen");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let line = |second, app| {
        serde_json::json!({"bucket_id": "bucket", "pulsetime": 5.0, "event": event(second, app)})
            .to_string()
    };
    // Three complete lines, the first already delivered, then a line cut short mid-write.
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n{{\"bucket_id\":\"buck",
            line(0, "delivered"),
            line(1, "two"),
            line(2, "three")
        ),
    )
    .unwrap();
    std::fs::write(path.with_file_name("queue.v1.jsonl.offset"), "1").unwrap();

    let client = AwClient::new("127.0.0.1", closed_port(), &unique("reopen")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    assert_eq!(queue.len(), 2);
    queue.heartbeat("bucket", &event(3, "four"), 5.0).unwrap();
    queue.stop();

    // The file was compacted to the pending heartbeats, and the new one appended cleanly.
    let contents = std::fs::read_to_string(&path).unwrap();
    let apps: Vec<_> = contents
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["event"]["data"]["app"].clone()
        })
        .collect();
    assert_eq!(apps, vec!["two", "three", "four"]);
    assert!(!path.with_file_name("queue.v1.jsonl.offset").exists());
}

#[test]
fn default_queue_path_separates_testing() {
    let prod = aw_client_rust::queue::default_queue_path("aw-watcher-x", false).unwrap();
    let testing = aw_client_rust::queue::default_queue_path("aw-watcher-x", true).unwrap();
    assert!(prod.ends_with("aw-client/queued/aw-watcher-x.v1.jsonl"));
    assert!(testing.ends_with("aw-client/queued/aw-watcher-x-testing.v1.jsonl"));
}

#[test]
fn a_queue_file_can_only_be_open_once() {
    let path = queue_path("locked");
    let client = AwClient::new("127.0.0.1", closed_port(), &unique("locked")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    let err = client.request_queue_at(path.clone()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    queue.stop();
    client.request_queue_at(path).unwrap().stop();
}

#[test]
fn reopened_queue_recreates_buckets_before_sending() {
    let port = closed_port();
    let path = queue_path("recreate");
    let client = AwClient::new("127.0.0.1", port, &unique("recreate")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.register_bucket("window", "currentwindow");
    queue.heartbeat("window", &event(0, "one"), 5.0).unwrap();
    queue.stop();

    // The watcher restarts; the queue delivers before the watcher registers anything.
    let server = MockServer::start_on(TcpListener::bind(("127.0.0.1", port)).unwrap(), |_| 200);
    let queue = client.request_queue_at(path).unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();
    assert_eq!(
        server.request_lines(),
        vec![
            "POST /api/0/buckets/window HTTP/1.1",
            "POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1",
        ]
    );
}

#[test]
fn a_missing_bucket_is_recreated_and_temporary_errors_retried() {
    let mut heartbeats = 0;
    let server = MockServer::start(move |line| {
        if !line.contains("/heartbeat") {
            return 200;
        }
        heartbeats += 1;
        match heartbeats {
            1 => 404, // bucket deleted behind our back: recreate, then retry
            2 => 503, // temporarily unavailable: retry
            3 => 429, // rate limited: retry
            _ => 200,
        }
    });
    let client = AwClient::new("127.0.0.1", server.port, &unique("missing")).unwrap();
    let queue = client.request_queue_at(queue_path("missing")).unwrap();
    queue.register_bucket("window", "currentwindow");
    queue.heartbeat("window", &event(0, "one"), 5.0).unwrap();
    queue.flush().unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();

    assert_eq!(
        server.bucket_request_lines(),
        vec![
            "POST /api/0/buckets/window HTTP/1.1",
            "POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/window HTTP/1.1",
            "POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1",
            "POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1",
        ]
    );
}

#[test]
fn heartbeats_to_an_unknown_missing_bucket_are_dropped() {
    let server = MockServer::start(|line| {
        if line.contains("/heartbeat") {
            404
        } else {
            200
        }
    });
    let client = AwClient::new("127.0.0.1", server.port, &unique("unknown")).unwrap();
    let queue = client.request_queue_at(queue_path("unknown")).unwrap();
    queue
        .heartbeat("never-registered", &event(0, "one"), 5.0)
        .unwrap();
    queue.flush().unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();
    assert_eq!(
        server.request_lines(),
        vec![
            "GET /api/0/info HTTP/1.1",
            "POST /api/0/buckets/never-registered/heartbeat?pulsetime=5 HTTP/1.1",
        ]
    );
}

#[cfg(unix)]
#[test]
fn queue_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let path = queue_path("private");
    let client = AwClient::new("127.0.0.1", closed_port(), &unique("private")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.heartbeat("bucket", &event(0, "one"), 5.0).unwrap();
    queue.stop();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_bucket_the_server_refuses_does_not_block_other_heartbeats() {
    let port = closed_port();
    let path = queue_path("refused");
    let client = AwClient::new("127.0.0.1", port, &unique("refused")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.register_bucket("refused", "currentwindow");
    queue.register_bucket("ok", "currentwindow");
    queue.heartbeat("refused", &event(0, "one"), 5.0).unwrap();
    queue.heartbeat("ok", &event(0, "two"), 5.0).unwrap();
    queue.stop();

    let server = MockServer::start_on(TcpListener::bind(("127.0.0.1", port)).unwrap(), |line| {
        if line.contains("/buckets/refused") {
            if line.contains("/heartbeat") {
                404
            } else {
                400
            }
        } else {
            200
        }
    });
    let queue = client.request_queue_at(path).unwrap();
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();
    let lines = server.request_lines();
    assert!(
        lines.contains(&"POST /api/0/buckets/ok/heartbeat?pulsetime=5 HTTP/1.1".to_string()),
        "{lines:?}"
    );
}

#[test]
fn transient_errors_creating_a_bucket_are_retried() {
    let (mut creates, mut created) = (0, false);
    let server = MockServer::start(move |line| {
        if line.contains("/api/0/info") {
            return 200;
        }
        if line.contains("/heartbeat") {
            // Like the server: no bucket, no heartbeat.
            return if created { 200 } else { 404 };
        }
        creates += 1;
        match creates {
            1 => 429,
            2 => 408,
            _ => {
                created = true;
                200
            }
        }
    });
    let client = AwClient::new("127.0.0.1", server.port, &unique("create-retry")).unwrap();
    let queue = client.request_queue_at(queue_path("create-retry")).unwrap();
    queue.register_bucket("window", "currentwindow");
    queue.heartbeat("window", &event(0, "one"), 5.0).unwrap();
    queue.flush().unwrap();
    // Registering again wakes the worker instead of waiting out the reconnect interval.
    // A registration made while an attempt is in flight must not be lost.
    let attempts = || {
        server
            .request_lines()
            .iter()
            .filter(|l| *l == "POST /api/0/buckets/window HTTP/1.1")
            .count()
    };
    wait_until("the first attempt", || attempts() >= 1);
    queue.register_bucket("window", "currentwindow");
    wait_until("the second attempt", || attempts() >= 2);
    queue.register_bucket("window", "currentwindow");
    wait_until("the queue to drain", || queue.is_empty());
    queue.stop();

    // The heartbeat went through after the third (successful) creation attempt. A later
    // registration may add another creation after it, which is fine.
    let lines = server.request_lines();
    let third_create = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "POST /api/0/buckets/window HTTP/1.1")
        .nth(2)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("bucket creation wasn't retried: {lines:?}"));
    assert!(
        lines[third_create..]
            .contains(&"POST /api/0/buckets/window/heartbeat?pulsetime=5 HTTP/1.1".to_string()),
        "{lines:?}"
    );
}

fn delivered_heartbeats(server: &MockServer) -> Vec<Event> {
    let requests = server.requests.lock().unwrap();
    requests
        .iter()
        .filter(|(line, _)| line.contains("/heartbeat"))
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect()
}

#[test]
fn premerges_heartbeats_until_the_commit_interval() {
    let server = MockServer::start(|_| 200);
    let client = AwClient::new("127.0.0.1", server.port, &unique("premerge")).unwrap();
    let queue = client.request_queue_at(queue_path("premerge")).unwrap();
    queue.set_commit_interval(10.0);

    // One heartbeat a second for 12 seconds, all with the same data.
    for second in 0..12 {
        queue
            .heartbeat("bucket", &event(second, "editor"), 5.0)
            .unwrap();
    }
    // Only the event that reached 10 s was queued; the rest is still being merged.
    wait_until("the committed heartbeat", || {
        queue.is_empty() && delivered_heartbeats(&server).len() == 1
    });
    queue.flush().unwrap();
    wait_until("the flushed heartbeat", || {
        queue.is_empty() && delivered_heartbeats(&server).len() == 2
    });
    queue.stop();

    let sent = delivered_heartbeats(&server);
    assert_eq!(sent[0].timestamp, event(0, "editor").timestamp);
    assert_eq!(sent[0].duration, chrono::Duration::seconds(10));
    assert_eq!(sent[1].timestamp, event(10, "editor").timestamp);
    assert_eq!(sent[1].duration, chrono::Duration::seconds(1));
}

#[test]
fn a_data_change_sends_the_merged_heartbeat() {
    let server = MockServer::start(|_| 200);
    let client = AwClient::new("127.0.0.1", server.port, &unique("change")).unwrap();
    let queue = client.request_queue_at(queue_path("change")).unwrap();

    for second in 0..3 {
        queue
            .heartbeat("bucket", &event(second, "editor"), 5.0)
            .unwrap();
    }
    queue
        .heartbeat("bucket", &event(3, "browser"), 5.0)
        .unwrap();
    wait_until("the merged heartbeat", || {
        queue.is_empty() && delivered_heartbeats(&server).len() == 1
    });
    queue.stop();

    let sent = delivered_heartbeats(&server);
    assert_eq!(sent[0].data["app"], "editor");
    assert_eq!(sent[0].timestamp, event(0, "editor").timestamp);
    assert_eq!(sent[0].duration, chrono::Duration::seconds(2));
}

#[test]
fn stopping_keeps_pending_merged_heartbeats() {
    let path = queue_path("pending");
    let client = AwClient::new("127.0.0.1", closed_port(), &unique("pending")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.heartbeat("a", &event(0, "editor"), 5.0).unwrap();
    queue.heartbeat("a", &event(1, "editor"), 5.0).unwrap();
    queue.heartbeat("b", &event(0, "browser"), 5.0).unwrap();
    assert_eq!(queue.len(), 0, "nothing reached the commit interval yet");
    queue.stop();

    let queue = client.request_queue_at(path).unwrap();
    assert_eq!(queue.len(), 2);
    queue.stop();
}

#[test]
fn merged_heartbeats_keep_the_bucket_type() {
    let path = queue_path("merged-type");
    let client = AwClient::new("127.0.0.1", closed_port(), &unique("merged-type")).unwrap();
    let queue = client.request_queue_at(path.clone()).unwrap();
    queue.register_bucket("window", "currentwindow");
    queue.heartbeat("window", &event(0, "editor"), 5.0).unwrap();
    queue.heartbeat("window", &event(1, "editor"), 5.0).unwrap();
    queue.stop();

    let line = std::fs::read_to_string(&path).unwrap();
    let queued: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(queued["bucket_type"], "currentwindow");
    assert_eq!(queued["event"]["duration"], 1.0);
}
