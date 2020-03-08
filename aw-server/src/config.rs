use rocket::config::{Config, Environment, Limits};
use std::fs::File;
use std::io::{Read, Write};

use crate::dirs;

#[derive(Serialize, Deserialize)]
pub struct AWConfig {
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(skip, default = "is_testing")]
    pub testing: bool, // This is not written to the config file (serde(skip))
    #[serde(default = "default_cors")]
    pub cors: Vec<String>,
}

impl Default for AWConfig {
    fn default() -> AWConfig {
        AWConfig {
            address: default_address(),
            port: default_port(),
            testing: is_testing(),
            cors: default_cors(),
        }
    }
}

impl AWConfig {
    pub fn to_rocket_config(&self) -> rocket::Config {
        let env = Environment::active().expect("Failed to get current environment");
        // Needed for bucket imports
        let limits = Limits::new().limit("json", 1 * 1000 * 1000 * 1000);

        Config::build(env)
            .address(self.address.clone())
            .port(self.port)
            .keep_alive(0)
            .limits(limits)
            .finalize()
            .unwrap()
    }
}

fn default_address() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    match Environment::active().expect("Failed to get current environment") {
        Environment::Production => 5600,
        Environment::Development => 5666,
        Environment::Staging => panic!("Staging environment not supported"),
    }
}

fn default_cors() -> Vec<String> {
    match Environment::active().expect("Failed to get current environment") {
        Environment::Production => Vec::<String>::new(),
        Environment::Development => Vec::<String>::new(),
        Environment::Staging => panic!("Staging environment not supported"),
    }
}

fn is_testing() -> bool {
    match Environment::active().expect("Failed to get current environment") {
        Environment::Production => false,
        Environment::Development => true,
        Environment::Staging => panic!("Staging environment not supported"),
    }
}

pub fn get_config() -> AWConfig {
    let mut config_path = dirs::get_config_dir().unwrap();
    match is_testing() {
        false => config_path.push("config.toml"),
        true => config_path.push("config-testing.toml"),
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
        default_config_str_commented.push_str(&"### DEFAULT SETTINGS ###\n");
        for line in default_config_str.lines() {
            default_config_str_commented.push_str(&format!("#{}\n", line));
        }
        wfile
            .write_all(&default_config_str_commented.into_bytes())
            .expect("Failed to write config to file");
        wfile.sync_all().expect("Unable to sync config file");
    }

    debug!("Reading config at {:?}", config_path);
    let mut rfile =
        File::open(config_path.clone()).expect("Failed to open config file for reading");
    let mut content = String::new();
    rfile
        .read_to_string(&mut content)
        .expect("Failed to read config as a string");
    let aw_config: AWConfig = toml::from_str(&content).expect("Failed to parse config file");

    aw_config
}
