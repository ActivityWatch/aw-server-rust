use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use aw_client_rust::blocking;
use aw_client_rust::AwClient;

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
        .block_on(future)
}

struct MockResponse {
    status_line: &'static str,
    content_type: &'static str,
    body: &'static str,
}

/// Drain the HTTP request fully before responding, returning its request line.
///
/// Parses Content-Length from headers so POST body data (which may arrive
/// in a separate TCP segment) is consumed before the mock writes its
/// response. Without this, reqwest may see a broken pipe on loopback if
/// the response arrives before the body finishes sending.
fn drain_request(stream: &mut impl Read) -> String {
    let mut reader = BufReader::new(stream);
    let mut content_length = 0_usize;
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("read request line");
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read request line");
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // End of headers; body follows if Content-Length > 0
            break;
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body_buf = vec![0_u8; content_length];
        reader
            .read_exact(&mut body_buf)
            .expect("drain request body");
    }
    request_line.trim().to_string()
}

/// The join handle yields the request line of every request the mock served.
fn spawn_mock_server(responses: Vec<MockResponse>) -> (u16, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock server");
    let port = listener.local_addr().expect("mock server addr").port();
    let handle = thread::spawn(move || {
        let mut request_lines = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            request_lines.push(drain_request(&mut stream));
            let body = response.body.as_bytes();
            write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n{}",
                response.status_line,
                body.len(),
                response.content_type,
                response.body
            )
            .expect("write response");
            stream.flush().expect("flush response");
        }
        request_lines
    });
    (port, handle)
}

#[test]
fn async_client_rejects_non_success_statuses() {
    let (port, handle) = spawn_mock_server(vec![
        MockResponse {
            status_line: "500 Internal Server Error",
            content_type: "application/json",
            body: "{}",
        },
        MockResponse {
            status_line: "409 Conflict",
            content_type: "text/plain",
            body: "",
        },
    ]);
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    let err = block_on(client.get_buckets()).expect_err("500 response must fail");
    assert_eq!(
        err.status(),
        Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
    );

    let err = block_on(client.create_bucket_simple("bucket", "type"))
        .expect_err("409 response must fail");
    assert_eq!(err.status(), Some(reqwest::StatusCode::CONFLICT));

    handle.join().expect("join mock server");
}

#[test]
fn get_event_count_returns_error_for_non_numeric_body() {
    let (port, handle) = spawn_mock_server(vec![
        MockResponse {
            status_line: "200 OK",
            content_type: "application/json",
            body: "42\n",
        },
        MockResponse {
            status_line: "200 OK",
            content_type: "text/html",
            body: "<html>not a count</html>",
        },
    ]);
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    assert_eq!(block_on(client.get_event_count("bucket")).unwrap(), 42);
    let err = block_on(client.get_event_count("bucket")).expect_err("non-numeric body must fail");
    assert!(err.is_decode(), "expected a decode error, got {err:?}");

    handle.join().expect("join mock server");
}

#[test]
fn blocking_client_rejects_non_success_statuses() {
    let (port, handle) = spawn_mock_server(vec![
        MockResponse {
            status_line: "500 Internal Server Error",
            content_type: "application/json",
            body: "{}",
        },
        MockResponse {
            status_line: "409 Conflict",
            content_type: "text/plain",
            body: "",
        },
    ]);
    let client =
        blocking::AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    let err = client
        .get_buckets()
        .expect_err("500 response must fail for blocking client");
    assert_eq!(
        err.status(),
        Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
    );

    let err = client
        .create_bucket_simple("bucket", "type")
        .expect_err("409 response must fail for blocking client");
    assert_eq!(err.status(), Some(reqwest::StatusCode::CONFLICT));

    handle.join().expect("join mock server");
}

#[test]
fn get_event_maps_404_to_none_and_rejects_other_errors() {
    let (port, handle) = spawn_mock_server(vec![
        MockResponse {
            status_line: "404 Not Found",
            content_type: "application/json",
            body: r#"{"message":"missing"}"#,
        },
        MockResponse {
            status_line: "500 Internal Server Error",
            content_type: "application/json",
            body: "{}",
        },
    ]);
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    let event = block_on(client.get_event("bucket", 1)).expect("404 must not be an error");
    assert!(event.is_none());

    let err = block_on(client.get_event("bucket", 2)).expect_err("500 response must fail");
    assert_eq!(
        err.status(),
        Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
    );

    handle.join().expect("join mock server");
}

#[test]
fn get_event_count_sends_time_filters() {
    let count = || MockResponse {
        status_line: "200 OK",
        content_type: "application/json",
        body: "3",
    };
    let (port, handle) = spawn_mock_server(vec![count(), count()]);
    let client =
        blocking::AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");
    let start = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00+00:00")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339("2024-01-02T00:00:00+00:00")
        .unwrap()
        .with_timezone(&chrono::Utc);

    assert_eq!(client.get_event_count("bucket", None, None).unwrap(), 3);
    assert_eq!(
        client
            .get_event_count("bucket", Some(start), Some(end))
            .unwrap(),
        3
    );

    let requests = handle.join().expect("join mock server");
    assert_eq!(
        requests,
        vec![
            "GET /api/0/buckets/bucket/events/count HTTP/1.1",
            "GET /api/0/buckets/bucket/events/count?start=2024-01-01T00%3A00%3A00%2B00%3A00&end=2024-01-02T00%3A00%3A00%2B00%3A00 HTTP/1.1",
        ]
    );
}

#[test]
fn setting_keys_are_encoded_as_one_path_segment() {
    let ok = || MockResponse {
        status_line: "200 OK",
        content_type: "application/json",
        body: "null",
    };
    let (port, handle) = spawn_mock_server(vec![ok(), ok()]);
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    block_on(client.set_setting("ui#theme?x/y", &serde_json::json!("dark"))).expect("set setting");
    block_on(client.get_setting("ui#theme?x/y")).expect("get setting");

    let requests = handle.join().expect("join mock server");
    assert_eq!(
        requests,
        vec![
            "POST /api/0/settings/ui%23theme%3Fx%2Fy HTTP/1.1",
            "GET /api/0/settings/ui%23theme%3Fx%2Fy HTTP/1.1",
        ]
    );
}

#[test]
fn query_cached_sends_name_and_cache_params() {
    let result = || MockResponse {
        status_line: "200 OK",
        content_type: "application/json",
        body: "[[]]",
    };
    let (port, handle) = spawn_mock_server(vec![result(), result()]);
    let client =
        blocking::AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");
    let period = (
        chrono::Utc::now() - chrono::Duration::hours(1),
        chrono::Utc::now(),
    );

    client.query("RETURN = 1;", vec![period]).expect("query");
    client
        .query_cached("RETURN = 1;", vec![period], "daily summary")
        .expect("cached query");

    let requests = handle.join().expect("join mock server");
    assert_eq!(
        requests,
        vec![
            "POST /api/0/query HTTP/1.1",
            "POST /api/0/query?name=daily+summary&cache=1 HTTP/1.1",
        ]
    );
}

#[test]
fn export_bucket_encodes_bucket_id_as_one_path_segment() {
    let (port, handle) = spawn_mock_server(vec![MockResponse {
        status_line: "200 OK",
        content_type: "application/json",
        body: r#"{"buckets":{}}"#,
    }]);
    let client = AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    block_on(client.export_bucket("a#b?c/d")).expect("export bucket");

    let requests = handle.join().expect("join mock server");
    assert_eq!(
        requests,
        vec!["GET /api/0/buckets/a%23b%3Fc%2Fd/export HTTP/1.1"]
    );
}

#[test]
fn get_classes_uses_server_setting_and_falls_back_to_defaults() {
    let default_names: Vec<_> = aw_client_rust::classes::default_classes()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let (port, handle) = spawn_mock_server(vec![
        MockResponse {
            status_line: "200 OK",
            content_type: "application/json",
            body: r#"[{"name":["Work","Rust"],"rule":{"type":"regex","regex":"cargo"}}]"#,
        },
        MockResponse {
            status_line: "200 OK",
            content_type: "application/json",
            body: "null",
        },
        MockResponse {
            status_line: "500 Internal Server Error",
            content_type: "application/json",
            body: "{}",
        },
    ]);
    let client =
        blocking::AwClient::new("127.0.0.1", port, "aw-client-rust-test").expect("create client");

    let classes = client.get_classes();
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].0, vec!["Work", "Rust"]);
    assert_eq!(classes[0].1.regex, "cargo");

    let unset: Vec<_> = client.get_classes().into_iter().map(|(n, _)| n).collect();
    assert_eq!(unset, default_names);

    let failed: Vec<_> = client.get_classes().into_iter().map(|(n, _)| n).collect();
    assert_eq!(failed, default_names);

    handle.join().expect("join mock server");
}
