#![feature(plugin)]
#![feature(proc_macro_hygiene)]
#![feature(decl_macro)]
#[macro_use]
extern crate rocket;
#[macro_use]
extern crate rocket_contrib;
extern crate multipart;
extern crate rocket_cors;

extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

extern crate chrono;

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
pub mod dirs;
pub mod endpoints;
pub mod logging;

#[cfg(target_os = "android")]
pub mod android;

extern crate aw_datastore;
extern crate aw_models;
extern crate aw_query;
extern crate aw_transform;
