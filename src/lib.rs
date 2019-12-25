#![feature(plugin)]
#![feature(proc_macro_hygiene)]
#![feature(decl_macro)]
#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_contrib;
extern crate rocket_cors;
extern crate multipart;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate crossbeam_requests;

extern crate chrono;

extern crate plex;

extern crate appdirs;

#[cfg(target_os="android")]
#[macro_use] extern crate lazy_static;

#[macro_use] extern crate log;
extern crate fern;

extern crate toml;

#[macro_use] pub mod macros;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;
pub mod dirs;
pub mod logging;
pub mod config;

#[cfg(target_os="android")]
pub mod android;

pub mod sync;

extern crate aw_models;
