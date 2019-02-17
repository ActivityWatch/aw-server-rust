// From: https://github.com/servo/servo/pull/21812/files
// With modifications from latest: https://github.com/servo/servo/blob/5de6d87c97050db35cfb0a575e14b4d9b6207ac5/ports/libsimpleservo/jniapi/src/lib.rs#L480

extern crate libc;
use self::libc::{pipe, dup2, read};
use std::os::raw::{c_char, c_int};
use std::thread;

extern "C" {
    pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

pub fn redirect_stdout_to_logcat() {
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
