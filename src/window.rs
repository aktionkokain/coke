//! Window discovery, subclassing, logical<->physical mapping and window modes.

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::{c_char, c_void};

// ---- code ----
// ------------------------------------------------------------- render size

pub unsafe fn monitor_rect(hwnd: Hwnd) -> Rect {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if !monitor.is_null() {
            let mut info = MonitorInfo {
                cb_size: core::mem::size_of::<MonitorInfo>() as u32,
                rc_monitor: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                rc_work: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                flags: 0,
            };
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                return info.rc_monitor;
            }
        }
        Rect {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    }
}

pub unsafe fn work_area() -> Rect {
    unsafe {
        let mut wa = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let ok = SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut Rect as *mut c_void, 0);
        if ok != 0 {
            wa
        } else {
            Rect {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }
}

/// Outer rectangle (position + size) for a framed window: `window_scale` percent
/// of the monitor, centred in the work area.
///
/// The monitor is used rather than the logical resolution: the engine's canvas
/// can be much smaller than the desktop (a 1024x768 game on a 2560x1440 screen),
/// and the window size must not shrink just because the engine renders small.
pub unsafe fn framed_geometry(hwnd: Hwnd) -> (i32, i32, i32, i32) {
    unsafe {
        let cfg = config().clone();
        let mon = monitor_rect(hwnd);
        let mw = (mon.right - mon.left).max(1) as i64;
        let mh = (mon.bottom - mon.top).max(1) as i64;
        let pct = cfg.window_scale.clamp(25, 100) as i64;
        let cw = ((mw * pct) / 100).max(640) as i32;
        let ch = ((mh * pct) / 100).max(480) as i32;
        let mut r = Rect {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        };
        AdjustWindowRect(&mut r, FRAMED_STYLE, 0);
        let (ow, oh) = (r.right - r.left, r.bottom - r.top);
        let wa = work_area();
        let x = wa.left + ((wa.right - wa.left - ow) / 2).max(0);
        let y = wa.top + ((wa.bottom - wa.top - oh) / 2).max(0);
        (x, y, ow, oh)
    }
}

/// The logical resolution the engine is told the screen is.
///
/// With `width`/`height` unset (0) this **follows the window**: the engine is
/// told the client area's real pixel size, so it renders natively at the size
/// the window actually is and the mod's logical -> physical mapping collapses to
/// the identity. That matters for more than sharpness: the mod's cursor
/// transform can only be correct if this number matches what the engine believes
/// the window is, and any mismatch between the reported screen size and the real
/// client surface shifts unit picking away from the cursor.
///
/// Set `width`/`height` to pin a fixed logical resolution instead; the mod then
/// scales the canvas to the window (older behaviour, cheaper on the GPU, but the
/// engine renders at the fixed size and the coordinate mapping is no longer the
/// identity).
pub unsafe fn compute_logical_size(cfg: &Config) -> (i32, i32) {
    unsafe {
        if cfg.width > 0 && cfg.height > 0 {
            return (cfg.width, cfg.height);
        }
        // Follow the window. The client size the first window will have is
        // predictable from the monitor and window_scale, so start there instead of
        // starting at the monitor and resizing the engine's layout a moment later.
        let mon = monitor_rect(core::ptr::null_mut());
        let (mw, mh) = (
            (mon.right - mon.left).max(1) as i64,
            (mon.bottom - mon.top).max(1) as i64,
        );
        if cfg.start_fullscreen {
            return (mw as i32, mh as i32);
        }
        let pct = cfg.window_scale.clamp(25, 100) as i64;
        (
            ((mw * pct) / 100).max(640) as i32,
            ((mh * pct) / 100).max(480) as i32,
        )
    }
}

/// Point the engine's idea of the screen at the window's real client size.
///
/// Picks up the client area after a resize, an F11 transition or the engine
/// recreating its window, and asks the engine to lay its canvas out again at
/// that size. No-op when `width`/`height` pin a fixed logical resolution.
pub unsafe fn adopt_client_logical(hwnd: Hwnd) -> bool {
    unsafe {
        let cfg = config().clone();
        if cfg.width > 0 && cfg.height > 0 {
            return false;
        }
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut c) == 0 {
            return false;
        }
        let (cw, ch) = (c.right - c.left, c.bottom - c.top);
        if cw < 320 || ch < 240 {
            return false;
        }
        let (lw, lh) = logical_size();
        if lw == cw && lh == ch {
            return false;
        }
        LOGICAL_W.store(cw, Ordering::Relaxed);
        LOGICAL_H.store(ch, Ordering::Relaxed);
        sync_scale(hwnd);
        log(&format!("logical follows client: {lw}x{lh} -> {cw}x{ch}"));
        // The engine cached its canvas at startup and its window procedure ignores
        // WM_SIZE, so posting WM_SIZE cannot relayout it. Write the new size into
        // the engine's own cached canvas instead; the engine reads those ints for
        // UI layout and for the mouse Y inversion on every message.
        patch_engine_canvas(cw, ch);
        InvalidateRect(hwnd, core::ptr::null(), 0);
        kick_repaint(hwnd);
        true
    }
}

/// True when `app` looks like the engine's app object *and* is the object this
/// process is using: committed, readable, carrying our HWND, and holding a
/// canvas the engine has legitimately used.
///
/// The HWND is the strongest identity available. The canvas check accepts the
/// current logical size or any canvas in `canvas_history()` because during a
/// resize the object still holds the previous canvas until we write the new
/// one.
unsafe fn validate_app_candidate(app: usize, hwnd: Hwnd) -> Option<(i32, i32)> {
    unsafe {
        if app < 0x10000 || hwnd.is_null() {
            return None;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(
            app as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) == 0
            || m.state != MEM_COMMIT
            || m.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
        {
            return reject_app("app object is not committed/readable");
        }
        let w = core::ptr::read_unaligned(app as *const i32);
        let h = core::ptr::read_unaligned((app + 4) as *const i32);
        if !(320..=16384).contains(&w) || !(240..=16384).contains(&h) {
            return reject_app(&format!("implausible canvas {w}x{h}"));
        }

        let canvas = profile::canvas()?;
        let want = hwnd as usize;
        let region_end = m.base_address as usize + m.region_size as usize;
        let mut source = "profile";
        let mut stored = 0usize;
        if canvas.app_hwnd_off != 0 {
            let off = canvas.app_hwnd_off as usize;
            if off + 4 <= region_end.saturating_sub(app) {
                stored = core::ptr::read_unaligned((app + off) as *const usize);
            } else {
                return reject_app(&format!(
                    "app_hwnd_off 0x{off:x} lies outside the app object's region"
                ));
            }
        }
        if stored != want {
            // Either the profile has no offset, or the field no longer holds this
            // window (the engine recreates its main window during startup). Fall
            // back to a scan of the object: exactly one occurrence of the HWND is
            // still proof, zero or several is a refusal.
            let span = region_end.saturating_sub(app).min(0x2000);
            let mut hits = 0usize;
            let mut found = 0usize;
            let mut found_off = 0usize;
            let mut off = 0usize;
            while off + 4 <= span {
                let v = core::ptr::read_unaligned((app + off) as *const u32) as usize;
                if v == want {
                    hits += 1;
                    found = v;
                    found_off = off;
                }
                off += 4;
            }
            if hits != 1 {
                return reject_app(&format!(
                    "hwnd 0x{want:x} not found exactly once in the app object ({hits} matches)"
                ));
            }
            if canvas.app_hwnd_off != 0 {
                static OFFSET_WARNED: AtomicU32 = AtomicU32::new(0);
                if OFFSET_WARNED.swap(1, Ordering::Relaxed) == 0 {
                    log(&format!(
                        "canvas patch: app+0x{:x} holds 0x{stored:x}, not the main window 0x{want:x}; \
                     using the single matching field at app+0x{found_off:x}",
                        canvas.app_hwnd_off
                    ));
                }
            }
            stored = found;
            source = "scan";
        }
        if stored != want {
            return reject_app("hwnd mismatch");
        }
        if !canvas_is_plausible(w, h, canvas.size_tolerance_pct) {
            let (lw, lh) = logical_size();
            return reject_app(&format!(
                "canvas {w}x{h} does not match logical {lw}x{lh} or history"
            ));
        }

        static FOUND: AtomicU32 = AtomicU32::new(0);
        if FOUND.swap(1, Ordering::Relaxed) == 0 {
            log(&format!(
                "canvas patch: adopted app=0x{app:x} canvas={w}x{h} stored_hwnd=0x{stored:x} \
             hwnd_source={source} checks=ok"
            ));
            // The engine's initial canvas is the first layout key: screens built
            // at startup (main menu) are centered for it.
            record_canvas(w, h);
        }
        Some((w, h))
    }
}

/// One rejection line per reason, rate-limited to keep the log readable while
/// the watcher retries every tick.
fn reject_app(reason: &str) -> Option<(i32, i32)> {
    static REJECTS: AtomicU32 = AtomicU32::new(0);
    if REJECTS.fetch_add(1, Ordering::Relaxed) < 4 {
        log(&format!("canvas patch: app rejected: {reason}"));
    }
    None
}

fn canvas_is_plausible(w: i32, h: i32, tolerance_pct: u32) -> bool {
    let near = |a: i32, b: i32| {
        if b <= 0 {
            return false;
        }
        if tolerance_pct == 0 {
            a == b
        } else {
            (a - b).abs() as i64 * 100 <= b as i64 * tolerance_pct as i64
        }
    };
    let (lw, lh) = logical_size();
    if lw > 0 && lh > 0 && near(w, lw) && near(h, lh) {
        return true;
    }
    canvas_history()
        .iter()
        .any(|&(cw, ch)| near(w, cw) && near(h, ch))
}

/// True when the profile provides `[canvas]` and the app object passes every
/// check. Never adopts without a window.
pub unsafe fn app_object() -> Option<usize> {
    unsafe {
        if profile::canvas().is_none() {
            static LOGGED: AtomicU32 = AtomicU32::new(0);
            if LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log("canvas patch: profile has no [canvas] block; leaving the engine canvas alone");
            }
            return None;
        }
        let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
        if hwnd.is_null() {
            return None;
        }
        let canvas = profile::canvas()?;
        let global = profile_addr(canvas.app_ptr)?;
        let app = core::ptr::read_unaligned(global as *const usize);
        validate_app_candidate(app, hwnd).map(|_| app)
    }
}

/// Write the new client size into the engine's cached canvas (`app[0]` /
/// `app[4]`). No-op when `canvas_patch` is off, when the build has no canvas
/// block, or when the app object does not validate. Returns true when the
/// canvas matches the requested size afterwards.
pub unsafe fn patch_engine_canvas(w: i32, h: i32) -> bool {
    unsafe {
        if !config().canvas_patch || w < 320 || h < 240 {
            return false;
        }
        let Some(app) = app_object() else {
            return false;
        };
        // Re-validate immediately before writing: the object can be freed or its
        // global repointed between adoption and this write.
        let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
        if validate_app_candidate(app, hwnd).is_none() {
            return false;
        }
        let old_w = core::ptr::read_unaligned(app as *const i32);
        let old_h = core::ptr::read_unaligned((app + 4) as *const i32);
        if old_w == w && old_h == h {
            return true;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        let mut old_prot = 0u32;
        let writable = VirtualQuery(
            app as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) != 0
            && matches!(
                m.protect & 0xff,
                PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
            );
        if !writable
            && VirtualProtect(app as *mut c_void, 8, PAGE_EXECUTE_READWRITE, &mut old_prot) == 0
        {
            return false;
        }
        core::ptr::write_unaligned(app as *mut i32, w);
        core::ptr::write_unaligned((app + 4) as *mut i32, h);
        if !writable {
            let mut ignored = 0u32;
            VirtualProtect(app as *mut c_void, 8, old_prot, &mut ignored);
        }
        PREV_LOGICAL_W.store(old_w, Ordering::Relaxed);
        PREV_LOGICAL_H.store(old_h, Ordering::Relaxed);
        record_canvas(old_w, old_h);
        // Record the canvas we are writing too, not just the one we are leaving.
        // Validation accepts "the current canvas or any canvas in history"; without
        // this the *next* transition (un-maximize, F11, shrink) sees the canvas we
        // patched to as unknown and rejects the patch, leaving the engine's cached
        // size (mouse Y-inversion + layout) stale exactly when the window is no
        // longer maximised.
        record_canvas(w, h);
        schedule_widget_recenter();
        // Diagnostic: snapshot the live widget tree after the engine has settled.
        schedule_widget_dump(DUMP_ON_RESIZE);
        // Native re-layout: replay the engine's widget rects against the new canvas.
        schedule_widget_relayout();
        log(&format!(
            "canvas patch: engine canvas {old_w}x{old_h} -> {w}x{h} (app=0x{app:x})"
        ));
        true
    }
}

/// Keep the engine's cached canvas equal to the real client size in
/// follow-the-window mode. Cheap enough to run from the watcher every tick; it
/// covers the case where the app object did not exist yet when `adopt` ran.
pub unsafe fn keep_engine_canvas_in_sync(hwnd: Hwnd) {
    unsafe {
        if !config().canvas_patch {
            return;
        }
        // A pinned `width`/`height` means a fixed canvas: the engine must keep
        // rendering at that size and the mod scales it, so do not touch it.
        if config().width > 0 && config().height > 0 {
            return;
        }
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut c) == 0 {
            return;
        }
        let (cw, ch) = (c.right - c.left, c.bottom - c.top);
        if cw < 320 || ch < 240 {
            return;
        }
        // Only in identity mode. While a resize is still being adopted, LOGICAL
        // intentionally trails the client; do not patch until they agree.
        let (lw, lh) = logical_size();
        if lw != cw || lh != ch {
            return;
        }
        patch_engine_canvas(cw, ch);
    }
}

// ------------------------------------------------------------------- hooks

/// CMBS calls its window class "CM Black Sea"; the other titles in the series
/// follow the same "CM &lt;name&gt;" pattern. Accept the known main window as well so
/// an exotic class name still works once the window has been discovered.
pub unsafe fn class_is_cm(hwnd: Hwnd) -> bool {
    unsafe {
        let mut buf = [0u8; 64];
        let n = GetClassNameA(hwnd, buf.as_mut_ptr() as *mut c_char, buf.len() as i32);
        if n <= 0 {
            return false;
        }
        let name = &buf[..n as usize];
        name.len() >= 3
            && name[0].eq_ignore_ascii_case(&b'C')
            && name[1].eq_ignore_ascii_case(&b'M')
            && name[2] == b' '
    }
}

pub unsafe fn is_game_window(hwnd: Hwnd) -> bool {
    unsafe {
        if !owns_window(hwnd) {
            return false;
        }
        if current_window(hwnd) {
            return true;
        }
        class_is_cm(hwnd)
    }
}

pub fn pack_point(x: i32, y: i32) -> isize {
    (((y as u16 as usize) << 16) | (x as u16 as usize)) as isize
}

/// Inverse of [`pack_point`].
pub fn unpack_point(packed: isize) -> (i32, i32) {
    (
        (packed & 0xffff) as i16 as i32,
        ((packed >> 16) & 0xffff) as i16 as i32,
    )
}

/// Keep a frozen cursor position away from the window edges.
///
/// The engine pans the camera when the cursor is near a client edge. While the
/// user is dragging the window the real cursor sits on the frame, so the cursor
/// hook substitutes the last in-client sample; clamping that sample inward by
/// roughly 10% (16-64 px) guarantees it is outside the engine's edge-scroll
/// band even after the window has been resized much smaller.
pub fn pan_guard_point(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    if w <= 0 || h <= 0 {
        return (x, y);
    }
    let mx = (w / 10).clamp(16, 64).min((w - 1) / 2);
    let my = (h / 10).clamp(16, 64).min((h - 1) / 2);
    (x.clamp(mx, w - 1 - mx), y.clamp(my, h - 1 - my))
}

/// Convert a physical client coordinate to the logical coordinate space the
/// engine draws and hit-tests in.
pub fn client_to_logical(x: i32, y: i32) -> (i32, i32) {
    let (sx, sy) = scale_xy();
    let sx = sx.max(0.0001);
    let sy = sy.max(0.0001);
    let ox = OFF_X.load(Ordering::Relaxed);
    let oy = OFF_Y.load(Ordering::Relaxed);
    (
        ((x - ox) as f32 / sx).round() as i32,
        ((y - oy) as f32 / sy).round() as i32,
    )
}

pub unsafe fn transform_client_lparam(lparam: isize) -> isize {
    let x = (lparam & 0xffff) as i16 as i32;
    let y = ((lparam >> 16) & 0xffff) as i16 as i32;
    let (lx, ly) = client_to_logical(x, y);
    pack_point(lx, ly)
}

/// Log the first few mouse transforms so the physical -> logical mapping the
/// engine receives can be compared with where it draws things.
pub static MOUSE_LOG: AtomicU32 = AtomicU32::new(0);

pub unsafe fn transform_screen_lparam(hwnd: Hwnd, lparam: isize) -> isize {
    unsafe {
        let mut pt = Point {
            x: (lparam & 0xffff) as i16 as i32,
            y: ((lparam >> 16) & 0xffff) as i16 as i32,
        };
        if ScreenToClient(hwnd, &mut pt) == 0 {
            return lparam;
        }
        let (lx, ly) = client_to_logical(pt.x, pt.y);
        pack_point(lx, ly)
    }
}

/// Custom window procedure: force-quit close, normal maximise, live resize and
/// physical -> logical mouse coordinates.
/// Throttled "the window was manipulated" logging for WM_SIZE /
/// WM_WINDOWPOSCHANGED. These messages can arrive dozens of times per second
/// during a drag; log at most one line per 500 ms, with the current geometry
/// so the log shows exactly when and how the window changed.
unsafe fn log_window_manip(what: &str, hwnd: Hwnd, wparam: usize, lparam: isize) {
    unsafe {
        static LAST: AtomicU32 = AtomicU32::new(0);
        let now = GetTickCount();
        if now.wrapping_sub(LAST.load(Ordering::Relaxed)) < 500 {
            return;
        }
        LAST.store(now, Ordering::Relaxed);
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let geom = if GetClientRect(hwnd, &mut c) != 0 {
            format!("{}x{}", c.right - c.left, c.bottom - c.top)
        } else {
            "<no client>".into()
        };
        log(&format!(
            "{what}: wparam=0x{wparam:x} lparam=0x{:x} client {geom}",
            lparam as usize
        ));
    }
}

/// True when the configured resize modifier is held right now.
unsafe fn resize_modifier_active(mode: u8) -> bool {
    unsafe {
        match mode {
            RESIZE_MODIFIER_CTRL_SHIFT => {
                GetAsyncKeyState(VK_CONTROL) < 0 && GetAsyncKeyState(VK_SHIFT) < 0
            }
            RESIZE_MODIFIER_CTRL => GetAsyncKeyState(VK_CONTROL) < 0,
            RESIZE_MODIFIER_SHIFT => GetAsyncKeyState(VK_SHIFT) < 0,
            RESIZE_MODIFIER_ALT => GetAsyncKeyState(VK_MENU) < 0,
            _ => false,
        }
    }
}

/// Native `HT*` resize code for a client-area hit, or `None` to pass the click
/// through to the game.
///
/// Two layers, both framed-window only:
/// - a band `resize_border` px inside each edge is always a resize handle, so
///   the effective border is much fatter than the native frame;
/// - while `resize_modifier` is held the whole client resizes from the nearest
///   third (the centre uses the nearest edge), so the window can be resized
///   without aiming at an edge at all.
unsafe fn resize_hit(hwnd: Hwnd, lparam: isize) -> Option<isize> {
    unsafe {
        let cfg = config();
        let mut pt = Point {
            x: (lparam & 0xffff) as i16 as i32,
            y: ((lparam >> 16) & 0xffff) as i16 as i32,
        };
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if ScreenToClient(hwnd, &mut pt) == 0 || GetClientRect(hwnd, &mut c) == 0 {
            return None;
        }
        let (w, h) = (c.right - c.left, c.bottom - c.top);
        if w <= 0 || h <= 0 || pt.x < 0 || pt.y < 0 || pt.x >= w || pt.y >= h {
            return None;
        }

        let (left, top, right, bottom) = if resize_modifier_active(cfg.resize_modifier) {
            let third_x = w / 3;
            let third_y = h / 3;
            let mut left = pt.x < third_x;
            let mut right = pt.x >= w - third_x;
            let mut top = pt.y < third_y;
            let mut bottom = pt.y >= h - third_y;
            if !left && !right && !top && !bottom {
                let (dl, dr, dt, db) = (pt.x, w - pt.x, pt.y, h - pt.y);
                let nearest = dl.min(dr).min(dt).min(db);
                if nearest == dl {
                    left = true;
                } else if nearest == dr {
                    right = true;
                } else if nearest == dt {
                    top = true;
                } else {
                    bottom = true;
                }
            }
            (left, top, right, bottom)
        } else {
            let band = cfg.resize_border.max(0);
            if band == 0 {
                return None;
            }
            (pt.x < band, pt.y < band, pt.x >= w - band, pt.y >= h - band)
        };

        Some(match (top, bottom, left, right) {
            (true, _, true, _) => HTTOPLEFT,
            (true, _, _, true) => HTTOPRIGHT,
            (_, true, true, _) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTTOP,
            (_, true, _, _) => HTBOTTOM,
            (_, _, true, _) => HTLEFT,
            (_, _, _, true) => HTRIGHT,
            _ => return None,
        })
    }
}

const ORIGINAL_PROC_PROPERTY: *const c_char = c"Coke.OriginalWndProc".as_ptr();

pub unsafe extern "system" fn custom_wndproc(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    unsafe {
        let original = GetPropA(hwnd, ORIGINAL_PROC_PROPERTY) as isize;
        let call_original = |hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize| -> isize {
            if original != 0 && is_executable_ptr(original as usize) {
                CallWindowProcW(original, hwnd, msg, wparam, lparam)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        };

        if msg == 0x0082 {
            // WM_NCDESTROY
            RemovePropA(hwnd, ORIGINAL_PROC_PROPERTY);
            return call_original(hwnd, msg, wparam, lparam);
        }
        if !current_window(hwnd) {
            return call_original(hwnd, msg, wparam, lparam);
        }
        if crate::range::handle_message(msg, wparam, lparam) {
            return 0;
        }
        match msg {
            WM_CLOSE => {
                if config().forced_exit {
                    log("WM_CLOSE: force-quitting");
                    ExitProcess(0);
                }
                return call_original(hwnd, msg, wparam, lparam);
            }
            WM_SYSCOMMAND => {
                match wparam & 0xFFF0 {
                    SC_CLOSE => {
                        if config().forced_exit {
                            log("SC_CLOSE: force-quitting");
                            ExitProcess(0);
                        }
                        return call_original(hwnd, msg, wparam, lparam);
                    }
                    SC_MAXIMIZE => {
                        WINDOW_MODE.store(2, Ordering::Relaxed);
                        if MAXIMIZE_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                            log("SC_MAXIMIZE: maximising the framed window");
                        }
                        ShowWindow(hwnd, SW_MAXIMIZE);
                        adopt_client_logical(hwnd);
                        return 0;
                    }
                    SC_RESTORE => {
                        WINDOW_MODE.store(0, Ordering::Relaxed);
                        ShowWindow(hwnd, SW_RESTORE);
                        adopt_client_logical(hwnd);
                        return 0;
                    }
                    SC_MINIMIZE => {
                        // Let a deliberate minimise through; the ShowWindow hook
                        // only suppresses the engine's minimise-on-focus-loss.
                        ALLOW_MINIMIZE.store(1, Ordering::Relaxed);
                        let r = call_original(hwnd, msg, wparam, lparam);
                        ALLOW_MINIMIZE.store(0, Ordering::Relaxed);
                        return r;
                    }
                    _ => {}
                }
            }
            WM_ACTIVATEAPP => {
                let active = wparam != 0;
                if WINDOW_MODE.load(Ordering::Relaxed) == 1 {
                    // Keep a borderless window above the taskbar while active, but
                    // drop topmost when switching away so it cannot cover other apps.
                    let after = if active { HWND_TOPMOST } else { HWND_NOTOPMOST } as Hwnd;
                    SetWindowPos(
                        hwnd,
                        after,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
            WM_SIZE => {
                log_window_manip("WM_SIZE", hwnd, wparam, lparam);
                sync_scale(hwnd);
            }
            WM_WINDOWPOSCHANGED => {
                log_window_manip("WM_WINDOWPOSCHANGED", hwnd, wparam, lparam);
            }
            WM_ENTERSIZEMOVE => {
                // Moving or resizing the frame puts the real cursor on the
                // window edge, which is exactly where the engine's per-frame
                // cursor poll starts panning the camera. Freeze the reported
                // position until the gesture ends.
                WINDOW_DRAGGING.store(1, Ordering::Relaxed);
            }
            WM_EXITSIZEMOVE => {
                WINDOW_DRAGGING.store(0, Ordering::Relaxed);
                // The user finished dragging a window edge. Post one synthetic
                // WM_MOUSEMOVE so the engine's idle menu loop wakes up and redraws
                // the now-resized window. Without this the screen stays black until
                // the user moves the mouse or scrolls.
                let result = call_original(hwnd, msg, wparam, lparam);
                let real_gcp = REAL_GET_CURSOR_POS.load(Ordering::Relaxed);
                if real_gcp != 0 {
                    let f_gcp: unsafe extern "system" fn(*mut Point) -> i32 =
                        core::mem::transmute(real_gcp);
                    let mut pt = Point { x: 0, y: 0 };
                    if f_gcp(&mut pt) != 0 {
                        let mut origin = Point { x: 0, y: 0 };
                        if ClientToScreen(hwnd, &mut origin) != 0 {
                            let (lx, ly) = client_to_logical(pt.x - origin.x, pt.y - origin.y);
                            PostMessageW(hwnd, WM_MOUSEMOVE, 0, pack_point(lx, ly));
                        }
                    }
                }
                // The engine ignores WM_SIZE. Adopt the final client size and write
                // it into the engine's cached canvas, then wake the renderer.
                adopt_client_logical(hwnd);
                InvalidateRect(hwnd, core::ptr::null(), 0);
                kick_repaint(hwnd);
                return result;
            }
            WM_NCHITTEST => {
                // Resizing was only possible on the thin native frame. Claim an
                // inset band just inside every client edge (and, while the
                // resize modifier is held, the whole client) so Windows runs its
                // own resize loop with the right cursor. This never moves the
                // window.
                let hit = call_original(hwnd, msg, wparam, lparam);
                if hit == HTCLIENT
                    && WINDOW_MODE.load(Ordering::Relaxed) != 1
                    && let Some(code) = resize_hit(hwnd, lparam)
                {
                    return code;
                }
                return hit;
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN
            | WM_RBUTTONUP | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN | WM_MBUTTONUP
            | WM_MBUTTONDBLCLK | WM_XBUTTONDOWN | WM_XBUTTONUP => {
                if camera_input_capture() && matches!(msg, WM_RBUTTONDOWN | WM_RBUTTONDBLCLK) {
                    CAMERA_RMB_MESSAGE_DOWN.store(1, Ordering::Release);
                    CAMERA_RMB_DRAG_CANCELLED.store(0, Ordering::Release);
                    CAMERA_RMB_DOWN_POINT.store(transform_client_lparam(lparam), Ordering::Release);
                    CAMERA_RMB_DOWN_WPARAM.store(wparam, Ordering::Release);
                    CAMERA_RMB_DOWN_MESSAGE.store(msg, Ordering::Release);
                    // Delay native right-click delivery until release. If this
                    // becomes a drag, the engine never enters its origin-based
                    // camera mode; coke-camera alone applies cursor deltas.
                    return 0;
                }
                if camera_input_capture() && matches!(msg, WM_MBUTTONDOWN | WM_MBUTTONDBLCLK) {
                    CAMERA_MMB_MESSAGE_DOWN.store(1, Ordering::Release);
                    return 0;
                }
                if camera_input_capture() && msg == WM_MBUTTONUP {
                    CAMERA_MMB_MESSAGE_DOWN.store(0, Ordering::Release);
                    return 0;
                }
                if camera_input_capture()
                    && msg == WM_MOUSEMOVE
                    && CAMERA_MMB_MESSAGE_DOWN.load(Ordering::Acquire) != 0
                {
                    return 0;
                }
                if camera_input_capture() && msg == WM_MOUSEMOVE && camera_rmb_message_down() {
                    let new_lparam = transform_client_lparam(lparam);
                    let (down_x, down_y) =
                        unpack_point(CAMERA_RMB_DOWN_POINT.load(Ordering::Acquire));
                    let (move_x, move_y) = unpack_point(new_lparam);
                    if (move_x - down_x).abs() >= 3 || (move_y - down_y).abs() >= 3 {
                        CAMERA_RMB_DRAG_CANCELLED.store(1, Ordering::Release);
                    }
                    return 0;
                }
                if camera_input_capture() && msg == WM_RBUTTONUP {
                    let cancelled = camera_rmb_drag_cancelled();
                    CAMERA_RMB_MESSAGE_DOWN.store(0, Ordering::Release);
                    CAMERA_RMB_DRAG_CANCELLED.store(0, Ordering::Release);
                    let down_message = CAMERA_RMB_DOWN_MESSAGE.swap(0, Ordering::AcqRel);
                    let down_wparam = CAMERA_RMB_DOWN_WPARAM.swap(0, Ordering::AcqRel);
                    let down_lparam = CAMERA_RMB_DOWN_POINT.swap(0, Ordering::AcqRel);
                    let up_lparam = transform_client_lparam(lparam);
                    if cancelled {
                        return 0;
                    }
                    // It was a click rather than a drag. Replay the normal
                    // button pair so unit-order cancellation and other native
                    // right-click actions keep working.
                    let down_message = if down_message == WM_RBUTTONDBLCLK {
                        WM_RBUTTONDBLCLK
                    } else {
                        WM_RBUTTONDOWN
                    };
                    let _ = call_original(hwnd, down_message, down_wparam, down_lparam);
                    return call_original(hwnd, WM_RBUTTONUP, wparam & !0x0002, up_lparam);
                }
                let new_lparam = transform_client_lparam(lparam);
                if MOUSE_LOG.fetch_add(1, Ordering::Relaxed) < 24 {
                    let rx = (lparam & 0xffff) as i16 as i32;
                    let ry = ((lparam >> 16) & 0xffff) as i16 as i32;
                    let lx = (new_lparam & 0xffff) as i16 as i32;
                    let ly = ((new_lparam >> 16) & 0xffff) as i16 as i32;
                    let (sx, sy) = scale_xy();
                    let mut o = Point { x: 0, y: 0 };
                    ClientToScreen(hwnd, &mut o);
                    log(&format!(
                        "mouse msg=0x{msg:04x} raw=({rx},{ry}) -> logical=({lx},{ly}) \
                     scale={sx:.4}x{sy:.4} origin=({},{})",
                        o.x, o.y
                    ));
                }
                return call_original(hwnd, msg, wparam, new_lparam);
            }
            WM_MOUSEWHEEL => {
                coke_note_wheel(((wparam >> 16) & 0xffff) as u16 as i16 as i32 / 120);
                if camera_input_capture() {
                    return 0;
                }
                // Wheel messages carry screen coordinates, unlike the other mouse messages.
                let new_lparam = transform_screen_lparam(hwnd, lparam);
                return call_original(hwnd, msg, wparam, new_lparam);
            }
            _ => {}
        }

        call_original(hwnd, msg, wparam, lparam)
    }
}

/// Install window procedure hook to fix window buttons and UI clicks
pub unsafe fn is_executable_ptr(addr: usize) -> bool {
    unsafe {
        if addr < 0x10000 {
            return false;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(
            addr as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) == 0
        {
            return false;
        }
        if m.state != 0x1000 || (m.protect & 0x100) != 0 {
            return false;
        }
        matches!(m.protect & 0xff, 0x10 | 0x20 | 0x40 | 0x80)
    }
}

/// 0xffffxxxx is a USER32 A/W window-procedure handle, not a jump target.
pub fn is_wndproc_thunk(value: isize) -> bool {
    (value as usize & 0xffff_0000) == 0xffff_0000
}

pub unsafe fn install_wndproc_hook(hwnd: Hwnd) {
    unsafe {
        if !current_window(hwnd) {
            return;
        }
        let ours = custom_wndproc as *const () as isize;
        let current = GetWindowLongW(hwnd, GWLP_WNDPROC) as isize;
        if current == ours {
            return;
        }
        if current == 0 {
            return;
        }
        // The engine installs its own window procedure during startup. Until then
        // GetWindowLongW returns a 0xffffxxxx USER32 thunk (seen: 0xffff0b8b),
        // which is not a callable code pointer here. Wait for the real one; the
        // watcher retries a few thousand times during startup, and again after the
        // engine destroys and recreates its window.
        if !is_executable_ptr(current as usize) {
            static LOGGED: AtomicU32 = AtomicU32::new(0);
            if LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!(
                    "install_wndproc_hook: waiting for a real procedure \
                 (W=0x{current:x} A=0x{:x} thunk={})",
                    GetWindowLongA(hwnd, GWLP_WNDPROC),
                    is_wndproc_thunk(current)
                ));
            }
            return;
        }
        let previous_property = GetPropA(hwnd, ORIGINAL_PROC_PROPERTY);
        if SetPropA(hwnd, ORIGINAL_PROC_PROPERTY, current as Handle) == 0 {
            log(&format!(
                "wndproc: cannot store previous procedure: {}",
                GetLastError()
            ));
            return;
        }
        let previous = SetWindowLongW(hwnd, GWLP_WNDPROC, ours as i32) as isize;
        if previous == 0 {
            let error = GetLastError();
            if previous_property.is_null() {
                RemovePropA(hwnd, ORIGINAL_PROC_PROPERTY);
            } else {
                SetPropA(hwnd, ORIGINAL_PROC_PROPERTY, previous_property);
            }
            log(&format!("wndproc: subclass failed: {error}"));
            return;
        }
        if previous != current {
            SetPropA(hwnd, ORIGINAL_PROC_PROPERTY, previous as Handle);
        }
        log(&format!("wndproc hooked (previous=0x{previous:x})"));
    }
}

// ------------------------------------------------------------------ window

pub unsafe fn sync_scale(hwnd: Hwnd) {
    unsafe {
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetClientRect(hwnd, &mut c);
        let pw = (c.right - c.left).max(1);
        let ph = (c.bottom - c.top).max(1);
        let (lw, lh) = logical_size();
        let (lw, lh) = (lw.max(1), lh.max(1));

        let (sx, sy) = if config().stretch {
            (pw as f32 / lw as f32, ph as f32 / lh as f32)
        } else {
            let s = (pw as f32 / lw as f32).min(ph as f32 / lh as f32);
            (s, s)
        };
        let (ox, oy) = if config().stretch {
            (0, 0)
        } else {
            // Centre the logical image when the window aspect differs.
            (
                ((pw as f32 - lw as f32 * sx) * 0.5).round() as i32,
                ((ph as f32 - lh as f32 * sy) * 0.5).round() as i32,
            )
        };

        PHYS_W.store(pw, Ordering::Relaxed);
        PHYS_H.store(ph, Ordering::Relaxed);
        SCALE_X.store(sx.to_bits(), Ordering::Relaxed);
        SCALE_Y.store(sy.to_bits(), Ordering::Relaxed);
        OFF_X.store(ox, Ordering::Relaxed);
        OFF_Y.store(oy, Ordering::Relaxed);
    }
}

/// The engine repaints the whole frame when it is deactivated and reactivated
/// (that is exactly what "minimise and refocus" does). The minimise itself is
/// already suppressed by the CloseWindow/ShowWindow hooks, so a synthetic
/// activation cycle refreshes the frame without the window leaving the screen.
pub unsafe fn kick_repaint(hwnd: Hwnd) {
    unsafe {
        PostMessageW(hwnd, WM_ACTIVATEAPP, 0, 0);
        PostMessageW(hwnd, WM_ACTIVATEAPP, 1, 0);
    }
}

pub unsafe fn apply_window_mode(hwnd: Hwnd, fullscreen: bool) {
    unsafe {
        if !current_window(hwnd) {
            return;
        }
        let (lw, lh) = logical_size();
        if lw <= 0 || lh <= 0 {
            return;
        }
        let cfg = config().clone();

        if fullscreen {
            // Remember the framed geometry so F11 returns to exactly the size the
            // user left, including a live-resized or maximised window.
            if WINDOW_MODE.load(Ordering::Relaxed) != 1 {
                let mut r = Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                if GetWindowRect(hwnd, &mut r) != 0 && r.right > r.left && r.bottom > r.top {
                    FRAMED_LEFT.store(r.left, Ordering::Relaxed);
                    FRAMED_TOP.store(r.top, Ordering::Relaxed);
                    FRAMED_W.store(r.right - r.left, Ordering::Relaxed);
                    FRAMED_H.store(r.bottom - r.top, Ordering::Relaxed);
                    FRAMED_MAXIMIZED.store((IsZoomed(hwnd) != 0) as u32, Ordering::Relaxed);
                }
            }
            let mon = monitor_rect(hwnd);
            let (mw, mh) = (mon.right - mon.left, mon.bottom - mon.top);
            // Leave a few pixels uncovered: a window that exactly covers the
            // monitor is classified as a fullscreen app, and entering/leaving that
            // path is what blanks the display on some machines. Which edge gives up
            // the pixels is configurable so the gap never sits where the user is
            // likely to click through (the taskbar edge).
            let inset = cfg.fullscreen_inset.clamp(0, 16);
            let (mut x, mut y) = (mon.left, mon.top);
            let (mut ww, mut wh) = (mw, mh);
            let edge = cfg.fullscreen_inset_edge;
            match edge {
                0 => {
                    y += inset;
                    wh -= inset;
                }
                1 => wh -= inset,
                2 => {
                    x += inset;
                    ww -= inset;
                }
                3 => ww -= inset,
                4 => {
                    ww -= inset;
                    wh -= inset;
                }
                _ => {
                    y += inset;
                    wh -= inset;
                }
            }
            let (ww, wh) = (ww.max(1), wh.max(1));
            WINDOW_MODE.store(1, Ordering::Relaxed);
            TARGET_W.store(ww, Ordering::Relaxed);
            TARGET_H.store(wh, Ordering::Relaxed);
            SetWindowLongW(hwnd, GWL_STYLE, BORDERLESS_STYLE as i32);
            SetWindowPos(
                hwnd,
                HWND_TOPMOST as Hwnd,
                x,
                y,
                ww,
                wh,
                SWP_FRAMECHANGED | SWP_NOACTIVATE,
            );
            sync_scale(hwnd);
            log(&format!(
                "window = borderless fullscreen {ww}x{wh} at ({x},{y}) inset={inset} \
             edge={} (logical {lw}x{lh})",
                ["top", "bottom", "left", "right", "right+bottom"]
                    .get(edge as usize)
                    .copied()
                    .unwrap_or("top")
            ));
            // The engine ignores WM_SIZE; tell it the new client size by updating
            // its cached canvas, then wake the renderer.
            adopt_client_logical(hwnd);
            InvalidateRect(hwnd, core::ptr::null(), 0);
            kick_repaint(hwnd);
            return;
        }

        // Framed window: restore the last framed rectangle, or build one from the
        // monitor and `window_scale` on first use.
        let (x, y, ow, oh, maximised) = {
            let (sx, sy, sw, sh) = (
                FRAMED_LEFT.load(Ordering::Relaxed),
                FRAMED_TOP.load(Ordering::Relaxed),
                FRAMED_W.load(Ordering::Relaxed),
                FRAMED_H.load(Ordering::Relaxed),
            );
            if sw > 0 && sh > 0 {
                (
                    sx,
                    sy,
                    sw,
                    sh,
                    FRAMED_MAXIMIZED.load(Ordering::Relaxed) != 0,
                )
            } else {
                let (x, y, ow, oh) = framed_geometry(hwnd);
                (x, y, ow, oh, false)
            }
        };

        WINDOW_MODE.store(if maximised { 2 } else { 0 }, Ordering::Relaxed);
        // Target client size for this window: `ow`x`oh` is the outer size, so take
        // the frame away to get what the client area will be.
        {
            let mut r = Rect {
                left: 0,
                top: 0,
                right: ow,
                bottom: oh,
            };
            AdjustWindowRect(&mut r, FRAMED_STYLE, 0);
            let fw = (r.right - r.left) - ow;
            let fh = (r.bottom - r.top) - oh;
            TARGET_W.store((ow - fw).max(1), Ordering::Relaxed);
            TARGET_H.store((oh - fh).max(1), Ordering::Relaxed);
        }
        // If the window was created at the right size (CBT hook), do not resize it
        // again: another buffer recreation is exactly what we are trying to avoid.
        if !maximised {
            let mut cur = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(hwnd, &mut cur) != 0
                && cur.left == x
                && cur.top == y
                && cur.right - cur.left == ow
                && cur.bottom - cur.top == oh
                && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_POPUP == 0
            {
                sync_scale(hwnd);
                log(&format!("window already framed {ow}x{oh} at ({x},{y})"));
                adopt_client_logical(hwnd);
                return;
            }
        }
        SetWindowLongW(hwnd, GWL_STYLE, FRAMED_STYLE as i32);
        SetWindowPos(
            hwnd,
            HWND_NOTOPMOST as Hwnd,
            x,
            y,
            ow,
            oh,
            SWP_FRAMECHANGED | SWP_NOACTIVATE,
        );
        if maximised {
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
        sync_scale(hwnd);
        log(&format!(
            "window = framed {ow}x{oh} at ({x},{y}) (logical {lw}x{lh})"
        ));
        // The engine ignores WM_SIZE; tell it the new client size by updating its
        // cached canvas, then wake the renderer.
        adopt_client_logical(hwnd);
        InvalidateRect(hwnd, core::ptr::null(), 0);
        kick_repaint(hwnd);
    }
}

pub unsafe fn log_geometry(hwnd: Hwnd, why: &str) {
    unsafe {
        let mut r = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut c = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetWindowRect(hwnd, &mut r);
        GetClientRect(hwnd, &mut c);
        log(&format!(
            "{why}: window {w}x{h} at ({x},{y}), client {cw}x{ch}, style=0x{s:08x}",
            w = r.right - r.left,
            h = r.bottom - r.top,
            x = r.left,
            y = r.top,
            cw = c.right - c.left,
            ch = c.bottom - c.top,
            s = GetWindowLongW(hwnd, GWL_STYLE) as u32
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_must_match_logical_or_a_recorded_canvas() {
        LOGICAL_W.store(1920, Ordering::Relaxed);
        LOGICAL_H.store(1080, Ordering::Relaxed);
        assert!(canvas_is_plausible(1920, 1080, 0));
        assert!(!canvas_is_plausible(1280, 720, 0));
        record_canvas(1280, 720);
        assert!(
            canvas_is_plausible(1280, 720, 0),
            "a canvas the engine used earlier stays acceptable during a resize"
        );
        assert!(canvas_is_plausible(1928, 1086, 1), "1% tolerance");
        assert!(!canvas_is_plausible(1928, 1086, 0));
    }

    #[test]
    fn pan_guard_keeps_the_frozen_point_away_from_the_edges() {
        // A point on the top-left frame is moved inward.
        let (x, y) = pan_guard_point(-40, -12, 1920, 1080);
        assert_eq!((x, y), (64, 64));
        // A point in the middle is untouched.
        assert_eq!(pan_guard_point(960, 540, 1920, 1080), (960, 540));
        // Tiny windows still get a valid, in-client point.
        let (x, y) = pan_guard_point(0, 0, 40, 40);
        assert!((0..40).contains(&x));
        assert!((0..40).contains(&y));
        assert_eq!(pan_guard_point(5, 5, 0, 0), (5, 5), "no size, no clamp");
    }
}
