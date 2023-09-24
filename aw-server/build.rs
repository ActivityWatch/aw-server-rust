use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    // if aw-webui/dist does not exist or is empty, print a warning
    let path = std::path::Path::new("../aw-webui/dist");
    if !path.exists() || path.read_dir()?.next().is_none() {
        println!("cargo:warning=`./aw-webui/dist` is not built, compiling without webui");
    }

    // ensure folder exists, since macro requires it
    std::fs::create_dir_all(path)?;
    println!("cargo:rustc-env=AW_WEBUI_DIR=../aw-webui/dist");

    Ok(())
}
