//! Client configuration shared with aw-client-python.
//!
//! Both clients read `aw-client.toml` from the `aw-client` directory under the
//! ActivityWatch config directory:
//!
//! ```toml
//! [server]
//! hostname = "127.0.0.1"
//! port = "5600"
//!
//! [client]
//! commit_interval = 10
//!
//! [server-testing]
//! hostname = "127.0.0.1"
//! port = "5666"
//!
//! [client-testing]
//! commit_interval = 5
//! ```
//!
//! Values missing from the file fall back to the defaults above, as in Python.
//! Unlike Python, a missing file is not created.

use std::path::PathBuf;

const DEFAULT_HOSTNAME: &str = "127.0.0.1";

/// Settings for one profile (`testing` or not) of `aw-client.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientConfig {
    pub hostname: String,
    pub port: u16,
    /// Seconds a pre-merged heartbeat may grow before it is sent (finite, `>= 0`).
    pub commit_interval: f64,
}

impl ClientConfig {
    /// The built-in defaults, used for anything the config file doesn't set.
    pub fn default_for(testing: bool) -> ClientConfig {
        ClientConfig {
            hostname: DEFAULT_HOSTNAME.to_string(),
            port: if testing { 5666 } else { 5600 },
            commit_interval: if testing { 5.0 } else { 10.0 },
        }
    }
}

/// Path of `aw-client.toml`, matching aw-core's `get_config_dir("aw-client")`, which
/// uses platformdirs' `user_config_dir("activitywatch")`.
pub fn config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let root = dirs::data_local_dir()?
        .join("activitywatch")
        .join("activitywatch");
    #[cfg(not(target_os = "windows"))]
    let root = dirs::config_dir()?.join("activitywatch");
    Some(root.join("aw-client").join("aw-client.toml"))
}

/// Load the config for the `testing` or production profile from [`config_path`].
///
/// A missing file gives the defaults; an unreadable or invalid file is logged and
/// also gives the defaults, so a broken config never stops a watcher from starting.
pub fn load_config(testing: bool) -> ClientConfig {
    let Some(path) = config_path() else {
        return ClientConfig::default_for(testing);
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return ClientConfig::default_for(testing)
        }
        Err(err) => {
            log::warn!("Failed to read {}, using defaults: {err}", path.display());
            return ClientConfig::default_for(testing);
        }
    };
    parse_config(&raw, testing).unwrap_or_else(|err| {
        log::warn!("Invalid {}, using defaults: {err}", path.display());
        ClientConfig::default_for(testing)
    })
}

/// Parse `aw-client.toml` contents for one profile, filling unset values with defaults.
pub fn parse_config(raw: &str, testing: bool) -> Result<ClientConfig, String> {
    let table: toml::Table = raw.parse().map_err(|err| format!("{err}"))?;
    let (server_key, client_key) = if testing {
        ("server-testing", "client-testing")
    } else {
        ("server", "client")
    };
    let mut config = ClientConfig::default_for(testing);

    if let Some(server) = section(&table, server_key)? {
        if let Some(hostname) = server.get("hostname") {
            config.hostname = hostname
                .as_str()
                .ok_or(format!("[{server_key}] hostname must be a string"))?
                .to_string();
        }
        if let Some(port) = server.get("port") {
            // Python writes the port as a string ("5600"); accept an integer too.
            let port = match port {
                toml::Value::String(s) => s.trim().parse::<u16>().ok(),
                toml::Value::Integer(i) => u16::try_from(*i).ok(),
                _ => None,
            };
            config.port = port.ok_or(format!("[{server_key}] port must be a port number"))?;
        }
    }
    if let Some(client) = section(&table, client_key)? {
        if let Some(interval) = client.get("commit_interval") {
            config.commit_interval = match interval {
                toml::Value::Integer(i) => *i as f64,
                toml::Value::Float(f) => *f,
                _ => return Err(format!("[{client_key}] commit_interval must be a number")),
            };
            // 0 is allowed: like in Python, it sends every merged heartbeat right away.
            if !config.commit_interval.is_finite() || config.commit_interval < 0.0 {
                return Err(format!(
                    "[{client_key}] commit_interval must be a non-negative number of seconds"
                ));
            }
        }
    }
    Ok(config)
}

fn section<'a>(table: &'a toml::Table, key: &str) -> Result<Option<&'a toml::Table>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => Ok(Some(t)),
        Some(_) => Err(format!("[{key}] must be a table")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(
            parse_config("", false).unwrap(),
            ClientConfig::default_for(false)
        );
        assert_eq!(
            parse_config("", true).unwrap(),
            ClientConfig {
                hostname: "127.0.0.1".to_string(),
                port: 5666,
                commit_interval: 5.0
            }
        );
    }

    #[test]
    fn python_default_file_parses() {
        let raw = r#"
[server]
hostname = "127.0.0.1"
port = "5600"

[client]
commit_interval = 10

[server-testing]
hostname = "127.0.0.1"
port = "5666"

[client-testing]
commit_interval = 5
"#;
        assert_eq!(
            parse_config(raw, false).unwrap(),
            ClientConfig::default_for(false)
        );
        assert_eq!(
            parse_config(raw, true).unwrap(),
            ClientConfig::default_for(true)
        );
    }

    #[test]
    fn partial_file_overrides_only_what_it_sets() {
        let raw = "[server]\nport = 5700\n[client]\ncommit_interval = 2.5\n[server-testing]\nhostname = \"10.0.0.2\"\n";
        assert_eq!(
            parse_config(raw, false).unwrap(),
            ClientConfig {
                hostname: "127.0.0.1".to_string(),
                port: 5700,
                commit_interval: 2.5
            }
        );
        assert_eq!(
            parse_config(raw, true).unwrap(),
            ClientConfig {
                hostname: "10.0.0.2".to_string(),
                port: 5666,
                commit_interval: 5.0
            }
        );
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!(parse_config("[server]\nport = \"http\"\n", false).is_err());
        assert!(parse_config("[server]\nport = 70000\n", false).is_err());
        assert!(parse_config("server = 1\n", false).is_err());
        assert!(parse_config("[client]\ncommit_interval = \"soon\"\n", false).is_err());
        assert!(parse_config("[client]\ncommit_interval = -1\n", false).is_err());
        assert!(parse_config("[client]\ncommit_interval = nan\n", false).is_err());
        assert!(parse_config("[client]\ncommit_interval = inf\n", false).is_err());
        assert_eq!(
            parse_config("[client]\ncommit_interval = 0\n", false)
                .unwrap()
                .commit_interval,
            0.0
        );
        assert!(parse_config("not toml", false).is_err());
    }

    #[test]
    fn config_path_is_under_activitywatch() {
        let path = config_path().unwrap();
        assert!(path.ends_with("activitywatch/aw-client/aw-client.toml"));
    }
}
