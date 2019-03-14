use std::path::PathBuf;
use std::sync::Mutex;

use rocket;
use rocket::config::{Config};
use rocket::response::{NamedFile};
use rocket::State;
use rocket_contrib::json::JsonValue;

#[macro_export]
macro_rules! endpoints_get_lock {
    ( $lock:expr ) => {
        match $lock.lock() {
            Ok(r) => r,
            Err(e) => {
                warn!("Taking datastore lock failed, returning 504: {}", e);
                return Err(Status::ServiceUnavailable);
            }
        }
    }
}

mod bucket;
mod query;
mod import;
mod cors;
mod export;

use crate::datastore::Datastore;

pub struct ServerState {
    pub datastore: Mutex<Datastore>,
    pub asset_path: PathBuf,
}

#[get("/")]
fn root_index(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("index.html")).ok()
}

#[get("/0.css")]
fn root_css(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("0.css")).ok()
}

#[get("/0.css.map")]
fn root_css_map(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("0.css.map")).ok()
}

#[get("/static/<file..>")]
fn root_static(file: PathBuf, state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("static").join(file)).ok()
}

#[get("/favicon.ico")]
fn root_favicon(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("favicon.ico")).ok()
}

#[get("/")]
fn server_info() -> JsonValue {
    let testing : bool;
    #[cfg(debug_assertions)]
    {
        testing = true;
    }
    #[cfg(not(debug_assertions))]
    {
        testing = false;
    }

    json!({
        "hostname": "johan-desktop",
        "version": "aw-server-rust_v0.1",
        "testing": testing
    })
}

// This catcher is sadly invalid as catchers in rocket are only forwarded if they
// have code 400-599 for some reason
#[catch(304)]
fn not_modified() -> JsonValue {
    json!({
        "status": 304,
        "reason": "Not modified."
    })
}

#[catch(404)]
fn not_found() -> JsonValue {
    json!({
        "status": 404,
        "reason": "Resource was not found."
    })
}

pub fn build_rocket(server_state: ServerState, config: Config) -> rocket::Rocket {
    info!("Starting aw-server-rust at {}:{}", config.address, config.port);
    rocket::custom(config)
        .mount("/", routes![
               root_index, root_favicon, root_static, root_css, root_css_map,
        ])
        .mount("/api/0/info", routes![server_info])
        .mount("/api/0/buckets", routes![
               bucket::bucket_new, bucket::bucket_delete, bucket::buckets_get, bucket::bucket_get,
               bucket::bucket_events_get, bucket::bucket_events_create, bucket::bucket_events_heartbeat, bucket::bucket_event_count,
               bucket::bucket_export
        ])
        .mount("/api/0/query", routes![
               query::query
        ])
        .mount("/api/0/import", routes![
               import::bucket_import_json,
               import::bucket_import_form
        ])
        .mount("/api/0/export", routes![
               export::buckets_export
        ])
        .attach(cors::cors())
        .register(catchers![not_modified, not_found])
        .manage(server_state)
}
