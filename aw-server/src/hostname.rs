//! Hostname reported by `GET /api/0/info`.
//!
//! By default this is the OS hostname from `gethostname()`. That is wrong for
//! Android: the kernel hostname is typically `localhost`, while buckets, sync,
//! and the Activity route use the sanitized device name (`deviceHostname()`).
//! The webui "this device" badge compares `info.hostname` to each bucket's
//! hostname, so the badge never matches the real device.
//!
//! Android calls [`set_hostname_override`] at startup with that device name.

use std::sync::RwLock;

static HOSTNAME_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// Report `hostname` from `/api/0/info` instead of the OS hostname.
///
/// The string is used verbatim, so the caller owns sanitization (Android
/// already sanitizes via `deviceHostname()`).
pub fn set_hostname_override(hostname: &str) {
    let mut guard = HOSTNAME_OVERRIDE.write().unwrap();
    *guard = Some(hostname.to_string());
}

/// The hostname string to report from `/api/0/info`.
pub fn hostname_string() -> String {
    if let Some(hostname) = HOSTNAME_OVERRIDE.read().unwrap().as_ref() {
        return hostname.clone();
    }
    gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HostnameGuard;
    impl Drop for HostnameGuard {
        fn drop(&mut self) {
            *HOSTNAME_OVERRIDE.write().unwrap() = None;
        }
    }

    #[test]
    fn override_replaces_os_hostname() {
        let _guard = HostnameGuard;

        let os_hostname = hostname_string();
        assert!(
            !os_hostname.is_empty(),
            "default should report an OS hostname or unknown"
        );

        set_hostname_override("poco_f8_ultra");
        assert_eq!(hostname_string(), "poco_f8_ultra");

        set_hostname_override("pixel_8");
        assert_eq!(hostname_string(), "pixel_8");
    }
}
