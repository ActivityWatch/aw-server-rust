#![cfg(feature = "cli")]

use std::net::TcpListener;
use std::process::Command;

// Named lists must normalize entries and continue past a failed pull to push.
// Empty lists stay distinct from omission. Refuse the local connection to keep
// this hermetic.
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
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
    }
}
