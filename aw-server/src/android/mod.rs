// Based On the following guide from Mozilla:
//   https://mozilla.github.io/firefox-browser-architecture/experiments/2017-09-21-rust-on-android.html

extern crate android_logger;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use crate::device_id;
use crate::dirs;

use android_logger::Config;
use rocket::serde::json::json;

#[no_mangle]
pub extern "C" fn rust_greeting(to: *const c_char) -> *mut c_char {
    let c_str = unsafe { CStr::from_ptr(to) };
    let recipient = match c_str.to_str() {
        Err(_) => "there",
        Ok(string) => string,
    };

    CString::new("Hello ".to_owned() + recipient + " (from Rust!)")
        .unwrap()
        .into_raw()
}

#[cfg(target_os = "android")]
#[allow(non_snake_case)]
pub mod android {
    extern crate jni;

    use self::jni::objects::{JClass, JString};
    use self::jni::sys::{jdouble, jint, jstring};
    use self::jni::JNIEnv;
    use super::*;

    use crate::panic_guard::catch_panic;

    use crate::endpoints;
    use crate::endpoints::ServerState;
    use aw_client_rust::classes::{classes_from_settings_str, default_classes};
    use aw_client_rust::queries::{
        build_android_canonical_events, AndroidQueryParams, QueryParamsBase,
    };
    use aw_datastore::Datastore;
    use aw_models::{Bucket, Event, TimeInterval};

    static mut DATASTORE: Option<Datastore> = None;

    unsafe fn openDatastore() -> Datastore {
        match DATASTORE {
            Some(ref ds) => ds.clone(),
            None => {
                let db_dir = dirs::db_path("default")
                    .expect("Failed to get db path")
                    .to_str()
                    .unwrap()
                    .to_string();
                DATASTORE = Some(Datastore::new(db_dir, false));
                openDatastore()
            }
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_greeting(
        env: JNIEnv,
        _: JClass,
        java_pattern: JString,
    ) -> jstring {
        jni_guard(env, "greeting", || {
            // Our Java companion code might pass-in "world" as a string, hence the name.
            let world = rust_greeting(
                env.get_string(java_pattern)
                    .expect("invalid pattern string")
                    .as_ptr(),
            );
            // Retake pointer so that we can use it below and allow memory to be freed when it goes out of scope.
            let world_ptr = CString::from_raw(world);
            let output = env
                .new_string(world_ptr.to_str().unwrap())
                .expect("Couldn't create java string!");

            output.into_raw()
        })
    }

    unsafe fn jstring_to_string(env: &JNIEnv, string: JString) -> String {
        let jstr = env.get_string(string).expect("Failed to get Java string");
        jstr.into()
    }

    unsafe fn string_to_jstring(env: &JNIEnv, string: String) -> jstring {
        env.new_string(string)
            .expect("Couldn't create java string")
            .into_raw()
    }

    unsafe fn create_error_object(env: &JNIEnv, msg: String) -> jstring {
        let obj = json!({ "error": &msg });
        string_to_jstring(&env, obj.to_string())
    }

    /// Run a `jstring`-returning JNI entry point with panics caught.
    ///
    /// Since Rust 1.81 a panic that unwinds out of an `extern "C"` function
    /// aborts the process, so any `unwrap()`/`expect()` reached from one of
    /// these natives kills the app with `SIGABRT` instead of failing the call.
    /// Those are the `libaw_server.so` → `SIGABRT` clusters in
    /// ActivityWatch/aw-android#267. Catching the unwind here returns the same
    /// `{"error": "…"}` object the callers already produce for ordinary
    /// failures, which `RustInterface` parses as a normal `JSONObject`.
    ///
    /// `log_panics::init()` (installed in `initialize`) still logs the panic and
    /// its backtrace to logcat before the unwind is stopped, so nothing is
    /// hidden.
    unsafe fn jni_guard<F>(env: JNIEnv, name: &str, f: F) -> jstring
    where
        F: FnOnce() -> jstring,
    {
        match catch_panic(name, f) {
            Ok(result) => result,
            Err(msg) => {
                error!("{}", msg);
                // A panic mid-JNI-call can leave a pending Java exception, which
                // would make the NewStringUTF below fail.
                let _ = env.exception_clear();
                // Building the error object allocates a Java string, which can
                // itself fail; never let that second failure unwind out of the
                // `extern "C"` frame. A null return surfaces as a Java-level
                // NullPointerException, which is recoverable, unlike SIGABRT.
                catch_panic(name, || create_error_object(&env, msg))
                    .unwrap_or_else(|_| std::ptr::null_mut())
            }
        }
    }

    /// Run a `void` JNI entry point with panics caught. There is no return value
    /// to carry an error, so the panic is logged and swallowed.
    unsafe fn jni_guard_void<F>(name: &str, f: F)
    where
        F: FnOnce(),
    {
        if let Err(msg) = catch_panic(name, f) {
            error!("{}", msg);
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_startServer(
        _env: JNIEnv,
        _: JClass,
        port: jint,
    ) {
        jni_guard_void("startServer", || {
            // `jint` is i32, so reject values that cannot be a listening port
            // instead of letting `as u16` silently wrap (65536 -> 0 -> ephemeral
            // port) or truncate a negative value.
            let port = match u16::try_from(port) {
                Ok(p) if p != 0 => p,
                _ => {
                    error!("startServer: invalid port {}; refusing to start", port);
                    return;
                }
            };
            info!("Starting server on port {}...", port);
            start_server(port);
            info!("Server exited");
        });
    }

    fn start_server(port: u16) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(start_server_impl(port));
    }

    async fn start_server_impl(port: u16) {
        info!("Building server state...");

        // FIXME: Why is unsafe needed here? Can we get rid of it?
        unsafe {
            let server_state: ServerState = endpoints::ServerState {
                datastore: openDatastore(),
                asset_resolver: endpoints::AssetResolver::new(None),
                device_id: device_id::get_device_id(),
            };
            info!("Using server_state:: device_id: {}", server_state.device_id);

            let mut server_config = crate::config::create_config("default", None);
            server_config.port = port;

            let _ = endpoints::build_rocket(server_state, server_config)
                .launch()
                .await;
        }
    }

    static mut INITIALIZED: bool = false;

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_initialize(
        env: JNIEnv,
        _: JClass,
    ) {
        jni_guard_void("initialize", || {
            if !INITIALIZED {
                android_logger::init_once(
                    Config::default()
                        .with_max_level(log::LevelFilter::Info) // limit log level
                        .with_tag("aw-server-rust"), // logs will show under mytag tag
                                                     //.with_filter( // configure messages for specific crate
                                                     //    FilterBuilder::new()
                                                     //        .parse("debug,hello::crate=error")
                                                     //        .build())
                );
                // Default panic hook writes to stderr, which Android discards
                // (ActivityWatch/aw-android#220). log_panics routes them through
                // android_logger so they appear in logcat.
                log_panics::init();
                info!("Initializing aw-server-rust...");
                debug!("Redirected aw-server-rust stdout/stderr to logcat");
            } else {
                info!("Already initialized");
            }
            INITIALIZED = true;

            // Without this it might not work due to weird error probably arising from Rust optimizing away the JNIEnv:
            //  JNI DETECTED ERROR IN APPLICATION: use of deleted weak global reference
            string_to_jstring(&env, "test".to_string());
        });
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_setDataDir(
        env: JNIEnv,
        _: JClass,
        java_dir: JString,
    ) {
        jni_guard_void("setDataDir", || {
            let path = &jstring_to_string(&env, java_dir);
            debug!("Setting android data dir as {}", path);
            dirs::set_android_data_dir(path);
        });
    }

    /// Report the Android app's release version from `/api/0/info` instead of
    /// the aw-server-rust package version, which is the version of a component
    /// rather than of the app the user installed.
    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_setVersionOverride(
        env: JNIEnv,
        _: JClass,
        java_version: JString,
    ) {
        jni_guard_void("setVersionOverride", || {
            let version = &jstring_to_string(&env, java_version);
            debug!("Setting reported version to {}", version);
            crate::version::set_version_override(version);
        });
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_getBuckets(
        env: JNIEnv,
        _: JClass,
    ) -> jstring {
        jni_guard(env, "getBuckets", || {
            // Return an error object instead of unwrapping: if the datastore worker
            // is gone (it panicked, e.g. the database could not be opened), the
            // request fails with SendError/RecvError and a panic here unwinds across
            // the JNI boundary on whatever thread called getBuckets — usually main.
            match openDatastore().get_buckets() {
                Ok(buckets) => string_to_jstring(&env, json!(buckets).to_string()),
                Err(e) => create_error_object(&env, format!("Failed to get buckets: {e:?}")),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_createBucket(
        env: JNIEnv,
        _: JClass,
        java_bucket: JString,
    ) -> jstring {
        jni_guard(env, "createBucket", || {
            let bucket = jstring_to_string(&env, java_bucket);
            let bucket_json: Bucket = match serde_json::from_str(&bucket) {
                Ok(json) => json,
                Err(err) => return create_error_object(&env, err.to_string()),
            };
            match openDatastore().create_bucket(&bucket_json) {
                Ok(()) => string_to_jstring(&env, "Bucket successfully created".to_string()),
                Err(e) => create_error_object(
                    &env,
                    format!("Something went wrong when trying to create bucket: {:?}", e),
                ),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_heartbeat(
        env: JNIEnv,
        _: JClass,
        java_bucket_id: JString,
        java_event: JString,
        java_pulsetime: jdouble,
    ) -> jstring {
        jni_guard(env, "heartbeat", || {
            let bucket_id = jstring_to_string(&env, java_bucket_id);
            let event = jstring_to_string(&env, java_event);
            let pulsetime = java_pulsetime as f64;
            let event_json: Event = match serde_json::from_str(&event) {
                Ok(json) => json,
                Err(err) => return create_error_object(&env, err.to_string()),
            };
            match openDatastore().heartbeat(&bucket_id, event_json, pulsetime) {
                Ok(_) => string_to_jstring(&env, "Heartbeat successfully received".to_string()),
                Err(e) => create_error_object(
                    &env,
                    format!(
                        "Something went wrong when trying to send heartbeat: {:?}",
                        e
                    ),
                ),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_getEvents(
        env: JNIEnv,
        _: JClass,
        java_bucket_id: JString,
        java_limit: jint,
    ) -> jstring {
        jni_guard(env, "getEvents", || {
            let bucket_id = jstring_to_string(&env, java_bucket_id);
            let limit = java_limit as u64;
            match openDatastore().get_events(&bucket_id, None, None, Some(limit)) {
                Ok(events) => string_to_jstring(&env, json!(events).to_string()),
                Err(e) => create_error_object(
                    &env,
                    format!("Something went wrong when trying to get events: {:?}", e),
                ),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_migrateHostname(
        env: JNIEnv,
        _: JClass,
        hostname: JString,
    ) -> jstring {
        jni_guard(env, "migrateHostname", || {
            let hostname = jstring_to_string(&env, hostname);
            if hostname.is_empty() {
                return create_error_object(&env, "hostname must not be empty".to_string());
            }
            match openDatastore().migrate_hostname(&hostname) {
                Ok(count) => {
                    string_to_jstring(&env, format!("Migrated hostname for {} bucket(s)", count))
                }
                Err(e) => create_error_object(&env, format!("Failed to migrate hostname: {:?}", e)),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_migrateAndroidBucketName(
        env: JNIEnv,
        _: JClass,
    ) -> jstring {
        jni_guard(env, "migrateAndroidBucketName", || {
            match openDatastore().rename_bucket("aw-android-test", "aw-android") {
                Ok(()) => string_to_jstring(
                    &env,
                    "Renamed bucket 'aw-android-test' to 'aw-android'".to_string(),
                ),
                Err(e) => create_error_object(&env, format!("Failed to rename bucket: {:?}", e)),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_migrateWatcherAndroidBucketNames(
        env: JNIEnv,
        _: JClass,
    ) -> jstring {
        jni_guard(
            env,
            "migrateWatcherAndroidBucketNames",
            || match openDatastore().migrate_test_bucket_names() {
                Ok(count) => string_to_jstring(
                    &env,
                    format!("Migrated {} 'aw-watcher-android-test' bucket(s)", count),
                ),
                Err(e) => create_error_object(
                    &env,
                    format!("Failed to migrate watcher bucket names: {:?}", e),
                ),
            },
        )
    }

    /// Return a raw settings JSON value (the datastore body, matching GET /api/0/settings/<key>).
    /// Missing or invalid keys return the JSON literal `null`.
    ///
    /// Widget/worker code must use this instead of unauthenticated HTTP: Android
    /// enables API-key auth by default, so GET /api/0/settings/... from the
    /// widget process 401s and silently falls back to defaults.
    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_getSetting(
        env: JNIEnv,
        _: JClass,
        java_key: JString,
    ) -> jstring {
        jni_guard(env, "getSetting", || {
            let key = jstring_to_string(&env, java_key);
            // Match GET /api/0/settings/<key>: dots are valid (nested-looking
            // keys like "foo.bar" store as settings.foo.bar). Reject empty keys
            // and path/NUL bytes so JNI cannot smuggle a lookup the HTTP router
            // would never pass through.
            if key.is_empty() || key.contains('/') || key.contains('\\') || key.contains('\0') {
                return string_to_jstring(&env, "null".to_string());
            }
            let setting_key = match crate::endpoints::settings_datastore_key(&key) {
                Ok(k) => k,
                Err(_) => return string_to_jstring(&env, "null".to_string()),
            };
            match openDatastore().get_key_value(&setting_key) {
                Ok(value) => string_to_jstring(&env, value),
                Err(_) => string_to_jstring(&env, "null".to_string()),
            }
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_query(
        env: JNIEnv,
        _: JClass,
        java_query: JString,
        java_timeperiods: JString,
    ) -> jstring {
        jni_guard(env, "query", || {
            let query_code = jstring_to_string(&env, java_query);
            let timeperiods_str = jstring_to_string(&env, java_timeperiods);
            let timeperiods: Vec<TimeInterval> = match serde_json::from_str(&timeperiods_str) {
                Ok(json) => json,
                Err(err) => return create_error_object(&env, err.to_string()),
            };

            let datastore = openDatastore();
            let mut results = Vec::new();

            for interval in &timeperiods {
                let result = match aw_query::query(&query_code, interval, &datastore) {
                    Ok(data) => data,
                    Err(e) => {
                        return create_error_object(
                            &env,
                            format!("Something went wrong when trying to query: {:?}", e),
                        )
                    }
                };
                results.push(result);
            }

            string_to_jstring(&env, json!(results).to_string())
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_androidQuery(
        env: JNIEnv,
        _: JClass,
        java_timeperiods: JString,
    ) -> jstring {
        jni_guard(env, "androidQuery", || {
            let timeperiods_str = jstring_to_string(&env, java_timeperiods);

            let timeperiods: Vec<TimeInterval> = match serde_json::from_str(&timeperiods_str) {
                Ok(json) => json,
                Err(err) => return create_error_object(&env, err.to_string()),
            };

            // Hardcoded bucket ID
            let bid_android = "aw-watcher-android".to_string();

            // Read classes from the datastore directly. Do NOT fetch them over HTTP:
            // Android enables API-key auth by default, and androidQuery runs from the
            // widget process which does not send a Bearer token. The previous
            // AwClient GET /api/0/settings/classes path 401'd (or failed if the
            // HTTP server wasn't up) and silently fell back to default_classes(),
            // which is why the homescreen widget disagreed with the Activity view
            // on per-category time while totals still matched.
            // See ActivityWatch/aw-android#142.
            let datastore = openDatastore();
            let classes = match datastore.get_key_value("settings.classes") {
                Ok(raw) => {
                    info!("Loaded classes from datastore settings.classes");
                    classes_from_settings_str(&raw)
                }
                Err(_) => {
                    info!("settings.classes unset or unreadable, using default classes");
                    default_classes()
                }
            };

            // Build canonical Android query
            let params = AndroidQueryParams {
                base: QueryParamsBase {
                    bid_browsers: Vec::new(),
                    classes,
                    filter_classes: Vec::new(),
                    filter_afk: true,
                    include_audible: true,
                },
                bid_android,
            };
            let query_code = format!(
                r#"{}
    duration = sum_durations(events);
    cat_events = sort_by_duration(merge_events_by_keys(events, ["$category"]));
    RETURN = {{"events": events, "duration": duration, "cat_events": cat_events}};"#,
                build_android_canonical_events(&params)
            );

            let mut results = Vec::new();

            for interval in &timeperiods {
                let result = match aw_query::query(&query_code, interval, &datastore) {
                    Ok(data) => data,
                    Err(e) => {
                        return create_error_object(
                            &env,
                            format!("Something went wrong when trying to query: {:?}", e),
                        )
                    }
                };
                results.push(result);
            }

            string_to_jstring(&env, json!(results).to_string())
        })
    }
}
