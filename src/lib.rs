#![feature(plugin,try_from)]
#![feature(proc_macro_hygiene)]
#![feature(custom_attribute)]
#![feature(decl_macro)]
#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_contrib;
extern crate rocket_cors;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

extern crate plex;

extern crate appdirs;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;
pub mod dirs;


// Everything below from;
//   https://mozilla.github.io/firefox-browser-architecture/experiments/2017-09-21-rust-on-android.html
//
// Should probably move it into module

use std::os::raw::{c_char};
use std::ffi::{CString, CStr};

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
    use models::{Bucket};

    fn openDatastore() -> Datastore {
        let db_dir = dirs::db_path().to_str().unwrap().to_string();
        Datastore::new(db_dir)
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustGreetings_greeting(env: JNIEnv, _: JClass, java_pattern: JString) -> jstring {
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

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustGreetings_setAndroidDataDir(env: JNIEnv, _: JClass, java_dir: JString) -> jstring {
        let dir = jstring_to_string(&env, java_dir);
        //let c_str = CStr::from_ptr(env.get_string(java_dir).expect("invalid pattern string").as_ptr());
        //let dir = c_str.to_str().unwrap();
        dirs::set_android_data_dir(&dir);

        let current_dir: String = match dirs::get_data_dir() {
            Ok(path) => String::from(path.to_str().unwrap()),
            Err(_) => String::from("invalid path")
        };
        string_to_jstring(&env, current_dir.as_str().to_string())
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustGreetings_getBucketsJSONString(env: JNIEnv, _: JClass) -> jstring {
        let buckets = openDatastore().get_buckets().unwrap();
        string_to_jstring(&env, json!(buckets).to_string())
    }

    #[no_mangle]
    pub unsafe extern fn Java_net_activitywatch_android_RustGreetings_createBucket(env: JNIEnv, _: JClass, java_bucket: JString) -> jstring {
        let bucket = jstring_to_string(&env, java_bucket);
        let bucket_json: Bucket = match serde_json::from_str(&bucket) {
            Ok(json) => json,
            Err(err) => return string_to_jstring(&env, err.to_string())
        };
        match openDatastore().create_bucket(&bucket_json) {
            Ok(()) => string_to_jstring(&env, "Bucket successfully created".to_string()),
            Err(_) => string_to_jstring(&env, "Something went wrong when trying to create bucket".to_string())
        }
    }
}
