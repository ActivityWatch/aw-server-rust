use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;

use rocket::config::Config;
use rocket::data::{Limits, ToByteUnit};
use rocket::log::LogLevel;
use serde::{Deserialize, Serialize};

use crate::dirs;
use serde_json;

pub const CORS_FIELDS: &[&str] = &[
    "cors",
    "cors_regex",
    "cors_allow_aw_chrome_extension",
    "cors_allow_all_mozilla_extension",
];

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

#[derive(Serialize, Deserialize, Clone)]
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

    #[serde(default = "default_true")]
    pub cors_allow_aw_chrome_extension: bool,

    #[serde(default = "default_false")]
    pub cors_allow_all_mozilla_extension: bool,

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
            cors: default_cors(),
            cors_regex: default_cors(),
            cors_allow_aw_chrome_extension: default_true(),
            cors_allow_all_mozilla_extension: default_false(),
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

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
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

pub fn get_config_path(testing: bool) -> (std::path::PathBuf, Vec<String>) {
    let mut config_path = dirs::get_config_dir().unwrap();
    if !testing {
        config_path.push("config.toml")
    } else {
        config_path.push("config-testing.toml")
    }
    if !config_path.is_file() {
        return (
            config_path,
            CORS_FIELDS.iter().map(|f| f.to_string()).collect(),
        );
    }
    let content = fs::read_to_string(&config_path).unwrap_or_default();
    let toml_value: toml::Value =
        toml::from_str(&content).unwrap_or_else(|_| toml::Value::Table(toml::Table::new()));

    let file_keys: HashSet<String> = toml_value
        .as_table()
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();

    let missing = CORS_FIELDS
        .iter()
        .filter(|f| !file_keys.contains(&f.to_string()))
        .map(|f| f.to_string())
        .collect();

    (config_path, missing)
}

pub fn create_config(testing: bool, datastore: &aw_datastore::Datastore) -> AWConfig {
    set_testing(testing);
    let (config_path, missing_cors_fields) = get_config_path(testing);

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
    let content = fs::read_to_string(config_path).expect("Failed to read config file");
    let toml_value: toml::Value = toml::from_str(&content).expect("Failed to parse config file");

    let mut aw_config: AWConfig =
        toml_value.try_into().expect("Failed to convert TOML value to AWConfig");

    for field in missing_cors_fields {
        let Ok(value_str) = datastore.get_key_value(&format!("cors.{field}")) else { continue };

        match field.as_str() {
            "cors"       => aw_config.cors       = serde_json::from_str(&value_str).unwrap_or_default(),
            "cors_regex" => aw_config.cors_regex  = serde_json::from_str(&value_str).unwrap_or_default(),
            "cors_allow_aw_chrome_extension"   => aw_config.cors_allow_aw_chrome_extension  = serde_json::from_str(&value_str).unwrap_or_default(),
            "cors_allow_all_mozilla_extension" => aw_config.cors_allow_all_mozilla_extension = serde_json::from_str(&value_str).unwrap_or_default(),
            _ => {}
        }
    }
    aw_config
}
