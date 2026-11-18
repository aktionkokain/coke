//! Engine image addresses, real-function pointers and all shared runtime state.

use crate::*;
use core::sync::atomic::{AtomicI32, AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering};

// ---- code ----
// ------------------------------------------------------------ engine addresses
//
// There are no engine addresses in this file. Every build-specific address and
// object offset comes from the selected profile (`profiles/<exe>/<build>.ini`)
// and is resolved as `module_base + rva`. A build without a matching profile
// runs in safe mode and performs no engine writes.

// ------------------------------------------------------------ real pointers

pub static REAL_WGL_CREATE_CONTEXT: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_MAKE_CURRENT: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_DELETE_CONTEXT: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_SWAP_BUFFERS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_SHARE_LISTS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_GET_CURRENT_CONTEXT: AtomicUsize = AtomicUsize::new(0);
pub static REAL_WGL_GET_CURRENT_DC: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_GET_STRING: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_VIEWPORT: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_SCISSOR: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_RASTER_POS2I: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_RASTER_POS3F: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_READ_PIXELS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_COPY_TEX_SUB_IMAGE2D: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_COPY_TEX_IMAGE2D: AtomicUsize = AtomicUsize::new(0);
pub static REAL_BIND_FRAMEBUFFER: AtomicUsize = AtomicUsize::new(0);
pub static REAL_SWAP_INTERVAL: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DRAW_PIXELS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DISABLE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_ENABLE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_BLEND_FUNC: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_MATRIX_MODE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_LOAD_IDENTITY: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_PUSH_MATRIX: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_POP_MATRIX: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_ORTHO: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_VERTEX_POINTER: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_COLOR_POINTER: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DRAW_ARRAYS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_ENABLE_CLIENT_STATE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DISABLE_CLIENT_STATE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DEPTH_MASK: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_BIND_BUFFER: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_PUSH_ATTRIB: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_POP_ATTRIB: AtomicUsize = AtomicUsize::new(0);
// Profiled calls (see src/perf.rs). Intercepted so the proxy can count them.
pub static REAL_GL_CALL_LIST: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_BIND_TEXTURE: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_BEGIN: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_END: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DRAW_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_TEX_IMAGE2D: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_TEX_SUB_IMAGE2D: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_NEW_LIST: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_END_LIST: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_GEN_LISTS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_DELETE_LISTS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GL_CALL_LISTS: AtomicUsize = AtomicUsize::new(0);

/// Tick count of the last non-zero framebuffer bind, 0 = none yet. The engine
/// binds an offscreen target every frame it renders the 3D scene (shadow map),
/// and never in the main menu or the briefing/purchase screens, so this is the
/// signal that separates "in a battle" from "engine UI". See `in_game_scene`.
pub static FBO_LAST_TICK: AtomicU32 = AtomicU32::new(0);

pub static REAL_CLOSE_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static REAL_METRICS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_CHANGE_DISPLAY: AtomicUsize = AtomicUsize::new(0);
pub static REAL_GET_CURSOR_POS: AtomicUsize = AtomicUsize::new(0);
pub static REAL_SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static REAL_SHOW_WINDOW_ASYNC: AtomicUsize = AtomicUsize::new(0);

pub static REAL_MODULE: AtomicUsize = AtomicUsize::new(0);
pub static MAIN_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static ORIGINAL_WNDPROC: AtomicIsize = AtomicIsize::new(0);
/// A build profile matched the running image: engine writes are allowed.
/// Read-only features (RTTI, window discovery) work without it.
pub static PROFILE_OK: AtomicU32 = AtomicU32::new(0);
/// Per-slot result of the slot-content check, one bit per configured slot.
/// A slot is only rewritten when its current value still points at the real
/// user32 function.
pub static SLOT_OK_MASK: AtomicU32 = AtomicU32::new(0);

pub const SLOT_BIT_CLOSE_WINDOW: u32 = 1 << 0;
pub const SLOT_BIT_GET_SYSTEM_METRICS: u32 = 1 << 1;
pub const SLOT_BIT_CHANGE_DISPLAY: u32 = 1 << 2;
pub const SLOT_BIT_GET_CURSOR_POS: u32 = 1 << 3;

/// Was the image identified as a known build? Gate for engine *writes*.
pub fn profile_ok() -> bool {
    PROFILE_OK.load(Ordering::Relaxed) != 0
}
/// Set while the user is deliberately minimising the window; the ShowWindow
/// hook lets that one through and keeps suppressing the engine's minimises.
pub static ALLOW_MINIMIZE: AtomicU32 = AtomicU32::new(0);
pub static SCAN_TICKS: AtomicU32 = AtomicU32::new(0);
pub static LAST_SCAN_MS: AtomicU32 = AtomicU32::new(0);
pub static SCAN_SHOW: AtomicU32 = AtomicU32::new(0);
pub static SCAN_SHOW_ASYNC: AtomicU32 = AtomicU32::new(0);
pub static MAXIMIZE_LOGGED: AtomicU32 = AtomicU32::new(0);
pub static IDENTITY_LOGGED: AtomicU32 = AtomicU32::new(0);
pub static CLOSE_LOGGED: AtomicU32 = AtomicU32::new(0);
/// Window mode last applied by the mod:
/// 0 = framed window, 1 = borderless fullscreen, 2 = maximised framed window.
pub static WINDOW_MODE: AtomicU32 = AtomicU32::new(0);
/// Set between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`, i.e. while the user is
/// moving or resizing the window with the mouse.
pub static WINDOW_DRAGGING: AtomicU32 = AtomicU32::new(0);
/// Last cursor sample that was inside the client area, in physical client
/// coordinates (see `pack_point`). While a window drag is in progress the
/// cursor hook reports this point instead of the live one, so the engine does
/// not pan the camera at the window edge.
pub static DRAG_ANCHOR: AtomicIsize = AtomicIsize::new(0);
pub static DRAG_ANCHOR_VALID: AtomicU32 = AtomicU32::new(0);
/// Set once the initial mode has been applied to the current engine window.
pub static INITIAL_MODE_DONE: AtomicU32 = AtomicU32::new(0);
/// Last framed-window rectangle, restored when leaving borderless fullscreen.
pub static FRAMED_LEFT: AtomicI32 = AtomicI32::new(0);
pub static FRAMED_TOP: AtomicI32 = AtomicI32::new(0);
pub static FRAMED_W: AtomicI32 = AtomicI32::new(0);
pub static FRAMED_H: AtomicI32 = AtomicI32::new(0);
pub static FRAMED_MAXIMIZED: AtomicU32 = AtomicU32::new(0);

/// Distinct viewport rectangles already logged. The engine sets a handful of
/// UI sub-viewports every frame, so remembering the set keeps the log readable.
pub const MAX_VIEWPORTS: usize = 32;
pub static VIEWPORTS: [AtomicU64; MAX_VIEWPORTS] =
    [const { AtomicU64::new(u64::MAX) }; MAX_VIEWPORTS];
pub static VIEWPORT_COUNT: AtomicUsize = AtomicUsize::new(0);

pub static FPS_START: AtomicU64 = AtomicU64::new(0);
pub static FPS_FRAMES: AtomicU32 = AtomicU32::new(0);
pub static VSYNC_LOGGED: AtomicU32 = AtomicU32::new(0);
/// Last measured FPS × 10 (so 614 = 61.4 fps). Avoids floats in atomics.
pub static FPS_CACHED: AtomicU32 = AtomicU32::new(0);

pub static SWAP_FRAME_COUNT: AtomicU32 = AtomicU32::new(0);

/// Last logical scissor rect set by the engine (packed as two u64).
/// Re-applied every swap so a stale scissor from before a resize doesn't clip everything.
pub static LAST_SCISSOR_XY: AtomicU64 = AtomicU64::new(u64::MAX);
pub static LAST_SCISSOR_WH: AtomicU64 = AtomicU64::new(u64::MAX);

/// Logical (engine-visible) resolution. `GetSystemMetrics` reports this and the
/// engine uses it for its window, viewports and cursor mapping.
pub static LOGICAL_W: AtomicI32 = AtomicI32::new(0);
pub static LOGICAL_H: AtomicI32 = AtomicI32::new(0);

/// Logical size the engine was told at process start. Used to recognise a
/// stale full-screen viewport that still uses the startup canvas after a
/// resize (e.g. `(0,0,1920,1080)` when the current canvas is 2560x1439).
pub static STARTUP_LOGICAL_W: AtomicI32 = AtomicI32::new(0);
pub static STARTUP_LOGICAL_H: AtomicI32 = AtomicI32::new(0);

/// The canvas the engine rendered with just before the most recent change.
/// Like `STARTUP_LOGICAL_*` this is a layout key: screens built before a resize
/// still carry geometry computed for it, so both sizes are needed to recognise
/// stale rects. 0 until the first canvas patch.
pub static PREV_LOGICAL_W: AtomicI32 = AtomicI32::new(0);
pub static PREV_LOGICAL_H: AtomicI32 = AtomicI32::new(0);

/// Every engine canvas seen so far (initial value + each previous value at
/// patch time), packed `(w as u64) << 32 | h`. Screens can have been built for
/// any of these; the widget re-center tests candidates against all of them.
pub static CANVAS_HIST: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub static CANVAS_HIST_N: AtomicU32 = AtomicU32::new(0);

/// Append `v` to a fixed-size history, newest last. Duplicates are ignored;
/// once the array is full the oldest entry is dropped.
fn record_history(hist: &[AtomicU64], count: &AtomicU32, v: u64) {
    let len = hist.len();
    let n = count.load(Ordering::Relaxed) as usize;
    for slot in hist.iter().take(n.min(len)) {
        if slot.load(Ordering::Relaxed) == v {
            return;
        }
    }
    if n < len {
        hist[n].store(v, Ordering::Relaxed);
        count.store(n as u32 + 1, Ordering::Relaxed);
    } else {
        for i in 1..len {
            let prev = hist[i].load(Ordering::Relaxed);
            hist[i - 1].store(prev, Ordering::Relaxed);
        }
        hist[len - 1].store(v, Ordering::Relaxed);
    }
}

/// Record a canvas the engine used (deduplicated, oldest dropped).
pub fn record_canvas(w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let v = ((w as u64) << 32) | (h as u32) as u64;
    record_history(&CANVAS_HIST, &CANVAS_HIST_N, v);
}

/// The canvases the engine has used, oldest first.
pub fn canvas_history() -> Vec<(i32, i32)> {
    let n = CANVAS_HIST_N.load(Ordering::Relaxed) as usize;
    (0..n.min(CANVAS_HIST.len()))
        .map(|i| {
            let v = CANVAS_HIST[i].load(Ordering::Relaxed);
            ((v >> 32) as i32, v as u32 as i32)
        })
        .collect()
}

/// The scene's top inset, learned at runtime from a battle viewport drawn at
/// the startup canvas (`(0,y,w,h-y)` -> `y`). 0 = not learned yet; the hook
/// then falls back to the profile's `[scene] top_inset`, and finally to the
/// last-resort layout constant.
pub static SCENE_INSET: AtomicI32 = AtomicI32::new(0);

pub fn scene_inset() -> i32 {
    let learned = SCENE_INSET.load(Ordering::Relaxed);
    if learned > 0 {
        learned
    } else {
        crate::profile::scene_inset_default()
    }
}

/// Does `(w, h)` have the battle-scene shape `(canvas_w, canvas_h - inset)`
/// for any canvas the engine has used? The vp trigger in the GL hook uses
/// this instead of hard-coding the current canvas, so a scene still laid out
/// for the battle-entry canvas (mid-battle resize) is recognised too.
pub fn is_scene_rect(w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let inset = {
        let cfg = crate::config().scene_inset;
        if cfg > 0 { cfg } else { scene_inset() }
    };
    (0..CANVAS_HIST_N.load(Ordering::Relaxed) as usize).any(|i| {
        let v = CANVAS_HIST[i].load(Ordering::Relaxed);
        let (cw, ch) = ((v >> 32) as i32, v as u32 as i32);
        w == cw && h == ch - inset
    })
}

/// The scene rect the engine itself asked for, packed like `pack_rect`
/// (x, y, w, h as u16). Written by the `glViewport` hook whenever the engine
/// asks for a battle-scene-shaped viewport at `y=0`; the scene-rect repair
/// scans for engine objects holding this exact rect.
pub static SCENE_VP_PACKED: AtomicU64 = AtomicU64::new(u64::MAX);
/// Distinct scene rects seen (packed), oldest dropped. The render-target rect
/// is frozen at battle entry, so after a resize the live scene rect no longer
/// matches the current canvas - candidates are matched against this history.
pub static SCENE_VP_HIST: [AtomicU64; 8] = [const { AtomicU64::new(u64::MAX) }; 8];
pub static SCENE_VP_HIST_N: AtomicU32 = AtomicU32::new(0);

/// Record a scene rect the engine asked for (deduplicated). Any y is kept:
/// after a mid-battle resize the engine has been observed asking for the
/// scene at `y` values that are neither 0 nor the inset, and the history is
/// matched by (w, h) only.
pub fn record_scene_vp(x: i32, y: i32, w: i32, h: i32) {
    if x != 0 || y < 0 || w <= 0 || h <= 0 {
        return;
    }
    let v = ((x as u16 as u64) << 48)
        | ((y as u16 as u64) << 32)
        | ((w as u16 as u64) << 16)
        | (h as u16 as u64);
    SCENE_VP_PACKED.store(v, Ordering::Relaxed);
    record_history(&SCENE_VP_HIST, &SCENE_VP_HIST_N, v);
}

/// Actual client size of the window, and the logical -> physical mapping.
pub static PHYS_W: AtomicI32 = AtomicI32::new(0);
pub static PHYS_H: AtomicI32 = AtomicI32::new(0);
pub static SCALE_X: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f
pub static SCALE_Y: AtomicU32 = AtomicU32::new(0x3f80_0000);
/// Letterbox/pillarbox offset in physical pixels (only used when stretch=0).
pub static OFF_X: AtomicI32 = AtomicI32::new(0);
pub static OFF_Y: AtomicI32 = AtomicI32::new(0);
/// 0 = the default framebuffer, anything else = an offscreen FBO.
pub static DRAW_FBO: AtomicU32 = AtomicU32::new(0);
pub static READ_FBO: AtomicU32 = AtomicU32::new(0);

/// Client size the current window mode is going to end up at. The engine can
/// create its GL context before our resize has been applied, and the intro
/// then uses the context's creation-time viewport forever. wglMakeCurrent
/// re-asserts this size so the intro is laid out for the window it will
/// actually be shown in.
pub static TARGET_W: AtomicI32 = AtomicI32::new(0);
pub static TARGET_H: AtomicI32 = AtomicI32::new(0);

/// Frames left where the live GL viewport/scissor are logged after a resize.
pub static RESIZE_DIAG: AtomicU32 = AtomicU32::new(0);

pub const FRAMED_STYLE: u32 = WS_CAPTION
    | WS_SYSMENU
    | WS_THICKFRAME
    | WS_MINIMIZEBOX
    | WS_MAXIMIZEBOX
    | WS_VISIBLE
    | WS_CLIPSIBLINGS
    | WS_CLIPCHILDREN;
pub const BORDERLESS_STYLE: u32 = WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;

pub fn logical_size() -> (i32, i32) {
    (
        LOGICAL_W.load(Ordering::Relaxed),
        LOGICAL_H.load(Ordering::Relaxed),
    )
}

pub fn scale_xy() -> (f32, f32) {
    (
        f32::from_bits(SCALE_X.load(Ordering::Relaxed)),
        f32::from_bits(SCALE_Y.load(Ordering::Relaxed)),
    )
}

pub fn is_scaled() -> bool {
    let (sx, sy) = scale_xy();
    sx != 1.0 || sy != 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_dedupes_and_drops_oldest() {
        let hist = [const { AtomicU64::new(0) }; 3];
        let count = AtomicU32::new(0);

        record_history(&hist, &count, 1);
        record_history(&hist, &count, 1); // duplicate
        assert_eq!(count.load(Ordering::Relaxed), 1);

        record_history(&hist, &count, 2);
        record_history(&hist, &count, 3);
        assert_eq!(count.load(Ordering::Relaxed), 3);

        record_history(&hist, &count, 4); // full: drop the oldest
        let values: Vec<u64> = hist.iter().map(|s| s.load(Ordering::Relaxed)).collect();
        assert_eq!(values, vec![2, 3, 4]);
        assert_eq!(count.load(Ordering::Relaxed), 3);
    }
}
