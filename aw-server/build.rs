use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let webui_var = std::env::var("AW_WEBUI_DIR");
    let path = if let Ok(var_path) = &webui_var {
        std::path::Path::new(var_path)
    } else {
        let path = std::path::Path::new("../aw-webui/dist");
        // ensure folder exists, since macro requires it
        std::fs::create_dir_all(path)?;
        println!("cargo:rustc-env=AW_WEBUI_DIR={}", path.display());
        path
    };

    let path_index = path.join("index.html");
    if !path_index.exists() {
        println!(
            "cargo:warning=`{}` is not built, compiling without webui",
            path.display()
        );
    }

    Ok(())
}
