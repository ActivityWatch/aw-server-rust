#![feature(plugin,try_from)]
#![feature(proc_macro_hygiene)]
#![feature(custom_attribute)]
#![feature(decl_macro)]
#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_contrib;
extern crate rocket_cors;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

extern crate plex;

extern crate appdirs;

#[macro_use] extern crate lazy_static;

#[macro_use] extern crate log;
extern crate fern;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;
pub mod dirs;
pub mod logging;

#[cfg(target_os="android")]
pub mod android;
