use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rocket::config::Config;
use rocket::data::{Limits, ToByteUnit};
use rocket::log::LogLevel;
use serde::{Deserialize, Serialize};

use crate::dirs;

// Far from an optimal way to solve it, but works and is simple
static mut TESTING: bool = true;
pub fn set_testing(testing: bool) {
    unsafe {
        TESTING = testing;
    }
}
pub fn is_testing() -> bool {
    unsafe { TESTING }
}

/// Authentication configuration, serialised as `[auth]` in config.toml.
#[derive(Serialize, Deserialize, Default)]
pub struct AWAuthConfig {
    /// Optional API key for Bearer-token authentication.
    /// When set, all `/api/*` endpoints except `/api/0/info` require:
    ///   Authorization: Bearer <api_key>
    /// Leave unset (default) to disable authentication.
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct AWConfig {
    #[serde(default = "default_address")]
    pub address: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(skip, default = "default_testing")]
    pub testing: bool, // This is not written to the config file (serde(skip))

    #[serde(default = "default_cors")]
    pub cors: Vec<String>,

    #[serde(default = "default_cors")]
    pub cors_regex: Vec<String>,

    // Authentication settings — serialised as [auth] section.
    #[serde(default)]
    pub auth: AWAuthConfig,

    // A mapping of watcher names to paths where the
    // custom visualizations are located.
    #[serde(default = "default_custom_static")]
    pub custom_static: std::collections::HashMap<String, String>,
}

impl Default for AWConfig {
    fn default() -> AWConfig {
        AWConfig {
            address: default_address(),
            port: default_port(),
            testing: default_testing(),
            auth: AWAuthConfig::default(),
            cors: default_cors(),
            cors_regex: default_cors(),
            custom_static: default_custom_static(),
        }
    }
}

impl AWConfig {
    pub fn to_rocket_config(&self) -> rocket::Config {
        let mut config;
        if self.testing {
            config = Config::debug_default();
            config.log_level = LogLevel::Debug;
        } else {
            config = Config::release_default()
        };

        // Needed for bucket imports
        let limits = Limits::default()
            .limit("json", 1000u64.megabytes())
            .limit("data-form", 1000u64.megabytes());

        config.address = self.address.parse().unwrap();
        config.port = self.port;
        config.keep_alive = 0;
        config.limits = limits;

        config
    }
}

fn default_address() -> String {
    "127.0.0.1".to_string()
}

fn default_cors() -> Vec<String> {
    Vec::<String>::new()
}

fn default_testing() -> bool {
    is_testing()
}

fn default_port() -> u16 {
    if is_testing() {
        5666
    } else {
        5600
    }
}

fn default_custom_static() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}

fn get_config_path(testing: bool, config_override: Option<&Path>) -> PathBuf {
    if let Some(config_path) = config_override {
        return config_path.to_path_buf();
    }

    let mut config_path = dirs::get_config_dir().unwrap();
    if !testing {
        config_path.push("config.toml")
    } else {
        config_path.push("config-testing.toml")
    }

    config_path
}

pub fn create_config(testing: bool, config_override: Option<&Path>) -> AWConfig {
    set_testing(testing);
    let config_path = get_config_path(testing, config_override);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).expect("Unable to create config dir");
    }

    /* If there is no config file, create a new config file with default values but every value is
     * commented out by default in case we would change a default value at some point in the future */
    if !config_path.is_file() {
        debug!("Writing default commented out config at {:?}", config_path);
        let mut wfile = File::create(config_path.clone()).expect("Unable to create config file");
        let default_config = AWConfig::default();
        let default_config_str =
            toml::to_string(&default_config).expect("Failed to convert default config to string");
        let mut default_config_str_commented = String::new();
        default_config_str_commented.push_str("### DEFAULT SETTINGS ###\n");
        for line in default_config_str.lines() {
            default_config_str_commented.push_str(&format!("#{line}\n"));
        }
        wfile
            .write_all(&default_config_str_commented.into_bytes())
            .expect("Failed to write config to file");
        wfile.sync_all().expect("Unable to sync config file");
    }

    debug!("Reading config at {:?}", config_path);
    let mut rfile = File::open(config_path).expect("Failed to open config file for reading");
    let mut content = String::new();
    rfile
        .read_to_string(&mut content)
        .expect("Failed to read config as a string");
    let aw_config: AWConfig = toml::from_str(&content).expect("Failed to parse config file");

    aw_config
}

#[cfg(test)]
mod tests {
    use super::create_config;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn unique_test_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("aw-server-config-tests")
            .join(format!("{name}-{}", Uuid::new_v4()))
            .join("config.toml")
    }

    #[test]
    fn create_config_uses_override_path() {
        let config_path = unique_test_path("override");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "address = \"0.0.0.0\"\nport = 5611\n").unwrap();

        let config = create_config(false, Some(config_path.as_path()));

        assert_eq!(config.address, "0.0.0.0");
        assert_eq!(config.port, 5611);
    }

    #[test]
    fn create_config_creates_missing_override_file() {
        let config_path = unique_test_path("missing");

        let config = create_config(false, Some(config_path.as_path()));

        assert!(config_path.is_file());
        assert_eq!(config.address, "127.0.0.1");
        assert_eq!(config.port, 5600);
    }
}
