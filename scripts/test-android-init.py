#!/usr/bin/env python3
"""Exercise the production JNI initialization body on a host without Android/JVM.

Only the JNI/logging dependencies are stubbed. Extract the real entry point so a
regression to an unsynchronized flag fails even though host cargo tests exclude
this module. The Android build separately type-checks the real dependencies.
"""

from pathlib import Path
import subprocess
import tempfile


SOURCE = Path(__file__).resolve().parents[1] / "aw-server/src/android/mod.rs"

# Match the dependencies of the initialization entry point, not its synchronization.
STUBS = r"""
use std::sync::{Arc, Barrier, Once};
use std::sync::atomic::{AtomicUsize, Ordering};
static LOGGER_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
static JNI_CALLS: AtomicUsize = AtomicUsize::new(0);
static ALREADY_INITIALIZED: AtomicUsize = AtomicUsize::new(0);
static FAIL_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
macro_rules! info {
    ("Already initialized") => { ALREADY_INITIALIZED.fetch_add(1, Ordering::SeqCst); };
    ($($args:tt)*) => {};
}
macro_rules! debug { ($($args:tt)*) => {}; }
struct JNIEnv;
struct JClass;
struct Config;
impl Config {
    fn default() -> Self { Self }
    fn with_max_level(self, _: log::LevelFilter) -> Self { self }
    fn with_tag(self, _: &str) -> Self { self }
}
mod log { pub enum LevelFilter { Info } }
mod android_logger {
    pub fn init_once(_: super::Config) {
        super::LOGGER_CALLS.fetch_add(1, super::Ordering::SeqCst);
        // Widen the old check-then-set window. Every other caller must wait
        // for the panic hook installation rather than return during this delay.
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
mod log_panics {
    pub fn init() {
        if super::FAIL_ONCE.swap(false, super::Ordering::SeqCst) {
            panic!("injected initialization failure");
        }
        super::HOOK_CALLS.fetch_add(1, super::Ordering::SeqCst);
    }
}
fn jni_guard_void(_: &str, f: impl FnOnce()) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
}
fn string_to_jstring(_: &JNIEnv, _: String) {
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
    JNI_CALLS.fetch_add(1, Ordering::SeqCst);
}
"""

TEST = r"""
fn main() {
    if std::env::args().any(|arg| arg == "retry") {
        FAIL_ONCE.store(true, Ordering::SeqCst);
        unsafe { Java_net_activitywatch_android_RustInterface_initialize(JNIEnv, JClass); }
        assert_eq!(JNI_CALLS.load(Ordering::SeqCst), 0);
        unsafe { Java_net_activitywatch_android_RustInterface_initialize(JNIEnv, JClass); }
        assert_eq!(LOGGER_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(JNI_CALLS.load(Ordering::SeqCst), 1);
        unsafe { Java_net_activitywatch_android_RustInterface_initialize(JNIEnv, JClass); }
        assert_eq!(LOGGER_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(JNI_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(ALREADY_INITIALIZED.load(Ordering::SeqCst), 1);
        println!("JNI initialization: retry after caught panic passed");
        return;
    }
    let start = Arc::new(Barrier::new(16));
    let threads: Vec<_> = (0..16).map(|_| {
        let start = start.clone();
        std::thread::spawn(move || {
            start.wait();
            unsafe { Java_net_activitywatch_android_RustInterface_initialize(JNIEnv, JClass); }
        })
    }).collect();
    for t in threads { t.join().unwrap(); }
    assert_eq!(LOGGER_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(JNI_CALLS.load(Ordering::SeqCst), 16);
    assert_eq!(ALREADY_INITIALIZED.load(Ordering::SeqCst), 15);
    // Repeated initialization still runs the JNI workaround, not the setup.
    unsafe { Java_net_activitywatch_android_RustInterface_initialize(JNIEnv, JClass); }
    assert_eq!(LOGGER_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(JNI_CALLS.load(Ordering::SeqCst), 17);
    assert_eq!(ALREADY_INITIALIZED.load(Ordering::SeqCst), 16);
    println!("JNI initialization: concurrent and repeated callers passed");
}
"""


def main():
    source = SOURCE.read_text()
    # Include the state declaration as well as the entry point, so the test
    # exercises its actual lifetime (one static for the process).
    before, after = source.split(
        '    pub unsafe extern "C" fn Java_net_activitywatch_android_RustInterface_initialize(',
        1,
    )
    declaration = before.rsplit("\n\n", 2)[-2]
    body = after.split("\n    #[no_mangle]", 1)[0]
    entry = (
        "    unsafe fn Java_net_activitywatch_android_RustInterface_initialize(" + body
    )
    with tempfile.TemporaryDirectory(prefix="aw-android-init-") as tmp:
        harness = Path(tmp) / "init.rs"
        binary = Path(tmp) / "init"
        harness.write_text(STUBS + declaration + "\n" + entry + TEST)
        subprocess.run(
            ["rustc", "--edition=2021", "-Awarnings", str(harness), "-o", str(binary)],
            check=True,
        )
        subprocess.run([str(binary)], check=True, timeout=30)
        subprocess.run([str(binary), "retry"], check=True, timeout=30)

    # The C ABI remains callable with a live C string, including non-UTF-8
    # input. Exercise allocation ownership without needing a Java VM.
    greeting = source.split("#[no_mangle]", 1)[1].split(
        '#[cfg(target_os = "android")]', 1
    )[0]
    with tempfile.TemporaryDirectory(prefix="aw-android-greeting-") as tmp:
        harness = Path(tmp) / "greeting.rs"
        binary = Path(tmp) / "greeting"
        harness.write_text(
            "use std::ffi::{CStr, CString};\nuse std::os::raw::c_char;\n"
            + greeting
            + r"""
fn main() {
    for (input, expected) in [
        (b"world".as_slice(), "Hello world (from Rust!)"),
        (b"".as_slice(), "Hello  (from Rust!)"),
        (b"\xff".as_slice(), "Hello there (from Rust!)"),
    ] {
        let input = CString::new(input).unwrap();
        let result = unsafe { CString::from_raw(rust_greeting(input.as_ptr())) };
        assert_eq!(result.to_str().unwrap(), expected);
    }
    println!("JNI greeting: valid, empty and non-UTF-8 inputs passed");
}
"""
        )
        subprocess.run(
            ["rustc", "--edition=2021", str(harness), "-o", str(binary)], check=True
        )
        subprocess.run([str(binary)], check=True, timeout=30)


if __name__ == "__main__":
    main()
