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

/// Drain the HTTP request fully before responding.
///
/// Parses Content-Length from headers so POST body data (which may arrive
/// in a separate TCP segment) is consumed before the mock writes its
/// response. Without this, reqwest may see a broken pipe on loopback if
/// the response arrives before the body finishes sending.
fn drain_request(stream: &mut impl Read) {
    let mut reader = BufReader::new(stream);
    let mut content_length = 0_usize;
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
}

fn spawn_mock_server(responses: Vec<MockResponse>) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock server");
    let port = listener.local_addr().expect("mock server addr").port();
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            drain_request(&mut stream);
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
