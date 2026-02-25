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

    use std::path::PathBuf;
    use std::sync::Mutex;

    use crate::config::AWConfig;
    use crate::endpoints;
    use crate::endpoints::ServerState;
    use aw_client_rust::blocking::AwClient;
    use aw_client_rust::classes::default_classes;
    use aw_client_rust::classes::{CategoryId, CategorySpec};
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
                let db_dir = dirs::db_path(false)
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

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_startServer(
        env: JNIEnv,
        _: JClass,
    ) {
        info!("Starting server...");
        start_server();
        info!("Server exited");
    }

    #[rocket::main]
    async fn start_server() {
        info!("Building server state...");

        // FIXME: Why is unsafe needed here? Can we get rid of it?
        unsafe {
            let server_state: ServerState = endpoints::ServerState {
                datastore: Mutex::new(openDatastore()),
                asset_resolver: endpoints::AssetResolver::new(None),
                device_id: device_id::get_device_id(),
            };
            info!("Using server_state:: device_id: {}", server_state.device_id);

            let mut server_config: AWConfig = AWConfig::default();
            server_config.port = 5600;

            endpoints::build_rocket(server_state, server_config)
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
            info!("Initializing aw-server-rust...");
            debug!("Redirected aw-server-rust stdout/stderr to logcat");
        } else {
            info!("Already initialized");
        }
        INITIALIZED = true;

        // Without this it might not work due to weird error probably arising from Rust optimizing away the JNIEnv:
        //  JNI DETECTED ERROR IN APPLICATION: use of deleted weak global reference
        string_to_jstring(&env, "test".to_string());
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_setDataDir(
        env: JNIEnv,
        _: JClass,
        java_dir: JString,
    ) {
        let path = &jstring_to_string(&env, java_dir);
        debug!("Setting android data dir as {}", path);
        dirs::set_android_data_dir(path);
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_getBuckets(
        env: JNIEnv,
        _: JClass,
    ) -> jstring {
        let buckets = openDatastore().get_buckets().unwrap();
        string_to_jstring(&env, json!(buckets).to_string())
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_createBucket(
        env: JNIEnv,
        _: JClass,
        java_bucket: JString,
    ) -> jstring {
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
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_heartbeat(
        env: JNIEnv,
        _: JClass,
        java_bucket_id: JString,
        java_event: JString,
        java_pulsetime: jdouble,
    ) -> jstring {
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
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_getEvents(
        env: JNIEnv,
        _: JClass,
        java_bucket_id: JString,
        java_limit: jint,
    ) -> jstring {
        let bucket_id = jstring_to_string(&env, java_bucket_id);
        let limit = java_limit as u64;
        match openDatastore().get_events(&bucket_id, None, None, Some(limit)) {
            Ok(events) => string_to_jstring(&env, json!(events).to_string()),
            Err(e) => create_error_object(
                &env,
                format!("Something went wrong when trying to get events: {:?}", e),
            ),
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_query(
        env: JNIEnv,
        _: JClass,
        java_query: JString,
        java_timeperiods: JString,
    ) -> jstring {
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
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_androidQuery(
        env: JNIEnv,
        _: JClass,
        java_timeperiods: JString,
    ) -> jstring {
        let timeperiods_str = jstring_to_string(&env, java_timeperiods);

        let timeperiods: Vec<TimeInterval> = match serde_json::from_str(&timeperiods_str) {
            Ok(json) => json,
            Err(err) => return create_error_object(&env, err.to_string()),
        };

        // Hardcoded bucket ID for testing
        let bid_android = "aw-watcher-android-test".to_string();

        // Get classes from server settings via HTTP API
        let classes = match AwClient::new("127.0.0.1", 5600, "aw-android-query") {
            Ok(client) => {
                match client.get_setting("classes") {
                    Ok(classes_value) => {
                        // Parse the server-side classes from JSON value
                        match serde_json::from_value::<Vec<aw_models::Class>>(classes_value) {
                            Ok(server_classes) => {
                                if server_classes.is_empty() {
                                    info!("Server classes list is empty, using default classes");
                                    default_classes()
                                } else {
                                    // Convert from aw_models::Class to CategorySpec format
                                    server_classes
                                        .iter()
                                        .map(|c| {
                                            let category_id: CategoryId = c.name.clone();
                                            let category_spec = CategorySpec {
                                                spec_type: c.rule.rule_type.clone(),
                                                regex: c.rule.regex.clone().unwrap_or_default(),
                                                ignore_case: c.rule.ignore_case.unwrap_or(false),
                                            };
                                            (category_id, category_spec)
                                        })
                                        .collect()
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse server classes, using defaults: {:?}", e);
                                default_classes()
                            }
                        }
                    }
                    Err(e) => {
                        info!("Failed to get server classes, using defaults: {:?}", e);
                        default_classes()
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to create client for fetching classes, using defaults: {:?}",
                    e
                );
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
    }
}
