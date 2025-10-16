use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use std::ffi::{CString, CStr};
use aw_client_rust::blocking::AwClient;
use serde_json::json;

use crate::{pull, pull_all, push};
use crate::util::get_server_port;

/// Helper function to convert Rust string to Java string
fn rust_string_to_jstring(env: &JNIEnv, s: String) -> jstring {
    let output = env.new_string(s)
        .expect("Couldn't create java string!");
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
    env: JNIEnv,
    _class: JClass,
    port: i32,
) -> jstring {
    let result = (|| {
        let client = get_client(port)?;
        pull_all(&client)
            .map_err(|e| format!("Sync pull failed: {}", e))?;
        Ok(json!({
            "success": true,
            "message": "Successfully pulled from all hosts"
        }).to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPullAll error: {}", e);
            let error_json = json!({
                "success": false,
                "error": e
            }).to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Pull sync data from a specific host
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPull(
    env: JNIEnv,
    _class: JClass,
    port: i32,
    hostname: JString,
) -> jstring {
    let result = (|| {
        let client = get_client(port)?;
        let hostname_str: String = env.get_string(hostname)
            .map_err(|e| format!("Failed to get hostname string: {}", e))?
            .into();
        
        pull(&hostname_str, &client)
            .map_err(|e| format!("Sync pull failed: {}", e))?;
        
        Ok(json!({
            "success": true,
            "message": format!("Successfully pulled from host: {}", hostname_str)
        }).to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPull error: {}", e);
            let error_json = json!({
                "success": false,
                "error": e
            }).to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Push local sync data to the sync directory
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncPush(
    env: JNIEnv,
    _class: JClass,
    port: i32,
) -> jstring {
    let result = (|| {
        let client = get_client(port)?;
        push(&client)
            .map_err(|e| format!("Sync push failed: {}", e))?;
        Ok(json!({
            "success": true,
            "message": "Successfully pushed local data"
        }).to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncPush error: {}", e);
            let error_json = json!({
                "success": false,
                "error": e
            }).to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}

/// Perform full sync (pull from all hosts, then push local data)
#[no_mangle]
pub extern "C" fn Java_net_activitywatch_android_SyncInterface_syncBoth(
    env: JNIEnv,
    _class: JClass,
    port: i32,
) -> jstring {
    let result = (|| {
        let client = get_client(port)?;
        
        pull_all(&client)
            .map_err(|e| format!("Pull phase failed: {}", e))?;
        
        push(&client)
            .map_err(|e| format!("Push phase failed: {}", e))?;
        
        Ok(json!({
            "success": true,
            "message": "Successfully completed full sync"
        }).to_string())
    })();

    match result {
        Ok(msg) => rust_string_to_jstring(&env, msg),
        Err(e) => {
            error!("syncBoth error: {}", e);
            let error_json = json!({
                "success": false,
                "error": e
            }).to_string();
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
    let result = crate::dirs::get_sync_dir();
    
    match result {
        Ok(path) => {
            let path_str = path.to_string_lossy().to_string();
            let response = json!({
                "success": true,
                "path": path_str
            }).to_string();
            rust_string_to_jstring(&env, response)
        }
        Err(e) => {
            let error_json = json!({
                "success": false,
                "error": format!("Failed to get sync dir: {}", e)
            }).to_string();
            rust_string_to_jstring(&env, error_json)
        }
    }
}
