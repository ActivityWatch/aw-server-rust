#![feature(plugin,try_from,custom_derive)]
#![feature(proc_macro_hygiene)]
#![plugin(rocket_codegen)]
extern crate rocket;
#[macro_use] extern crate rocket_contrib;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate plex;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;
