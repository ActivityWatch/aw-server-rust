use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use gethostname::gethostname;
use rocket;
use rocket::response::NamedFile;
use rocket::State;
use rocket_contrib::json::JsonValue;
use uuid::Uuid;

use crate::config::AWConfig;
use crate::dirs;

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
    };
}

mod bucket;
mod cors;
mod export;
mod import;
mod query;
mod settings;

use aw_datastore::Datastore;

pub struct ServerState {
    pub datastore: Mutex<Datastore>,
    pub asset_path: PathBuf,
}

#[get("/")]
fn root_index(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("index.html")).ok()
}

#[get("/css/<file..>")]
fn root_css(file: PathBuf, state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("css").join(file)).ok()
}

#[get("/fonts/<file..>")]
fn root_fonts(file: PathBuf, state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("fonts").join(file)).ok()
}

#[get("/js/<file..>")]
fn root_js(file: PathBuf, state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("js").join(file)).ok()
}

#[get("/static/<file..>")]
fn root_static(file: PathBuf, state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("static").join(file)).ok()
}

#[get("/favicon.ico")]
fn root_favicon(state: State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("favicon.ico")).ok()
}

/// Retrieves the device ID, if none exists it generates one (using UUID v4)
fn get_device_id() -> String {
    // TODO: Cache to avoid retrieving on every /info call
    // TODO: How should these unwraps be removed?
    //       Should this be propagated into a 500 Internal Server Error? How?
    // I chose get_data_dir over get_config_dir since the latter isn't yet supported on Android.
    let mut path = dirs::get_data_dir().unwrap();
    path.push("device_id");
    if path.exists() {
        fs::read_to_string(path).unwrap()
    } else {
        let uuid = Uuid::new_v4().to_hyphenated().to_string();
        fs::write(path, &uuid).unwrap();
        uuid
    }
}

#[get("/")]
fn server_info() -> JsonValue {
    let testing: bool;
    #[cfg(debug_assertions)]
    {
        testing = true;
    }
    //TODO: Should this be commented?
    #[cfg(not(debug_assertions))]
    {
        testing = false;
    }

    let hostname = gethostname().into_string().unwrap_or("unknown".to_string());
    const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

    json!({
        "hostname": hostname,
        "version": format!("aw-server-rust v{}", VERSION.unwrap_or("(unknown)")),
        "testing": testing,
        "device_id": get_device_id(),
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

pub fn build_rocket(server_state: ServerState, config: &AWConfig) -> rocket::Rocket {
    info!(
        "Starting aw-server-rust at {}:{}",
        config.address, config.port
    );
    rocket::custom(config.to_rocket_config())
        .mount(
            "/",
            routes![
                root_index,
                root_favicon,
                root_fonts,
                root_css,
                root_js,
                root_static,
            ],
        )
        .mount("/api/0/info", routes![server_info])
        .mount(
            "/api/0/buckets",
            routes![
                bucket::bucket_new,
                bucket::bucket_delete,
                bucket::buckets_get,
                bucket::bucket_get,
                bucket::bucket_events_get,
                bucket::bucket_events_create,
                bucket::bucket_events_heartbeat,
                bucket::bucket_event_count,
                bucket::bucket_events_delete_by_id,
                bucket::bucket_export
            ],
        )
        .mount("/api/0/query", routes![query::query])
        .mount(
            "/api/0/import",
            routes![import::bucket_import_json, import::bucket_import_form],
        )
        .mount("/api/0/export", routes![export::buckets_export])
        .mount(
            "/api/0/settings",
            routes![
                settings::setting_get,
                settings::settings_list_get,
                settings::setting_set,
                settings::setting_delete
            ],
        )
        .attach(cors::cors(&config))
        .register(catchers![not_modified, not_found])
        .manage(server_state)
}
