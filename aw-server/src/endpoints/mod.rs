use std::path::PathBuf;
use std::sync::Mutex;

use gethostname::gethostname;
use rocket::get;
use rocket::response::NamedFile;
use rocket::State;
use rocket_contrib::json::Json;
use rocket_okapi::swagger_ui::{make_swagger_ui, SwaggerUIConfig};
use rocket_okapi::{openapi, routes_with_openapi};

use crate::config::AWConfig;

use aw_datastore::Datastore;
use aw_models::Info;

pub struct ServerState {
    pub datastore: Mutex<Datastore>,
    pub asset_path: PathBuf,
    pub device_id: String,
}

#[macro_use]
mod util;
mod bucket;
mod cors;
mod export;
mod import;
mod query;
mod settings;

pub use util::HttpErrorJson;

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

#[openapi]
#[get("/")]
fn server_info(config: State<AWConfig>, state: State<ServerState>) -> Json<Info> {
    #[allow(clippy::or_fun_call)]
    let hostname = gethostname().into_string().unwrap_or("unknown".to_string());
    const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

    Json(Info {
        hostname,
        version: format!("v{} (rust)", VERSION.unwrap_or("(unknown)")),
        testing: config.testing,
        device_id: state.device_id.clone(),
    })
}

fn get_docs() -> SwaggerUIConfig {
    use rocket_okapi::swagger_ui::UrlObject;

    SwaggerUIConfig {
        url: "/info/openapi.json".to_string(),
        urls: vec![
            UrlObject::new("Info", "/api/0/info/openapi.json"),
            UrlObject::new("Bucket", "/api/0/buckets/openapi.json"),
            UrlObject::new("Export", "/api/0/export/openapi.json"),
        ],
        ..Default::default()
    }
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
        .mount("/api/0/info", routes_with_openapi![server_info])
        .mount(
            "/api/0/buckets",
            routes_with_openapi![
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
        .mount(
            "/api/0/export",
            routes_with_openapi![export::buckets_export],
        )
        .mount(
            "/api/0/settings",
            routes![
                settings::setting_get,
                settings::settings_list_get,
                settings::setting_set,
                settings::setting_delete
            ],
        )
        .mount("/api", make_swagger_ui(&get_docs()))
        .attach(cors::cors(&config))
        .manage(server_state)
        .manage(config)
}
