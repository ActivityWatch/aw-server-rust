//! Version string reported by `GET /api/0/info`.
//!
//! By default this is the aw-server-rust package version. That is wrong for
//! builds that embed aw-server-rust as a component of a larger product: on
//! Android the webui footer showed `v0.14.0 (rust)` (the Cargo.toml version)
//! while the installed app was `v0.14.0b2`.
//!
//! Such builds call [`set_version_override`] at startup to report their own
//! release version instead.

use std::sync::RwLock;

static VERSION_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// Report `version` from `/api/0/info` instead of the package version.
///
/// The string is used verbatim, so the caller controls the exact format
/// (including any `v` prefix).
pub fn set_version_override(version: &str) {
    let mut guard = VERSION_OVERRIDE.write().unwrap();
    *guard = Some(version.to_string());
}

/// The version string to report from `/api/0/info`.
pub fn version_string() -> String {
    if let Some(version) = VERSION_OVERRIDE.read().unwrap().as_ref() {
        return version.clone();
    }
    const VERSION: Option<&str> = option_env!("CARGO_PKG_VERSION");
    format!("v{} (rust)", VERSION.unwrap_or("(unknown)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct VersionGuard;
    impl Drop for VersionGuard {
        fn drop(&mut self) {
            *VERSION_OVERRIDE.write().unwrap() = None;
        }
    }

    // These share process-global state, so they run as one test.
    #[test]
    fn override_replaces_package_version() {
        let _guard = VersionGuard; // resets VERSION_OVERRIDE on exit, even on panic

        assert!(
            version_string().ends_with(" (rust)"),
            "default should report the package version, got {:?}",
            version_string()
        );

        set_version_override("v0.14.0b2");
        assert_eq!(version_string(), "v0.14.0b2");

        // Used verbatim: the caller owns the format.
        set_version_override("1.2.3-custom");
        assert_eq!(version_string(), "1.2.3-custom");
    }
}
