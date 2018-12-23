#![feature(plugin,try_from,custom_derive)]
#![feature(proc_macro_hygiene)]
#![plugin(rocket_codegen)]
extern crate rocket;
#[macro_use] extern crate rocket_contrib;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

extern crate plex;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;

fn main() {
    let server_state = endpoints::ServerState {
        datastore: datastore::Datastore::new("/home/erb/.local/share/activitywatch/aw-server-rust/aw-server-rust-testing.sqlite".to_string())
    };

    endpoints::rocket(server_state).launch();
}
