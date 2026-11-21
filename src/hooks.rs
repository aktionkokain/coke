//! user32 function hooks (screen size, cursor, display mode, minimise suppression).

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::c_void;

// ---- code ----
pub unsafe extern "system" fn hook_close_window(hwnd: Hwnd) -> i32 {
    unsafe {
        if config().fix_alt_tab && is_game_window(hwnd) {
            if CLOSE_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log("WM_ACTIVATEAPP(0): CloseWindow suppressed, last frame kept");
            }
            return 1;
        }
        let real = REAL_CLOSE_WINDOW.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        let f: unsafe extern "system" fn(Hwnd) -> i32 = core::mem::transmute(real);
        f(hwnd)
    }
}

pub fn is_minimise_cmd(cmd: i32) -> bool {
    matches!(
        cmd,
        SW_MINIMIZE | SW_SHOWMINIMIZED | SW_SHOWMINNOACTIVE | SW_FORCEMINIMIZE
    )
}

/// The engine minimises its GL window on focus loss, either directly through
/// ShowWindow or through CloseWindow. A minimised OpenGL front buffer is
/// undefined and shows up as the alt-tab black flash, so suppress that one
/// transition. A minimise the user asks for is still allowed.
pub unsafe extern "system" fn hook_show_window(hwnd: Hwnd, cmd: i32) -> i32 {
    unsafe {
        let real = REAL_SHOW_WINDOW.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        if config().fix_alt_tab
            && ALLOW_MINIMIZE.load(Ordering::Relaxed) == 0
            && is_game_window(hwnd)
            && is_minimise_cmd(cmd)
        {
            if CLOSE_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log("focus-loss minimise suppressed (ShowWindow), last frame kept");
            }
            return 1;
        }
        let f: unsafe extern "system" fn(Hwnd, i32) -> i32 = core::mem::transmute(real);
        f(hwnd, cmd)
    }
}

pub unsafe extern "system" fn hook_show_window_async(hwnd: Hwnd, cmd: i32) -> i32 {
    unsafe {
        let real = REAL_SHOW_WINDOW_ASYNC.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        if config().fix_alt_tab
            && ALLOW_MINIMIZE.load(Ordering::Relaxed) == 0
            && is_game_window(hwnd)
            && is_minimise_cmd(cmd)
        {
            return 1;
        }
        let f: unsafe extern "system" fn(Hwnd, i32) -> i32 = core::mem::transmute(real);
        f(hwnd, cmd)
    }
}

pub unsafe extern "system" fn hook_metrics(index: i32) -> i32 {
    unsafe {
        let (w, h) = logical_size();
        if index == SM_CXSCREEN || index == SM_CYSCREEN {
            static FIRST: AtomicU32 = AtomicU32::new(0);
            if FIRST.swap(1, Ordering::Relaxed) == 0 {
                log(&format!("first GetSystemMetrics({index}) -> {w}x{h}"));
            }
        }
        match index {
            SM_CXSCREEN => return w,
            SM_CYSCREEN => return h,
            _ => {}
        }
        let real = REAL_METRICS.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        let f: unsafe extern "system" fn(i32) -> i32 = core::mem::transmute(real);
        f(index)
    }
}

pub unsafe extern "system" fn hook_change_display(devmode: *mut c_void, flags: u32) -> i32 {
    // The mod owns the window mode; exclusive fullscreen would defeat the
    // borderless path and leave the engine's logical size wrong.
    log(&format!(
        "ChangeDisplaySettingsA(flags=0x{flags:x}) suppressed"
    ));
    let _ = devmode;
    0
}

/// The engine's per-frame input poll uses `GetCursorPos` and treats the result
/// as if it were a client coordinate in its logical resolution. Undo both the
/// window offset and the logical -> physical scale so hover, clicks and the
/// drawn cursor stay aligned.
pub unsafe extern "system" fn hook_get_cursor_pos(pt: *mut Point) -> i32 {
    unsafe {
        let real = REAL_GET_CURSOR_POS.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        let f: unsafe extern "system" fn(*mut Point) -> i32 = core::mem::transmute(real);
        let ok = f(pt);
        if ok == 0 || pt.is_null() {
            return ok;
        }
        let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return ok;
        }
        let mut origin = Point { x: 0, y: 0 };
        if ClientToScreen(hwnd, &mut origin) == 0 {
            return ok;
        }
        let (cx, cy) = ((*pt).x - origin.x, (*pt).y - origin.y);
        let (gx, gy) = guard_drag_cursor(hwnd, cx, cy);
        let (mut lx, mut ly) = client_to_logical(gx, gy);
        if camera_input_capture() && camera_rmb_held() {
            if CAMERA_CURSOR_ANCHOR_VALID.load(Ordering::Acquire) == 0 {
                CAMERA_CURSOR_ANCHOR.store(pack_point(lx, ly), Ordering::Release);
                CAMERA_CURSOR_ANCHOR_VALID.store(1, Ordering::Release);
            } else {
                let (ax, ay) = unpack_point(CAMERA_CURSOR_ANCHOR.load(Ordering::Acquire));
                lx = ax;
                ly = ay;
            }
        }
        static GCP_LOG: AtomicU32 = AtomicU32::new(0);
        if GCP_LOG.fetch_add(1, Ordering::Relaxed) < 5 {
            let (sx, sy) = scale_xy();
            log(&format!(
                "GetCursorPos raw=({},{}) origin=({},{}) -> logical=({lx},{ly}) scale={sx:.4}x{sy:.4}",
                (*pt).x,
                (*pt).y,
                origin.x,
                origin.y
            ));
        }
        (*pt).x = lx;
        (*pt).y = ly;
        ok
    }
}

/// Replace the live client position with a frozen one while the window is being
/// moved or resized.
///
/// The engine polls `GetCursorPos` every frame and pans the camera when the
/// cursor reaches the client edge. Dragging the frame (or a resize border) puts
/// the real cursor on that edge for the whole gesture, so the camera runs away
/// while the window moves. The last sample taken inside the client area is kept
/// and reported instead, clamped away from the edges by [`pan_guard_point`].
unsafe fn guard_drag_cursor(hwnd: Hwnd, cx: i32, cy: i32) -> (i32, i32) {
    unsafe {
        if !config().freeze_pan_while_dragging {
            return (cx, cy);
        }
        if WINDOW_DRAGGING.load(Ordering::Relaxed) == 0 {
            // Remember only positions the engine would see as inside the client.
            let mut c = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetClientRect(hwnd, &mut c) != 0
                && cx >= 0
                && cy >= 0
                && cx < c.right
                && cy < c.bottom
            {
                DRAG_ANCHOR.store(pack_point(cx, cy), Ordering::Relaxed);
                DRAG_ANCHOR_VALID.store(1, Ordering::Relaxed);
            }
            return (cx, cy);
        }
        if DRAG_ANCHOR_VALID.load(Ordering::Relaxed) == 0 {
            return (cx, cy);
        }
        let (ax, ay) = unpack_point(DRAG_ANCHOR.load(Ordering::Relaxed));
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut c) == 0 {
            return (ax, ay);
        }
        pan_guard_point(ax, ay, c.right - c.left, c.bottom - c.top)
    }
}
