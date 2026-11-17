//! End-to-end tests for the `aw-client` binary against an in-process aw-server.
#![cfg(feature = "cli")]

use std::net::TcpListener;
use std::process::Command;

use aw_client_rust::blocking::AwClient;
use aw_client_rust::Event;
use chrono::{Duration, Utc};

fn start_server() -> (u16, rocket::Shutdown) {
    use aw_server::endpoints::{AssetResolver, ServerState};

    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let state = ServerState::new(
        aw_datastore::Datastore::new_in_memory(false),
        AssetResolver::new(None),
        "test_id".to_string(),
    );
    let config = aw_server::config::AWConfig {
        port,
        testing: true,
        ..Default::default()
    };
    let rocket =
        tokio_test::block_on(aw_server::endpoints::build_rocket(state, config).ignite()).unwrap();
    let shutdown = rocket.shutdown();
    std::thread::spawn(move || {
        // Surface a failed launch (e.g. the port was taken) instead of only timing out later.
        if let Err(err) = tokio_test::block_on(rocket.launch()) {
            eprintln!("aw-server failed to launch on port {port}: {err}");
        }
    });
    (port, shutdown)
}

fn event(minutes_ago: i64, seconds: i64, data: serde_json::Value) -> Event {
    Event {
        id: None,
        timestamp: Utc::now() - Duration::minutes(minutes_ago),
        duration: Duration::seconds(seconds),
        data: data.as_object().unwrap().clone(),
    }
}

fn aw_client(port: u16, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_aw-client"))
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "aw-client {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn cli_commands_against_a_server() {
    let (port, shutdown) = start_server();
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-cli-test").unwrap();
    client
        .wait_for_start()
        .expect("aw-server didn't start; see stderr for a launch error");

    let host = "clihost";
    let window = format!("aw-watcher-window_{host}");
    let afk = format!("aw-watcher-afk_{host}");
    client
        .create_bucket_simple(&window, "currentwindow")
        .unwrap();
    client.create_bucket_simple(&afk, "afkstatus").unwrap();
    client
        .insert_events(
            &window,
            vec![
                event(
                    30,
                    600,
                    serde_json::json!({"app": "vim", "title": "main.rs - GitHub"}),
                ),
                event(
                    10,
                    300,
                    serde_json::json!({"app": "Firefox", "title": "Reddit"}),
                ),
            ],
        )
        .unwrap();
    client
        .insert_event(
            &afk,
            &event(40, 2400, serde_json::json!({"status": "not-afk"})),
        )
        .unwrap();

    // buckets
    let out = aw_client(port, &["buckets"]);
    assert!(out.starts_with("Buckets:\n"), "{out}");
    assert!(out.contains(&format!(" - {window}\n")), "{out}");

    // heartbeat, then events
    client.create_bucket_simple("cli-bucket", "test").unwrap();
    let out = aw_client(port, &["heartbeat", "cli-bucket", r#"{"k": 1}"#]);
    assert!(out.contains("\"k\":1"), "{out}");
    let out = aw_client(port, &["events", "cli-bucket"]);
    assert!(out.starts_with("events:\n - "), "{out}");
    assert!(out.contains("(0:00:00) {\"k\":1}"), "{out}");

    // query, table and JSON
    let query_file =
        std::env::temp_dir().join(format!("aw-client-cli-{}.query", std::process::id()));
    std::fs::write(&query_file, format!("RETURN = query_bucket(\"{window}\");")).unwrap();
    let query_path = query_file.to_str().unwrap();
    let out = aw_client(port, &["query", query_path]);
    assert!(out.contains("Showing 2 out of 2 events:"), "{out}");
    assert!(out.contains("Total duration:\t 0:15:00"), "{out}");
    let out = aw_client(port, &["query", query_path, "--json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json[0].as_array().unwrap().len(), 2);
    // A range with no events
    let out = aw_client(
        port,
        &[
            "query",
            query_path,
            "--start",
            "2000-01-01",
            "--stop",
            "2000-01-02",
        ],
    );
    assert!(out.contains("Showing 0 out of 0 events:"), "{out}");
    let _ = std::fs::remove_file(&query_file);

    // --limit is capped at the query's own limit
    let output = Command::new(env!("CARGO_BIN_EXE_aw-client"))
        .args([
            "--port",
            &port.to_string(),
            "report",
            host,
            "--limit",
            "101",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());

    // report: categorized with the default classes (no classes setting on the server)
    let out = aw_client(port, &["report", host]);
    assert!(out.contains("Top 10 Categories"), "{out}");
    assert!(out.contains("Work > Programming"), "{out}");
    assert!(out.contains("Media > Social Media"), "{out}");
    assert!(out.contains("Top 10 Titles"), "{out}");
    assert!(out.contains("main.rs - GitHub"), "{out}");
    assert!(out.contains("Total duration:\t 0:15:00"), "{out}");

    // canonical
    let out = aw_client(port, &["canonical", host]);
    assert!(out.contains("Showing last 10 out of 2 events:"), "{out}");
    assert!(out.contains("[vim] main.rs - GitHub"), "{out}");
    assert!(out.contains("[Firefox] Reddit"), "{out}");

    shutdown.notify();
}

#[test]
fn cli_reports_errors_with_a_non_zero_exit() {
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let output = Command::new(env!("CARGO_BIN_EXE_aw-client"))
        .args(["--port", &port.to_string(), "buckets"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("Error: "));
}
