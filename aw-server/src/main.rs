#[macro_use]
extern crate log;

use aw_server::*;

fn main() {
    use std::sync::Mutex;

    logging::setup_logger().expect("Failed to setup logging");

    let config = config::get_config();

    let db_path = dirs::db_path().to_str().unwrap().to_string();
    info!("Using DB at path {:?}", db_path);

    let asset_path = get_asset_path();
    info!("Using aw-webui assets at path {:?}", asset_path);

    let server_state = endpoints::ServerState {
        // Even if legacy_import is set to true it is disabled on Android so
        // it will not happen there
        datastore: Mutex::new(aw_datastore::Datastore::new(db_path, true)),
        asset_path: asset_path,
    };

    endpoints::build_rocket(server_state, &config).launch();
}

use std::path::PathBuf;

// The appdirs implementation of site_data_dir is broken on computers which has flatpak installed
// as flatpak adds its ~/.local/share/flatpak/exports/share directory first and then doesn't care
// about the rest of the paths.
// This is a rewrite of site_data_dir which takes the first folder which exists out of all folders
// in XDG_DATA_DIRS
// TODO: Should we talk to upstream about this? This changes the behavior quite a lot so maybe they
// don't want this change?
fn site_data_dir(app: Option<&str>, _: Option<&str>) -> Result<PathBuf, ()> {
    use std::env;
    // Iterate over all XDG_DATA_DIRS and return first match that exists
    match env::var_os("XDG_DATA_DIRS") {
        Some(joined) => {
            for mut data_dir in env::split_paths(&joined) {
                if app.is_some() {
                    data_dir.push(app.unwrap());
                }
                if !data_dir.is_dir() {
                    continue;
                }
                return Ok(data_dir);
            }
        }
        None => {}
    };
    // If no dirs exists in XDG_DATA_DIRS, fallback to /usr/local/share
    let default = "/usr/local/share";
    let mut data_dir = PathBuf::new();
    data_dir.push(default);
    if app.is_some() {
        data_dir.push(app.unwrap());
    }
    match data_dir.is_dir() {
        true => Ok(data_dir),
        false => Err(()),
    }
}

fn get_asset_path() -> PathBuf {
    use std::env::current_exe;

    // TODO: Add cmdline arg which can override asset path?

    // Search order for asset path is:
    // 1. ./aw-webui/dist
    // 2. $current_exe_dir/aw_server_rust/static
    // 3. $XDG_DATA_DIR/aw_server_rust/static
    // 4. (fallback) ./aw-webui/dist

    // cargo_dev_path
    // (for running with cargo run)
    let cargo_dev_path = PathBuf::from("./aw-webui/dist/");
    if cargo_dev_path.as_path().exists() {
        return cargo_dev_path;
    }

    // current_exe_path
    // (for self-contained deployed binaries)
    match current_exe() {
        Ok(mut current_exe_path) => {
            current_exe_path.pop(); // remove name of executable
            current_exe_path.push("./static/");
            if current_exe_path.as_path().exists() {
                return current_exe_path;
            }
        }
        Err(_) => (),
    };

    // usr_path
    // (for linux usr installs)
    match site_data_dir(Some("aw-server"), None) {
        Ok(mut usr_path) => {
            usr_path.push("static");
            if usr_path.as_path().exists() {
                return usr_path;
            }
        }
        Err(_) => {}
    }

    warn!("Unable to find an aw-webui asset path which exists, falling back to ./aw-webui/dist");
    return cargo_dev_path;
}
