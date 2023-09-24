use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all("../aw-webui/dist").unwrap();
    println!("cargo:rustc-env=AW_WEBUI_DIR=../aw-webui/dist");

    Ok(())
}
