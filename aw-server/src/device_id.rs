use std::fs;

use uuid::Uuid;

use crate::dirs;

/// Retrieves the device ID, if none exists it generates one (using UUID v4)
pub fn get_device_id() -> String {
    // I chose get_data_dir over get_config_dir since the latter isn't yet supported on Android.
    let mut path = dirs::get_data_dir().unwrap();
    path.push("device_id");
    if path.exists() {
        fs::read_to_string(path).unwrap()
    } else {
        let uuid = Uuid::new_v4().as_hyphenated().to_string();
        fs::write(path, &uuid).unwrap();
        uuid
    }
}
