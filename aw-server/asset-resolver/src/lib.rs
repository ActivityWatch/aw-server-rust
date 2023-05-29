use rust_embed::RustEmbed;

pub trait AssetResolver: Send + Sync {
    fn resolve(&self, file_path: &str) -> Option<Vec<u8>>;
}

#[derive(RustEmbed)]
#[folder = "../../aw-webui/dist/"]
pub struct ProjectAssetResolver;

impl AssetResolver for ProjectAssetResolver {
    fn resolve(&self, file_path: &str) -> Option<Vec<u8>> {
        Some(ProjectAssetResolver::get(file_path)?.data.to_vec())
    }
}
