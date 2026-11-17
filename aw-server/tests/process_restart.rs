//! HTTP acknowledgement followed by real process death, not a second connection
//! to a still-running datastore. This does not simulate device power loss.
#![cfg(unix)]

use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::fs::File;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Server {
    child: Child,
    client: Client,
    url: String,
}

impl Server {
    fn start(root: &Path) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let log = File::create(root.join("server.log")).unwrap();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_aw-server"))
            .args(["--profile", "restart-regression", "--host", "127.0.0.1"])
            .args(["--port", &port.to_string(), "--no-legacy-import"])
            .arg("--dbpath")
            .arg(root.join("events.db"))
            .arg("--config")
            .arg(root.join("config.toml"))
            .env("HOME", root)
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env_remove("AW_DB_PASSWORD")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            client,
            url: format!("http://127.0.0.1:{port}/api/0"),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "server exited during startup: {}",
                std::fs::read_to_string(root.join("server.log")).unwrap()
            );
            if server
                .client
                .get(format!("{}/info", server.url))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                return server;
            }
            assert!(Instant::now() < deadline, "server readiness timeout");
            sleep(Duration::from_millis(20));
        }
    }

    fn post(&self, path: &str, body: &Value) -> Value {
        self.client
            .post(format!("{}{path}", self.url))
            .json(body)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }

    fn events(&self, bucket: &str) -> Value {
        self.client
            .get(format!("{}/buckets/{bucket}/events", self.url))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }

    fn bucket(&self, id: &str, kind: &str) {
        self.client
            .post(format!("{}/buckets/{id}", self.url))
            .json(
                &json!({"id": id, "type": kind, "client": "restart-test", "hostname": "synthetic"}),
            )
            .send()
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    fn stop(&mut self, graceful: bool) {
        if graceful {
            // Rocket's ordinary Unix shutdown path; do not add force_commit.
            let status = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());
        } else {
            self.child.kill().unwrap(); // SIGKILL: no destructor/checkpoint.
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                if graceful {
                    assert!(status.success(), "ordinary shutdown failed: {status}");
                }
                return;
            }
            assert!(Instant::now() < deadline, "server shutdown timeout");
            sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Also reap on request failures/assertion panic. A live child cannot
        // retain the temporary database after its owning test unwinds.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn stopwatch_restart(graceful: bool) {
    let dir = TempDir::new().unwrap();
    let mut server = Server::start(dir.path());
    server.bucket("timer", "general.stopwatch");
    let started = server.post(
        "/buckets/timer/heartbeat?pulsetime=1",
        &json!({"timestamp": "2026-01-01T12:00:00Z", "duration": 0.0,
            "data": {"running": true, "label": "restart-regression"}}),
    );
    assert!(started["id"].is_number());
    server.stop(graceful);
    server = Server::start(dir.path());
    assert_eq!(server.events("timer"), json!([started.clone()]));

    let mut stopped = started;
    stopped["duration"] = json!(123.0);
    stopped["data"]["running"] = json!(false);
    let acknowledged = server.post("/buckets/timer/events", &json!([stopped.clone()]));
    assert_eq!(acknowledged, json!([stopped]));
    server.stop(graceful);
    server = Server::start(dir.path());
    assert_eq!(server.events("timer"), acknowledged);
    server.stop(false);
}

#[test]
fn acknowledged_stopwatch_start_and_stop_survive_process_kill() {
    stopwatch_restart(false);
}

#[test]
fn acknowledged_stopwatch_start_and_stop_survive_normal_shutdown() {
    stopwatch_restart(true);
}

fn ordinary_watcher_restart(first_attempt_delay: Duration) -> usize {
    for attempt in 0..3 {
        let dir = TempDir::new().unwrap();
        // Start before spawn, not after bucket creation: the writer's batch
        // clock can already be running during startup. Include process death
        // so a pause after the SQLite read cannot invalidate the restart check.
        let window = Instant::now();
        let mut server = Server::start(dir.path());
        server.bucket("watcher", "currentwindow");
        if attempt == 0 {
            sleep(first_attempt_delay);
        }
        let acknowledged = server.post(
            "/buckets/watcher/heartbeat?pulsetime=1",
            &json!({"timestamp": "2026-01-01T12:00:00Z", "duration": 1.0,
                "data": {"title": "synthetic batched control"}}),
        );
        let conn = rusqlite::Connection::open(dir.path().join("events.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM events WHERE id = ?1",
                [acknowledged["id"].as_i64().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        server.stop(false);
        if window.elapsed() >= Duration::from_secs(15) {
            // Normal interval commits are allowed. Retry with a fresh process
            // and database; never retry a failure inside the valid window.
            continue;
        }
        assert_eq!(count, 0, "ordinary watcher unexpectedly forced a commit");
        server = Server::start(dir.path());
        assert_eq!(server.events("watcher"), json!([]));
        server.stop(false);
        return attempt;
    }
    panic!("ordinary watcher control exhausted three attempts within the 15-second batch window");
}

#[test]
fn ordinary_watcher_acknowledgement_remains_batched() {
    ordinary_watcher_restart(Duration::ZERO);
}

#[test]
fn ordinary_watcher_control_retries_expired_batch_window() {
    assert!(ordinary_watcher_restart(Duration::from_secs(16)) >= 1);
}
