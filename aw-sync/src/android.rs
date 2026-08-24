use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::panic;
use std::sync::Once;

use aw_client_rust::blocking::AwClient;
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
    init_android_logging();
    JNI_VERSION_1_6
}

/// Helper function to convert Rust string to Java string
fn rust_string_to_jstring(env: &JNIEnv, s: String) -> jstring {
    let output = env.new_string(s).expect("Couldn't create java string!");
    output.into_raw()
}

/// Helper function to get AwClient from port
fn get_client(port: i32) -> Result<AwClient, String> {
    let host = "127.0.0.1";
    AwClient::new(host, port as u16, "aw-sync-android")
        .map_err(|e| format!("Failed to create client: {}", e))
}

/// Pull sync data from all hosts in the sync directory
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPullAll(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    init_android_logging();
    let hostname_str: String = match env.get_string(&hostname) {
        Ok(s) => s.into(),
        Err(e) => {
            let error_msg = format!("Failed to get hostname: {}", e);
            error!("syncPullAll: {}", error_msg);
            return rust_string_to_jstring(
                &env,
                json!({
                    "success": false,
                    "error": error_msg
                })
                .to_string(),
            );
        }
    };

    let result: Result<String, String> = (|| {
        let client = get_client(port)?;
        pull_all(&client).map_err(|e| format!("Sync pull failed: {}", e))?;
        Ok(json!({
            "success": true,
            "message": "Successfully pulled from all hosts"
        })
        .to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPullAll error: {}", e);
            let error_msg: &str = &e;
            let error_json = json!({
                "success": false,
                "error": error_msg
            })
            .to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Pull sync data from a specific host
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPull(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    init_android_logging();
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

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPull error: {}", e);
            let error_msg: &str = &e;
            let error_json = json!({
                "success": false,
                "error": error_msg
            })
            .to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Push local sync data to the sync directory
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPush(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    init_android_logging();
    let hostname_str: String = match env.get_string(&hostname) {
        Ok(s) => s.into(),
        Err(e) => {
            let error_msg = format!("Failed to get hostname: {}", e);
            error!("syncPush: {}", error_msg);
            return rust_string_to_jstring(
                &env,
                json!({
                    "success": false,
                    "error": error_msg
                })
                .to_string(),
            );
        }
    };

    let result: Result<String, String> = (|| {
        let client = get_client(port)?;
        push_with_hostname(&client, &hostname_str)
            .map_err(|e| format!("Sync push failed: {}", e))?;
        Ok(json!({
            "success": true,
            "message": "Successfully pushed local data"
        })
        .to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPush error: {}", e);
            let error_msg: &str = &e;
            let error_json = json!({
                "success": false,
                "error": error_msg
            })
            .to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Perform full sync (pull from all hosts, then push local data)
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncBoth(
    mut env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    init_android_logging();
    let hostname_str: String = match env.get_string(&hostname) {
        Ok(s) => s.into(),
        Err(e) => {
            let error_msg = format!("Failed to get hostname: {}", e);
            error!("syncBoth: {}", error_msg);
            return rust_string_to_jstring(
                &env,
                json!({
                    "success": false,
                    "error": error_msg
                })
                .to_string(),
            );
        }
    };

    let result: Result<String, String> = (|| {
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

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncBoth error: {}", e);
            let error_msg: &str = &e;
            let error_json = json!({
                "success": false,
                "error": error_msg
            })
            .to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Get the sync directory path
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_getSyncDir(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    init_android_logging();
    let result = crate::dirs::get_sync_dir();

    match result {
        Ok(path) => {
            let path_str = path.to_string_lossy().to_string();
            let response = json!({
                "success": true,
                "path": path_str
            })
            .to_string();
            rust_string_to_jstring(&env, response)
        }
        Err(e) => {
            let error_json = json!({
                "success": false,
                "error": format!("Failed to get sync dir: {}", e)
            })
            .to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}
