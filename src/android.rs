// Based On the following guide from Mozilla:
//   https://mozilla.github.io/firefox-browser-architecture/experiments/2017-09-21-rust-on-android.html

extern crate libc;
use self::libc::{pipe, dup2, read};

use std::os::raw::{c_char};
use std::ffi::{CString, CStr};
use dirs;

#[no_mangle]
pub extern fn rust_greeting(to: *const c_char) -> *mut c_char {
    let c_str = unsafe { CStr::from_ptr(to) };
    let recipient = match c_str.to_str() {
        Err(_) => "there",
        Ok(string) => string,
    };

    CString::new("Hello ".to_owned() + recipient + " (from Rust!)").unwrap().into_raw()
}

#[cfg(target_os="android")]
#[allow(non_snake_case)]
pub mod android {
    extern crate jni;

    use super::*;
    use self::jni::JNIEnv;
    use self::jni::objects::{JClass, JString};
    use self::jni::sys::{jstring};
    use datastore::Datastore;
    use models::{Event, Bucket};

    fn openDatastore() -> Datastore {
        let db_dir = dirs::db_path().to_str().unwrap().to_string();
        Datastore::new(db_dir)
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_greeting(env: JNIEnv, _: JClass, java_pattern: JString) -> jstring {
        // Our Java companion code might pass-in "world" as a string, hence the name.
        let world = rust_greeting(env.get_string(java_pattern).expect("invalid pattern string").as_ptr());
        // Retake pointer so that we can use it below and allow memory to be freed when it goes out of scope.
        let world_ptr = CString::from_raw(world);
        let output = env.new_string(world_ptr.to_str().unwrap()).expect("Couldn't create java string!");

        output.into_inner()
    }

    unsafe fn jstring_to_string(env: &JNIEnv, string: JString) -> String {
        let c_str = CStr::from_ptr(env.get_string(string).expect("invalid string").as_ptr());
        String::from(c_str.to_str().unwrap())
    }

    unsafe fn string_to_jstring(env: &JNIEnv, string: String) -> jstring {
        env.new_string(string).expect("Couldn't create java string").into_inner()
    }

    unsafe fn create_error_object(env: &JNIEnv, msg: String) -> jstring {
        let mut obj = json!({});
        obj["error"] = json!(msg).0;
        string_to_jstring(&env, obj.to_string())
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_startServer(env: JNIEnv, _: JClass, java_asset_path: JString) {
        use std::path::{PathBuf};
        use endpoints;
        use rocket::config::{Config, Environment};

        println!("Building server state...");

        let asset_path = jstring_to_string(&env, java_asset_path);
        println!("Using asset dir: {}", asset_path);

        let server_state = endpoints::ServerState {
            datastore: openDatastore(),
            asset_path: PathBuf::from(asset_path),
        };

        let config = Config::build(Environment::Production)
            .address("127.0.0.1")
            .port(5600)
            .finalize().unwrap();

        println!("Starting server...");
        endpoints::rocket(server_state, Some(config)).launch();
        println!("Server exited");
    }

    static mut INITIALIZED: bool = false;

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_initialize(env: JNIEnv, _: JClass) {
        if !INITIALIZED {
            redirect_stdout_to_logcat();
            println!("Initializing aw-server-rust...");
            println!("Redirecting aw-server-rust stdout/stderr to logcat");
        } else {
            println!("Already initialized");
        }
        INITIALIZED = true;

        // Without this it might not work due to weird error probably arising from Rust optimizing away the JNIEnv:
        //  JNI DETECTED ERROR IN APPLICATION: use of deleted weak global reference
        string_to_jstring(&env, "test".to_string());
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_setDataDir(env: JNIEnv, _: JClass, java_dir: JString) {
        println!("Setting android data dir");
        dirs::set_android_data_dir(&jstring_to_string(&env, java_dir));
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_getBuckets(env: JNIEnv, _: JClass) -> jstring {
        let buckets = openDatastore().get_buckets().unwrap();
        string_to_jstring(&env, json!(buckets).to_string())
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_createBucket(env: JNIEnv, _: JClass, java_bucket: JString) -> jstring {
        let bucket = jstring_to_string(&env, java_bucket);
        let bucket_json: Bucket = match serde_json::from_str(&bucket) {
            Ok(json) => json,
            Err(err) => return create_error_object(&env, err.to_string())
        };
        match openDatastore().create_bucket(&bucket_json) {
            Ok(()) => string_to_jstring(&env, "Bucket successfully created".to_string()),
            Err(_) => create_error_object(&env, "Something went wrong when trying to create bucket".to_string())
        }
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_heartbeat(env: JNIEnv, _: JClass, java_bucket_id: JString, java_event: JString) -> jstring {
        let bucket_id = jstring_to_string(&env, java_bucket_id);
        let event = jstring_to_string(&env, java_event);
        let event_json: Event = match serde_json::from_str(&event) {
            Ok(json) => json,
            Err(err) => return create_error_object(&env, err.to_string())
        };
        match openDatastore().heartbeat(&bucket_id, event_json, 60.0) {
            Ok(()) => string_to_jstring(&env, "Heartbeat successfully received".to_string()),
            Err(_) => create_error_object(&env, "Something went wrong when trying to send heartbeat".to_string())
        }
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustInterface_getEvents(env: JNIEnv, _: JClass, java_bucket_id: JString) -> jstring {
        let bucket_id = jstring_to_string(&env, java_bucket_id);
        match openDatastore().get_events(&bucket_id, None, None, None) {
            Ok(events) => string_to_jstring(&env, json!(events).to_string()),
            Err(_) => create_error_object(&env, "Something went wrong when trying to send heartbeat".to_string())
        }
    }

    // From: https://github.com/servo/servo/pull/21812/files
    // With modifications from latest: https://github.com/servo/servo/blob/5de6d87c97050db35cfb0a575e14b4d9b6207ac5/ports/libsimpleservo/jniapi/src/lib.rs#L480
    use std::os::raw::{c_char, c_int};
    use std::thread;

    extern "C" {
        pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    fn redirect_stdout_to_logcat() {
        // The first step is to redirect stdout and stderr to the logs.
        // We redirect stdout and stderr to a custom descriptor.
        let mut pfd: [c_int; 2] = [0, 0];
        unsafe {
            pipe(pfd.as_mut_ptr());
            dup2(pfd[1], 1);
            dup2(pfd[1], 2);
        }

        let descriptor = pfd[0];

        // Then we spawn a thread whose only job is to read from the other side of the
        // pipe and redirect to the logs.
        let _detached = thread::spawn(move || {
            const BUF_LENGTH: usize = 512;
            let mut buf = vec![b'\0' as c_char; BUF_LENGTH];

            // Always keep at least one null terminator
            const BUF_AVAILABLE: usize = BUF_LENGTH - 1;
            let buf = &mut buf[..BUF_AVAILABLE];

            let mut cursor = 0_usize;

            let tag = b"aw-server-rust\0".as_ptr() as _;

            loop {
                let result = {
                    let read_into = &mut buf[cursor..];
                    unsafe {
                        read(
                            descriptor,
                            read_into.as_mut_ptr() as *mut _,
                            read_into.len(),
                            )
                    }
                };

                let end = if result == 0 {
                    return;
                } else if result < 0 {
                    unsafe {
                        __android_log_write(
                            3,
                            tag,
                            b"error in log thread; closing\0".as_ptr() as *const _,
                            );
                    }
                    return;
                } else {
                    result as usize + cursor
                };

                // Only modify the portion of the buffer that contains real data.
                let buf = &mut buf[0..end];

                if let Some(last_newline_pos) = buf.iter().rposition(|&c| c == b'\n' as c_char) {
                    buf[last_newline_pos] = b'\0' as c_char;
                    unsafe {
                        __android_log_write(3, tag, buf.as_ptr());
                    }
                    if last_newline_pos < buf.len() - 1 {
                        let pos_after_newline = last_newline_pos + 1;
                        let len_not_logged_yet = buf[pos_after_newline..].len();
                        for j in 0..len_not_logged_yet as usize {
                            buf[j] = buf[pos_after_newline + j];
                        }
                        cursor = len_not_logged_yet;
                    } else {
                        cursor = 0;
                    }
                } else if end == BUF_AVAILABLE {
                    // No newline found but the buffer is full, flush it anyway.
                    // `buf.as_ptr()` is null-terminated by BUF_LENGTH being 1 less than BUF_AVAILABLE.
                    unsafe {
                        __android_log_write(3, tag, buf.as_ptr());
                    }
                    cursor = 0;
                } else {
                    cursor = end;
                }
            }
        });
    }
}
