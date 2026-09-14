use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::panic;
use std::path::Path;
use std::sync::Once;

use aw_client_rust::blocking::AwClient;
use aw_server::panic_guard::catch_panic;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring, JNI_VERSION_1_6};
use jni::JNIEnv;
use serde_json::json;

use crate::{pull, pull_all, push_with_hostname};

const ANDROID_LOG_FATAL: c_int = 7;
const ANDROID_LOG_TAG: &str = "aw-sync";
/// logcat truncates a single `__android_log_write` payload around 4 KiB.
const MAX_LOG_BYTES: usize = 4000;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Write `msg` to logcat, splitting on the 4 KiB limit and stripping NULs so
/// `CString::new` cannot fail. Never panics.
fn android_log_fatal(msg: &str) {
    let Ok(tag) = CString::new(ANDROID_LOG_TAG) else {
        return;
    };
    let sanitized = msg.replace('\0', "\\0");
    let mut rest = sanitized.as_str();
    while !rest.is_empty() {
        let mut idx = rest.len().min(MAX_LOG_BYTES);
        while idx > 0 && !rest.is_char_boundary(idx) {
            idx -= 1;
        }
        if idx == 0 {
            break;
        }
        if let Ok(text) = CString::new(&rest[..idx]) {
            unsafe {
                __android_log_write(ANDROID_LOG_FATAL, tag.as_ptr(), text.as_ptr());
            }
        }
        rest = &rest[idx..];
    }
}

/// Install a panic hook that writes to logcat via `android_log`.
///
/// The default Rust hook writes to stderr, which Android discards, so a panic
/// in this `.so` currently surfaces only as `SIGABRT` with no message
/// (ActivityWatch/aw-android#220).
fn install_panic_hook() {
    panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&'static str>() {
            *s
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.as_str()
        } else {
            "Box<dyn Any>"
        };
        let location = match info.location() {
            Some(loc) => format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
            None => "unknown location".to_string(),
        };
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        let header = format!("Rust panic in thread '{thread_name}' at {location}: {payload}");

        android_log_fatal(&header);
        error!("{}", header);

        let backtrace =
            panic::catch_unwind(|| format!("{}", std::backtrace::Backtrace::force_capture()));
        if let Ok(bt) = backtrace {
            android_log_fatal("Backtrace:");
            for line in bt.lines() {
                android_log_fatal(line);
            }
        }
    }));
}

/// Route `log` macros to logcat and install the panic hook. Safe to call more than once.
fn init_android_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag(ANDROID_LOG_TAG),
        );
        install_panic_hook();
        info!("aw-sync: android_logger and panic hook installed");
    });
}

/// Called automatically when `System.loadLibrary("aw_sync")` loads this `.so`,
/// so panics are diagnosable even if a JNI entry point is never reached.
#[no_mangle]
pub extern "system" fn JNI_OnLoad(_vm: *mut jni::sys::JavaVM, _reserved: *mut c_void) -> jint {
    // Guarded like the other entry points: this is the one that runs before any
    // logging exists, so an unwind out of it would abort the app during
    // `System.loadLibrary("aw_sync")` with nothing in logcat at all.
    let _ = catch_panic("JNI_OnLoad", init_android_logging);
    JNI_VERSION_1_6
}

/// Helper function to convert Rust string to Java string.
///
/// Must not panic: it is also used on the guard's error path, where a second
/// unwind would reach the `extern "C"` frame and abort after all. A null return
/// surfaces in Kotlin as a `NullPointerException` at the call site, which
/// `SyncInterface.performSyncAsync` already catches — recoverable, unlike
/// `SIGABRT`.
fn rust_string_to_jstring(env: &JNIEnv, s: String) -> jstring {
    match env.new_string(s) {
        Ok(output) => output.into_raw(),
        Err(e) => {
            error!("Couldn't create java string: {}", e);
            std::ptr::null_mut()
        }
    }
}

/// The response shape `SyncInterface.performSyncAsync` parses: it reads
/// `success` and then either `message` or `error`.
fn sync_error_json(msg: &str) -> String {
    json!({
        "success": false,
        "error": msg
    })
    .to_string()
}

/// Run a `jstring`-returning JNI entry point with panics caught.
///
/// A panic that unwinds out of an `extern "C"` function aborts the process
/// (Rust >= 1.81), which is how a plain `unwrap()` on the sync path takes the
/// whole app down — see ActivityWatch/aw-android#220 and #267. Catching it here
/// turns that abort into the ordinary `{"success": false, "error": ...}` object
/// the Kotlin caller already handles.
///
/// The panic hook installed by `init_android_logging` still runs first, so the
/// message and backtrace reach logcat before the unwind is stopped.
fn jni_guard<F>(env: &mut JNIEnv, name: &str, f: F) -> jstring
where
    F: FnOnce(&mut JNIEnv) -> jstring,
{
    match catch_panic(name, || {
        init_android_logging();
        f(&mut *env)
    }) {
        Ok(result) => result,
        Err(msg) => {
            error!("{}", msg);
            android_log_fatal(&msg);
            // A panic in the middle of a JNI call can leave a pending Java
            // exception, which would make the following NewStringUTF fail.
            let _ = env.exception_clear();
            rust_string_to_jstring(env, sync_error_json(&msg))
        }
    }
}

/// Run a `void` JNI entry point with panics caught. There is no return value to
/// carry an error, so the panic is logged and swallowed.
fn jni_guard_void<F>(name: &str, f: F)
where
    F: FnOnce(),
{
    if let Err(msg) = catch_panic(name, || {
        init_android_logging();
        f();
    }) {
        error!("{}", msg);
        android_log_fatal(&msg);
    }
}

/// Point this library's `ANDROID_DATA_DIR` at the app filesDir.
///
/// `libaw_sync.so` and `libaw_server.so` are separate cdylibs, so
/// `RustInterface.setDataDir` does not update the copy compiled into this
/// `.so`. SyncInterface.kt already sets `XDG_DATA_HOME=$filesDir/data`, which
/// is the path that works for debug (`applicationIdSuffix ".debug"`) and
/// work-profile installs — the hardcoded default is only the release user-0
/// path.
fn apply_android_data_dir_from_env() {
    let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") else {
        return;
    };
    let Some(files_dir) = crate::dirs::files_dir_from_xdg_data_home(Path::new(&xdg_data)) else {
        warn!(
            "XDG_DATA_HOME={} is not $filesDir/data; leaving android data dir unchanged",
            xdg_data
        );
        return;
    };
    let path = files_dir.to_string_lossy();
    info!("android data dir from XDG_DATA_HOME: {}", path);
    aw_server::dirs::set_android_data_dir(&path);
}

/// Mirror of `RustInterface.setDataDir`. Prefer this explicit path; the XDG
/// fallback in `get_client` covers current Kotlin that does not call it.
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_setDataDir(
    mut env: JNIEnv,
    _class: JClass,
    java_dir: JString,
) {
    jni_guard_void("setDataDir", || match env.get_string(&java_dir) {
        Ok(s) => {
            let path: String = s.into();
            info!("Setting android data dir as {}", path);
            aw_server::dirs::set_android_data_dir(&path);
        }
        Err(e) => {
            error!("setDataDir: failed to read path: {}", e);
        }
    });
}

/// Helper function to get AwClient from port.
///
/// Android enables API-key auth whenever `config.toml` has `[auth].api_key`.
/// The desktop CLI path (`main.rs`) already forwards that key; this JNI path
/// used `AwClient::new()` and 401'd on `GET /api/0/buckets` (aw-android#247).
fn get_client(port: i32) -> Result<AwClient, String> {
    apply_android_data_dir_from_env();
    let host = "127.0.0.1";
    let api_key = match crate::util::get_server_config(false, None) {
        Ok(cfg) => {
            if cfg.api_key.is_some() {
                info!("using API key from config.toml for local client");
            }
            cfg.api_key
        }
        Err(e) => {
            warn!("failed to read server config for API key: {}", e);
            None
        }
    };
    AwClient::new_with_api_key(host, port as u16, "aw-sync-android", api_key)
        .map_err(|e| format!("Failed to create client: {}", e))
}

/// Render a sync result as the JSON object the Kotlin side parses.
fn sync_result_to_jstring(env: &JNIEnv, name: &str, result: Result<String, String>) -> jstring {
    match result {
        Ok(msg) => rust_string_to_jstring(env, msg),
        Err(e) => {
            error!("{} error: {}", name, e);
            rust_string_to_jstring(env, sync_error_json(&e))
        }
    }
}

/// Pull sync data from all hosts in the sync directory
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPullAll(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    // syncPullAll iterates every host found in the sync directory, so the
    // hostname the Kotlin side passes is unused. Kept for signature parity with
    // the other SyncInterface natives.
    _hostname: JString,
) -> jstring {
    jni_guard(&mut env, "syncPullAll", |env| {
        let result: Result<String, String> = (|| {
            let client = get_client(port)?;
            pull_all(&client).map_err(|e| format!("Sync pull failed: {}", e))?;
            Ok(json!({
                "success": true,
                "message": "Successfully pulled from all hosts"
            })
            .to_string())
        })();
        sync_result_to_jstring(env, "syncPullAll", result)
    })
}

/// Pull sync data from a specific host
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPull(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    jni_guard(&mut env, "syncPull", |env| {
        let result: Result<String, String> = (|| {
            let client = get_client(port)?;
            let hostname_str: String = env
                .get_string(&hostname)
                .map_err(|e| format!("Failed to get hostname string: {}", e))?
                .into();

            pull(&hostname_str, &client).map_err(|e| format!("Sync pull failed: {}", e))?;

            Ok(json!({
                "success": true,
                "message": format!("Successfully pulled from host: {}", hostname_str)
            })
            .to_string())
        })();
        sync_result_to_jstring(env, "syncPull", result)
    })
}

/// Push local sync data to the sync directory
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPush(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    jni_guard(&mut env, "syncPush", |env| {
        let result: Result<String, String> = (|| {
            let hostname_str: String = env
                .get_string(&hostname)
                .map_err(|e| format!("Failed to get hostname: {}", e))?
                .into();
            let client = get_client(port)?;
            push_with_hostname(&client, &hostname_str)
                .map_err(|e| format!("Sync push failed: {}", e))?;
            Ok(json!({
                "success": true,
                "message": "Successfully pushed local data"
            })
            .to_string())
        })();
        sync_result_to_jstring(env, "syncPush", result)
    })
}

/// Perform full sync (pull from all hosts, then push local data)
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncBoth(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    jni_guard(&mut env, "syncBoth", |env| {
        let result: Result<String, String> = (|| {
            let hostname_str: String = env
                .get_string(&hostname)
                .map_err(|e| format!("Failed to get hostname: {}", e))?
                .into();
            let client = get_client(port)?;

            pull_all(&client).map_err(|e| format!("Pull phase failed: {}", e))?;

            push_with_hostname(&client, &hostname_str)
                .map_err(|e| format!("Push phase failed: {}", e))?;

            Ok(json!({
                "success": true,
                "message": "Successfully completed full sync"
            })
            .to_string())
        })();
        sync_result_to_jstring(env, "syncBoth", result)
    })
}

/// Get the sync directory path
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_getSyncDir(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(
        &mut env,
        "getSyncDir",
        |env| match crate::dirs::get_sync_dir() {
            Ok(path) => {
                let path_str = path.to_string_lossy().to_string();
                let response = json!({
                    "success": true,
                    "path": path_str
                })
                .to_string();
                rust_string_to_jstring(env, response)
            }
            Err(e) => {
                let msg = format!("Failed to get sync dir: {}", e);
                error!("getSyncDir error: {}", msg);
                rust_string_to_jstring(env, sync_error_json(&msg))
            }
        },
    )
}
