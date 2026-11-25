#![cfg(feature = "cli")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// Owns a local port until the test process exits and answers every request with
/// a 503, so no other process can take the port and change how the push fails.
fn failing_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut request = BufReader::new(stream);
            // Drain the headers first: closing with unread data resets the connection.
            let mut line = String::new();
            while request.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = request.get_mut().write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    port
}

// Named lists must normalize entries and continue past a failed pull to push.
// Empty lists stay distinct from omission. A failing local server keeps this
// hermetic.
#[test]
fn named_host_forms_continue_after_first_pull_failure() {
    for (args, expected_hosts, pull_all) in [
        (vec!["--host", "missing-a", "--host", "missing-b"], 2, false),
        (vec!["--host", "missing-a,missing-b"], 2, false),
        (vec!["--host", "missing-a, missing-b, ,"], 2, false),
        (vec!["--host", " , , "], 0, false),
        (vec![], 0, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        // Windows resolves known folders from `%USERPROFILE%\AppData\...` and
        // fails if the folder is missing, so the isolated profile needs both.
        for dir in ["AppData/Local", "AppData/Roaming"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        let port = failing_server();
        let output = Command::new(env!("CARGO_BIN_EXE_aw-sync"))
            .env("HOME", root.path())
            .env("USERPROFILE", root.path())
            .env("APPDATA", root.path().join("config"))
            .env("LOCALAPPDATA", root.path().join("data"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("AW_SYNC_LAST_REPORT", root.path().join("last-report.json"))
            .env("LOG_LEVEL", "info")
            .env("NO_PROXY", "127.0.0.1")
            .env_remove("AW_PROFILE")
            .arg("--port")
            .arg(port.to_string())
            .arg("--sync-dir")
            .arg(root.path().join("sync"))
            .arg("sync")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report_path = root.path().join("last-report.json");
        assert!(
            report_path.is_file(),
            "no report at {}; dir: {:?}\n{log}",
            report_path.display(),
            std::fs::read_dir(root.path())
                .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
        );
        assert!(!log.contains("panicked"), "{log}");
        if expected_hosts > 0 {
            assert!(log.contains("Pull from missing-a failed"), "{log}");
            assert!(log.contains("Pull from missing-b failed"), "{log}");
        }
        assert_eq!(
            log.matches("Pulling from host:").count(),
            expected_hosts,
            "{log}"
        );
        // An explicitly empty list must not turn into the omitted-host all-peer pull.
        assert_eq!(log.contains("Pulling from all hosts"), pull_all, "{log}");
        assert!(log.contains("Pushing local data"), "{log}");
        assert!(log.contains("push failed:"), "{log}");
        let report = std::fs::read_to_string(&report_path).unwrap();
        assert!(report.contains("503 Service Unavailable"), "{report}");
    }
}
