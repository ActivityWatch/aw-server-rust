#[macro_use] extern crate log;

use aw_server::*;

fn main() {
    use std::path::PathBuf;
    use std::sync::Mutex;

    logging::setup_logger().expect("Failed to setup logging");

    let config = config::get_config();

    let db_path = dirs::db_path().to_str().unwrap().to_string();
    info!("Using DB at path {:?}", db_path);

    let server_state = endpoints::ServerState {
        datastore: Mutex::new(datastore::Datastore::new(db_path)),
        asset_path: PathBuf::from("aw-webui").join("dist"),
    };

    endpoints::build_rocket(server_state, &config).launch();
}
