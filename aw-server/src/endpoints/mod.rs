use std::path::PathBuf;
use std::sync::Mutex;

use gethostname::gethostname;
use rocket::http::Status;
use rocket::response::NamedFile;
use rocket::State;
use rocket_contrib::json::JsonValue;

use crate::config::AWConfig;

use aw_datastore::Datastore;

pub struct ServerState {
    pub datastore: Mutex<Datastore>,
    pub asset_path: PathBuf,
    pub device_id: String,
}

use rocket::http::ContentType;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use std::io::Cursor;

#[derive(Serialize, Debug)]
pub struct HttpErrorJson {
    #[serde(skip_serializing)]
    status: Status,
    message: String,
}

impl HttpErrorJson {
    pub fn new(status: Status, err: String) -> HttpErrorJson {
        HttpErrorJson {
            status: status,
            message: format!("{}", err),
        }
    }
}

impl<'r> Responder<'r> for HttpErrorJson {
    fn respond_to(self, _: &Request) -> response::Result<'r> {
        Response::build()
            .status(self.status)
            .sized_body(Cursor::new(format!("{{\"message\":\"{}\"}}", self.message)))
            .header(ContentType::new("application", "json"))
            .ok()
    }
}

#[macro_export]
macro_rules! endpoints_get_lock {
    ( $lock:expr ) => {
        match $lock.lock() {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("Taking datastore lock failed, returning 504: {}", e);
                warn!("{}", err_msg);
                return Err(HttpErrorJson::new(Status::ServiceUnavailable, err_msg));
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

#[get("/")]
fn server_info(config: State<AWConfig>, state: State<ServerState>) -> JsonValue {
    #[allow(clippy::or_fun_call)]
    let hostname = gethostname().into_string().unwrap_or("unknown".to_string());
    const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

    json!({
        "hostname": hostname,
        "version": format!("v{} (rust)", VERSION.unwrap_or("(unknown)")),
        "testing": config.testing,
        "device_id": state.device_id,
    })
}

pub fn build_rocket(server_state: ServerState, config: AWConfig) -> rocket::Rocket {
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
        .manage(server_state)
        .manage(config)
}
