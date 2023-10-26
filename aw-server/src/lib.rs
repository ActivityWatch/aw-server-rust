#[macro_use]
extern crate rocket;
extern crate rocket_cors;

extern crate serde;
extern crate serde_json;

extern crate chrono;

#[cfg(not(target_os = "android"))]
extern crate appdirs;

#[cfg(target_os = "android")]
#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;
extern crate fern;

extern crate toml;

#[macro_use]
pub mod macros;
pub mod config;
pub mod device_id;
pub mod dirs;
pub mod endpoints;
pub mod logging;

#[cfg(target_os = "android")]
pub mod android;

extern crate aw_datastore;
extern crate aw_models;
extern crate aw_query;
extern crate aw_transform;
