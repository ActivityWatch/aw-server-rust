#![feature(plugin,try_from)]
#![feature(proc_macro_hygiene, decl_macro)]

#[cfg(not(target_os = "linux"))]
#[macro_use] extern crate rocket;

#[cfg(not(target_os = "linux"))]
#[macro_use] extern crate rocket_contrib;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate plex;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;

#[cfg(not(target_os = "linux"))]
pub mod endpoints;


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

    CString::new("Hello ".to_owned() + recipient).unwrap().into_raw()
}

#[cfg(target_os="android")]
#[allow(non_snake_case)]
pub mod android {
    extern crate jni;

    use super::*;
    use self::jni::JNIEnv;
    use self::jni::objects::{JClass, JString};
    use self::jni::sys::{jstring};

    #[no_mangle]
    pub unsafe extern fn Java_com_mozilla_greetings_RustGreetings_greeting(env: JNIEnv, _: JClass, java_pattern: JString) -> jstring {
        // Our Java companion code might pass-in "world" as a string, hence the name.
        let world = rust_greeting(env.get_string(java_pattern).expect("invalid pattern string").as_ptr());
        // Retake pointer so that we can use it below and allow memory to be freed when it goes out of scope.
        let world_ptr = CString::from_raw(world);
        let output = env.new_string(world_ptr.to_str().unwrap()).expect("Couldn't create java string!");

        output.into_inner()
    }
}
