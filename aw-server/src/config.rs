use std::fs::File;
use std::io::{Read, Write};

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

pub fn create_config(testing: bool) -> AWConfig {
    set_testing(testing);
    let mut config_path = dirs::get_config_dir().unwrap();
    if !testing {
        config_path.push("config.toml")
    } else {
        config_path.push("config-testing.toml")
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
