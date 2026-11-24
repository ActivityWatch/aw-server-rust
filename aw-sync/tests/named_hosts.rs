#![cfg(feature = "cli")]

use std::net::TcpListener;
use std::process::Command;

// Both forms must reach the host loop, continue past its first failure, and
// attempt the local push. Refuse the local connection to keep this hermetic.
#[test]
fn named_host_forms_continue_after_first_pull_failure() {
    for args in [
        vec!["--host", "missing-a", "--host", "missing-b"],
        vec!["--host", "missing-a,missing-b"],
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
        assert!(!log.contains("panicked"), "{log}");
        assert!(log.contains("Pull from missing-a failed"), "{log}");
        assert!(log.contains("Pull from missing-b failed"), "{log}");
        assert!(log.contains("Pushing local data"), "{log}");
        assert!(log.contains("push failed:"), "{log}");
    }
}
