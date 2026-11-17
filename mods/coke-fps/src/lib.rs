//! Sample `coke/` mod.
//!
//! Draws a red FPS counter in the top-left corner through the host's
//! `draw_text` service and toggles it with `F10` through the host-owned hotkey
//! bus (API v4). The same key is the core FPS counter's default; either counter
//! can be disabled independently in `coke_config.ini` (`show_fps`,
//! `fps_hotkey=off`). The proxy only loads this DLL when
//! `coke/user_consent_coke_fps.txt` exists.

#![allow(non_snake_case)]
// Exported C entry points have fixed signatures; the host contract lives in
// the shared `coke-host` crate. These lints only restate that.
#![allow(clippy::missing_safety_doc, clippy::type_complexity)]

use coke_host::CokeHost;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::ffi::CString;

const VK_F10: i32 = 0x79;

static HOST: AtomicUsize = AtomicUsize::new(0);
static HUD_MODE: AtomicU32 = AtomicU32::new(0);
static VISIBLE: AtomicU32 = AtomicU32::new(1);

unsafe fn log(msg: &str) {
    unsafe {
        let ptr = HOST.load(Ordering::Relaxed);
        if ptr == 0 {
            return;
        }
        let host = &*(ptr as *const CokeHost);
        if let Some(log) = host.log
            && let Ok(text) = CString::new(msg)
        {
            log(text.as_ptr());
        }
    }
}

unsafe extern "system" fn toggle() {
    let visible = VISIBLE.fetch_xor(1, Ordering::Relaxed) ^ 1;
    unsafe {
        log(&format!(
            "coke_fps: {} (F10)",
            if visible != 0 { "on" } else { "off" }
        ));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_init(host: *const CokeHost) -> i32 {
    unsafe {
        if host.is_null() {
            return 0;
        }
        let h = &*host;
        if h.api_version < 1 {
            return 0;
        }
        HOST.store(host as usize, Ordering::Relaxed);
        // API v3+ calls `coke_hud` after every mod's world-space pass. Defer the
        // text there so a world overlay (cokelos) cannot cover it.
        HUD_MODE.store(u32::from(h.api_version >= 3), Ordering::Relaxed);

        // API v4 adds the host-owned hotkey bus. Older proxies get the counter
        // without a toggle so the sample stays compatible in both directions.
        let mut hotkey = "unavailable (host < v4)";
        if h.api_version >= 4 && (h.size as usize) >= core::mem::size_of::<CokeHost>() {
            hotkey = "F10";
            if let Some(register) = h.register_hotkey {
                if register(VK_F10, Some(toggle)) == 0 {
                    hotkey = "rejected";
                }
            } else {
                hotkey = "unavailable (no register_hotkey)";
            }
        }
        log(&format!(
            "coke_fps: loaded (red FPS, top-left, hotkey={hotkey})"
        ));
        1
    }
}

unsafe fn draw_fps(fps: f32) {
    unsafe {
        if VISIBLE.load(Ordering::Relaxed) == 0 {
            return;
        }
        let host_ptr = HOST.load(Ordering::Relaxed);
        if host_ptr == 0 {
            return;
        }
        let host = &*(host_ptr as *const CokeHost);
        let Some(draw_text) = host.draw_text else {
            return;
        };
        let text = format!("FPS {fps:.1}");
        if let Ok(text) = CString::new(text) {
            // 0x00RRGGBB, opaque red.
            draw_text(8, 8, text.as_ptr(), 0x00FF_0000);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_frame(_width: i32, _height: i32, fps: f32) {
    unsafe {
        if HUD_MODE.load(Ordering::Relaxed) == 0 {
            draw_fps(fps);
        }
    }
}

/// API v3 HUD pass: runs after all world overlays, so the text stays on top.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_hud(_width: i32, _height: i32, fps: f32) {
    unsafe {
        if HUD_MODE.load(Ordering::Relaxed) != 0 {
            draw_fps(fps);
        }
    }
}
