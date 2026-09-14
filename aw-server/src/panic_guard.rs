//! Catching Rust panics before they cross an FFI boundary.
//!
//! Since Rust 1.81 a panic that unwinds out of an `extern "C"` function aborts
//! the process. On Android that means every panic inside a
//! `Java_net_activitywatch_android_…` JNI entry point kills the whole app with
//! `SIGABRT` instead of surfacing as a recoverable error — which is what the
//! `libaw_server.so`/`libaw_sync.so` crash clusters in
//! ActivityWatch/aw-android#267 and the sync aborts in
//! ActivityWatch/aw-android#220 are.
//!
//! The JNI-facing wrappers in [`crate::android`] are thin: they call
//! [`catch_panic`] and turn an `Err` into the same `{"error": "…"}` object the
//! Kotlin side already parses. The logic here is platform-independent so it can
//! be unit tested on the host, where the JNI modules do not even compile.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Best-effort human-readable message for a panic payload.
///
/// `panic!` payloads are `&'static str` for literal messages and `String` for
/// formatted ones; anything else (`panic_any`) is reported by type only.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Box<dyn Any>".to_string()
    }
}

/// Run `f`, converting a panic into an `Err` describing it.
///
/// The panic hook still runs first (and on Android still logs to logcat), so
/// this does not hide the panic — it only stops the unwind before it reaches
/// the `extern "C"` frame that would abort the process.
///
/// `JNIEnv` is not `UnwindSafe`, and neither is the `&mut` reborrow the JNI
/// wrappers capture, so [`AssertUnwindSafe`] is applied here rather than at
/// every call site. That is sound for this use: on the error path the caller
/// only allocates a Java string from the env, and any Rust state the panic left
/// inconsistent (the datastore connection) is behind a channel to a separate
/// worker thread rather than shared through the closure.
pub fn catch_panic<T, F: FnOnce() -> T>(name: &str, f: F) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f))
        .map_err(|payload| format!("panic in {}: {}", name, panic_message(payload.as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_the_return_value_when_no_panic() {
        assert_eq!(catch_panic("ok", || 42), Ok(42));
    }

    #[test]
    fn reports_str_payloads() {
        let err = catch_panic("entry", || panic!("boom")).unwrap_err();
        assert_eq!(err, "panic in entry: boom");
    }

    #[test]
    fn reports_formatted_string_payloads() {
        let err = catch_panic("entry", || panic!("bad bucket {}", 7)).unwrap_err();
        assert_eq!(err, "panic in entry: bad bucket 7");
    }

    #[test]
    fn reports_unwrap_and_expect_payloads() {
        // This is the shape the sync path actually panics in: an `unwrap()` or
        // `expect()` on a datastore error deep inside the JNI call.
        fn missing() -> Option<u8> {
            None
        }
        let err = catch_panic("entry", || missing().expect("no value")).unwrap_err();
        assert!(err.starts_with("panic in entry: no value"), "{err}");

        fn failing() -> Result<u8, String> {
            Err("io".to_string())
        }
        let err = catch_panic("entry", || failing().unwrap()).unwrap_err();
        assert!(err.contains("io"), "{err}");
    }

    #[test]
    fn reports_non_string_payloads_by_placeholder() {
        let err = catch_panic("entry", || std::panic::panic_any(7u8)).unwrap_err();
        assert_eq!(err, "panic in entry: Box<dyn Any>");
    }

    #[test]
    fn allows_closures_capturing_mutable_state() {
        // Mirrors how the JNI wrappers capture a `&mut JNIEnv` reborrow: the
        // borrow must end when catch_panic returns so the error path can use it.
        let mut env = String::new();
        let result = catch_panic("entry", || {
            env.push_str("touched");
            panic!("after touching env");
        });
        assert!(result.is_err());
        assert_eq!(env, "touched");
    }
}
