use std::path::PathBuf;
use std::sync::Mutex;

use gethostname::gethostname;
use rocket::fs::NamedFile;
use rocket::serde::json::Json;
use rocket::State;

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
mod auth;
mod bucket;
mod cors;
mod export;
mod hostcheck;
mod import;
mod query;
mod settings;

pub use util::HttpErrorJson;

#[get("/")]
async fn root_index(state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("index.html"))
        .await
        .ok()
}

#[get("/css/<file..>")]
async fn root_css(file: PathBuf, state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("css").join(file))
        .await
        .ok()
}

#[get("/fonts/<file..>")]
async fn root_fonts(file: PathBuf, state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("fonts").join(file))
        .await
        .ok()
}

#[get("/js/<file..>")]
async fn root_js(file: PathBuf, state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("js").join(file))
        .await
        .ok()
}

#[get("/static/<file..>")]
async fn root_static(file: PathBuf, state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("static").join(file))
        .await
        .ok()
}

#[get("/favicon.ico")]
async fn root_favicon(state: &State<ServerState>) -> Option<NamedFile> {
    NamedFile::open(state.asset_path.join("favicon.ico"))
        .await
        .ok()
}

#[get("/")]
fn server_info(config: &State<AWConfig>, state: &State<ServerState>) -> Json<Info> {
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

pub fn build_rocket(server_state: ServerState, config: AWConfig) -> rocket::Rocket<rocket::Build> {
    info!(
        "Starting aw-server-rust at {}:{}",
        config.address, config.port
    );
    let cors = cors::cors(&config);
    let hostcheck = hostcheck::HostCheck::new(&config);
    rocket::custom(config.to_rocket_config())
        .attach(cors.clone())
        .attach(hostcheck)
        .manage(cors)
        .manage(server_state)
        .manage(config)
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
                bucket::bucket_events_get_single,
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
        .mount("/", rocket_cors::catch_all_options_routes())
}
