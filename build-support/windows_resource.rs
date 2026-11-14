// Shared by the aw-server and aw-sync build scripts via `include!`.
//
// Embeds a PE version resource (VS_VERSION_INFO) in the Windows executables.
// Code-signing services (e.g. SignPath Foundation) require consistent product
// name/version metadata on every signed binary.
//
// The resource is linked only into the named binary target, never into the
// library, so crates that depend on aw-server (e.g. aw-tauri) do not pick up a
// second VERSIONINFO resource. Does nothing when the target is not Windows.

/// `AW_VERSION` (the ActivityWatch release version, e.g. "0.14.2") overrides
/// the crate version as the product version, so all bundled executables agree.
fn embed_windows_version_resource(bin: &str, description: &str) {
    println!("cargo:rerun-if-env-changed=AW_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let file_version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let product_version = std::env::var("AW_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| file_version.clone());

    let rc = format!(
        r#"#include <winver.h>

VS_VERSION_INFO VERSIONINFO
FILEVERSION {file_nums}
PRODUCTVERSION {product_nums}
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "ActivityWatch Contributors"
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{file_version}"
            VALUE "InternalName", "{bin}"
            VALUE "OriginalFilename", "{bin}.exe"
            VALUE "ProductName", "ActivityWatch"
            VALUE "ProductVersion", "{product_version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        file_nums = version_numbers(&file_version),
        product_nums = version_numbers(&product_version),
    );

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join(format!("{bin}-version.rc"));
    std::fs::write(&rc_path, rc).expect("failed to write version resource script");
    embed_resource::compile_for(&rc_path, [bin], embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows version resource");
}

/// Leading numeric part as four comma-separated numbers: "0.14.0b5" -> "0,14,0,0".
fn version_numbers(version: &str) -> String {
    let prefix: String = version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut nums: Vec<u16> = prefix
        .split('.')
        .filter_map(|n| n.parse().ok())
        .take(4)
        .collect();
    nums.resize(4, 0);
    nums.iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
