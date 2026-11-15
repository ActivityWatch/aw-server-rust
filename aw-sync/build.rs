include!("../build-support/windows_resource.rs");

fn main() {
    embed_windows_version_resource("aw-sync", "ActivityWatch sync");
}
