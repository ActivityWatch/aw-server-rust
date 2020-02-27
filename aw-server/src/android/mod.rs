// Based On the following guide from Mozilla:
//   https://mozilla.github.io/firefox-browser-architecture/experiments/2017-09-21-rust-on-android.html

extern crate android_logger;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

use crate::dirs;

use android_logger::Config;
use log::Level;

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
    use self::jni::sys::{jdouble, jstring};
    use self::jni::JNIEnv;
    use super::*;
    use aw_datastore::Datastore;
    use aw_models::{Bucket, Event};

    static mut DATASTORE: Option<Datastore> = None;

    unsafe fn openDatastore() -> Datastore {
        match DATASTORE {
            Some(ref ds) => ds.clone(),
            None => {
                let db_dir = dirs::db_path().to_str().unwrap().to_string();
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

        output.into_inner()
    }

    unsafe fn jstring_to_string(env: &JNIEnv, string: JString) -> String {
        let c_str = CStr::from_ptr(env.get_string(string).expect("invalid string").as_ptr());
        String::from(c_str.to_str().unwrap())
    }

    unsafe fn string_to_jstring(env: &JNIEnv, string: String) -> jstring {
        env.new_string(string)
            .expect("Couldn't create java string")
            .into_inner()
    }

    unsafe fn create_error_object(env: &JNIEnv, msg: String) -> jstring {
        let mut obj = json!({});
        obj["error"] = json!(msg).0;
        string_to_jstring(&env, obj.to_string())
    }

    #[no_mangle]
    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_startServer(
        env: JNIEnv,
        _: JClass,
        java_asset_path: JString,
    ) {
        use crate::config::AWConfig;
        use std::path::PathBuf;

        use crate::endpoints;

        info!("Building server state...");

        let asset_path = jstring_to_string(&env, java_asset_path);
        info!("Using asset dir: {}", asset_path);

        let server_state = endpoints::ServerState {
            datastore: Mutex::new(openDatastore()),
            asset_path: PathBuf::from(asset_path),
        };

        let mut config = AWConfig::default();
        config.port = 5600;
        info!("Starting server...");
        endpoints::build_rocket(server_state, &config).launch();
        info!("Server exited");
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
                    .with_min_level(Level::Trace) // limit log level
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
        debug!("Setting android data dir");
        dirs::set_android_data_dir(&jstring_to_string(&env, java_dir));
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
    ) -> jstring {
        let bucket_id = jstring_to_string(&env, java_bucket_id);
        match openDatastore().get_events(&bucket_id, None, None, None) {
            Ok(events) => string_to_jstring(&env, json!(events).to_string()),
            Err(e) => create_error_object(
                &env,
                format!("Something went wrong when trying to get events: {:?}", e),
            ),
        }
    }
}
