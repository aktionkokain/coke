//! Logging plus the string / symbol helpers used by the rest of the crate.

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::{c_char, c_void};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

// ---- code ----
// ------------------------------------------------------------------ logging

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub static LOG_T0: AtomicU32 = AtomicU32::new(0);

pub fn log(msg: &str) {
    let path = exe_dir().join(LOG_FILE);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        // Milliseconds since this process loaded the proxy. The raw tick count
        // wraps after 49 days and is identical across launches, which made
        // entries from different runs impossible to tell apart.
        let elapsed = unsafe { GetTickCount() }.wrapping_sub(LOG_T0.load(Ordering::Relaxed));
        let _ = writeln!(f, "[{elapsed:>7} ms] {msg}");
    }
}

// --------------------------------------------------------------------- helpers

pub unsafe fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

pub unsafe fn str_from_wide(p: *const u16) -> String {
    unsafe {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0usize;
        while n < 32768 && *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    }
}

pub fn cstr_is(p: *const c_char, name: &str) -> bool {
    if p.is_null() {
        return false;
    }
    for (i, b) in name.as_bytes().iter().enumerate() {
        if unsafe { *p.add(i) } as u8 != *b {
            return false;
        }
    }
    unsafe { *p.add(name.len()) == 0 }
}

pub unsafe fn resolve(module: Hmodule, name: &str) -> *mut c_void {
    unsafe {
        let mut buf = [0u8; 128];
        let bytes = name.as_bytes();
        if bytes.len() + 1 > buf.len() {
            return core::ptr::null_mut();
        }
        buf[..bytes.len()].copy_from_slice(bytes);
        GetProcAddress(module, buf.as_ptr() as *const c_char)
    }
}
