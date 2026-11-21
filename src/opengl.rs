//! Intercepted GL/wgl entry points and the swap-buffer frame pump.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::ffi::{c_char, c_void};

// ---- code ----
// ------------------------------------------------------------------ GL bits

pub unsafe fn log_gl_identity() {
    unsafe {
        let f = REAL_GL_GET_STRING.load(Ordering::Relaxed);
        if f == 0 {
            return;
        }
        let get: unsafe extern "system" fn(u32) -> *const u8 = core::mem::transmute(f);
        let mut out = String::from("GL:");
        for (id, label) in [
            (0x1F00u32, "VENDOR"),
            (0x1F01, "RENDERER"),
            (0x1F02, "VERSION"),
        ] {
            let p = get(id);
            if p.is_null() {
                out.push_str(&format!(" {label}=<null>"));
                continue;
            }
            let mut len = 0usize;
            while len < 512 && *p.add(len) != 0 {
                len += 1;
            }
            out.push_str(&format!(
                " {label}={}",
                String::from_utf8_lossy(core::slice::from_raw_parts(p, len))
            ));
        }
        log(&out);
    }
}

pub static QPC_FREQ: AtomicU64 = AtomicU64::new(0);
pub static NEXT_FRAME: AtomicU64 = AtomicU64::new(0);

pub unsafe fn qpc() -> u64 {
    unsafe {
        let mut v: i64 = 0;
        QueryPerformanceCounter(&mut v);
        v as u64
    }
}

pub unsafe fn qpc_freq() -> u64 {
    unsafe {
        let cached = QPC_FREQ.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        let mut f: i64 = 0;
        QueryPerformanceFrequency(&mut f);
        QPC_FREQ.store(f as u64, Ordering::Relaxed);
        f as u64
    }
}

/// Returns true the first time a given rectangle is seen, and false for every
/// repeat. The GL thread is the only caller, but the count is claimed with a
/// compare-exchange so a stray second thread can never corrupt the table.
pub fn viewport_is_new(key: u64) -> bool {
    loop {
        let n = VIEWPORT_COUNT.load(Ordering::Relaxed);
        if n >= MAX_VIEWPORTS {
            return false;
        }
        let mut seen = false;
        for slot in VIEWPORTS.iter().take(n) {
            if slot.load(Ordering::Relaxed) == key {
                seen = true;
                break;
            }
        }
        if seen {
            return false;
        }
        if VIEWPORT_COUNT
            .compare_exchange_weak(n, n + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            VIEWPORTS[n].store(key, Ordering::Relaxed);
            return true;
        }
    }
}

pub fn scaled_span(v: i32, s: f32) -> i32 {
    ((v as f32) * s).round() as i32
}

/// True when the engine is drawing to the default framebuffer, i.e. when the
/// logical -> physical mapping applies. Offscreen passes (shadow maps, render
/// targets) keep their own resolution.
pub fn on_main_framebuffer() -> bool {
    config().scale_fixes
        && DRAW_FBO.load(Ordering::Relaxed) == 0
        && PHYS_W.load(Ordering::Relaxed) > 0
}

/// When shadows are on the engine renders the 3D scene into a fixed subregion
/// of the logical canvas instead of the full canvas. The two observed viewports
/// are (768,336,1024,768) and (781,396,998,666). With shadows off a later
/// compositing pass fills the whole window, masking the subregion; with shadows
/// on that pass is skipped so the subregion is visible.
///
/// Fix: while the engine is drawing a 3D scene, promote such a large subregion
/// to the full logical canvas. Small UI viewports are left untouched.
///
/// The main menu uses the very same (768,336,1024,768) rectangle for its
/// background art and menu panel. That rectangle is a *designated area*, not a
/// scene that should fill the window: the engine draws it at 1:1 in logical
/// space and the logical -> physical mapping already scales it. Treating "the
/// window is maximized/borderless" as a reason to promote stretched the whole
/// menu across the screen. The only valid signal is whether the engine is
/// actually rendering a scene, i.e. whether it has bound an offscreen target.
pub fn promote_scene_viewport(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    if !config().promote_scene || !on_main_framebuffer() || !in_game_scene() {
        return (x, y, w, h);
    }
    let (lw, lh) = logical_size();
    // A scene viewport is large (covers >25% of logical in one axis, half in the
    // other) but not already the full canvas. Promote it to full.
    if (x != 0 || y != 0 || w != lw || h != lh) && w > lw / 4 && h > lh / 2 {
        if PROMO_LOG.fetch_add(1, Ordering::Relaxed) < 12 {
            log(&format!(
                "promote: ({x},{y},{w},{h}) -> ({lw},{lh}) fbo_age={}ms",
                fbo_age_ms()
            ));
        }
        return (0, 0, lw, lh);
    }
    (x, y, w, h)
}

/// Repair two viewport regressions seen after the canvas patch:
///
/// 1. A full-screen pass still uses the startup canvas (e.g. `(0,0,1920,1080)`
///    after a resize to 2560x1439). Expanding it to the current canvas removes
///    the black area outside the old size. This applies wherever the engine
///    still draws a full-screen pass at the old canvas - battle background
///    passes and the menu background alike - so it no longer waits for a 3D
///    scene: the engine binds no offscreen targets in menus, and the
///    `(0,0,startup_w,startup_h)` signature alone is precise.
/// 2. The battle scene is drawn at `y=0` after the canvas grows, instead of the
///    normal `y=155` (the scene height is already `canvas_h - 155`). Restoring
///    the inset removes the top black band. The same rewrite handles the
///    shrink-back direction: a scene still sized for the previous canvas is
///    rebuilt at the current one.
///
/// Both are config-gated so a bad guess can be turned off without a rebuild.
pub fn fix_layout_viewport(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let cfg = config();
    if DRAW_FBO.load(Ordering::Relaxed) != 0 {
        return (x, y, w, h);
    }
    let (lw, lh) = logical_size();
    let (sw, sh) = (
        STARTUP_LOGICAL_W.load(Ordering::Relaxed),
        STARTUP_LOGICAL_H.load(Ordering::Relaxed),
    );
    if sw <= 0 || sh <= 0 || lw <= 0 || lh <= 0 {
        return (x, y, w, h);
    }

    // Learn the scene's top inset from a battle frame drawn at the startup
    // canvas, before any patch: the first viewport of the shape
    // `(0,y,w,h-y)` with y>0 is the scene with its command bar reserved.
    // The engine's own constant is 155 on every known build, so the learned
    // value is a no-op in practice and the fallback stays correct.
    let inset = {
        let cfg_inset = config().scene_inset;
        let learned = SCENE_INSET.load(Ordering::Relaxed);
        if cfg_inset > 0 {
            cfg_inset
        } else if learned > 0 {
            learned
        } else if lw == sw && lh == sh && x == 0 && y > 0 && y * 4 <= lh && w == lw && h == lh - y {
            SCENE_INSET.store(y, Ordering::Relaxed);
            log(&format!("scene inset learned: {y}"));
            y
        } else {
            profile::scene_inset_default()
        }
    };

    // The engine asks for the battle scene at a wrong y after the canvas
    // changes - observed shapes: `(0,0,w,h-inset)` after F11, and
    // `(0,98,cw,ch-inset)` (stale battle-entry canvas + a y from an unknown
    // convention) after a mid-battle resize. Record the rect and let the
    // scene-rect repair handle any misplaced y; the matcher keys on
    // (w, h) = (canvas_w, canvas_h - inset) for ANY canvas the engine used.
    if cfg.scene_rect_fix && inset > 0 && x == 0 && w > 0 && h > 0 && is_scene_rect(w, h) {
        scene_vp_observed(x, y, w, h);
        // Diagnostic: capture the battle widget tree once per canvas epoch.
        schedule_widget_dump(DUMP_ON_BATTLE);
        // Normalise a battle screen that was built for a canvas narrower than
        // the HUD (negative flank widths mean "inherit parent" in the ctor).
        schedule_battle_relayout();
    }

    static STALE_LOG: AtomicU32 = AtomicU32::new(0);
    static SCENE_LOG: AtomicU32 = AtomicU32::new(0);
    let log_fix =
        |what: &str, counter: &AtomicU32, from: (i32, i32, i32, i32), to: (i32, i32, i32, i32)| {
            if counter.fetch_add(1, Ordering::Relaxed) < 24 {
                log(&format!(
                    "viewport fix: {what} ({},{},{},{}) -> ({},{},{},{})",
                    from.0, from.1, from.2, from.3, to.0, to.1, to.2, to.3
                ));
            }
        };

    // A stale full-screen pass at the startup canvas. No scene gate: the
    // engine binds no FBOs in menus (fbo_age grows unbounded there), and a
    // full-screen pass at a canvas size the engine is no longer configured
    // with is always stale.
    if cfg.stale_viewport_fix && x == 0 && y == 0 && w == sw && h == sh && (lw != sw || lh != sh) {
        let to = (0, 0, lw, lh);
        log_fix("stale full-screen", &STALE_LOG, (x, y, w, h), to);
        return to;
    }

    // Legacy fallback (scene_vp_rewrite, default off): rewriting the *drawn*
    // viewport shifts the picture without moving the engine's hit testing, so
    // clicks land 155 px off against 3D objects. The real fix is
    // `scene_rect_fix`, which repairs the engine's own rects.
    if cfg.scene_vp_rewrite
        && cfg.scene_rect_fix
        && inset > 0
        && x == 0
        && y == 0
        && w == lw
        && h == lh - inset
        && h > 0
    {
        let to = (0, inset, w, h);
        log_fix("scene top inset (vp)", &SCENE_LOG, (x, y, w, h), to);
        return to;
    }

    (x, y, w, h)
}

/// True while the engine is drawing a 3D scene. It binds an offscreen target
/// (the shadow map) every frame it renders one; the menu and the briefing
/// screens never do. The flag stays valid for two seconds so a frame that skips
/// the shadow pass cannot flicker the scene back into its subregion.
pub fn in_game_scene() -> bool {
    fbo_age_ms() < 2000
}

/// Milliseconds since the engine last bound an offscreen target, `u32::MAX` if
/// it never has.
pub fn fbo_age_ms() -> u32 {
    let last = FBO_LAST_TICK.load(Ordering::Relaxed);
    if last == 0 {
        return u32::MAX;
    }
    unsafe { GetTickCount() }.wrapping_sub(last)
}

// ------------------------------------------------------------- diagnostics

/// Viewports seen since the last "frame vps" log line. Temporary: this is what
/// tells us which rectangle the battle view is actually drawn into.
pub static FRAME_VPS: [AtomicU64; 8] = [const { AtomicU64::new(u64::MAX) }; 8];
pub static FRAME_VP_COUNT: AtomicUsize = AtomicUsize::new(0);
static PROMO_LOG: AtomicU32 = AtomicU32::new(0);
static LAST_SCENE_LOG: AtomicU32 = AtomicU32::new(0);

fn pack_rect(x: i32, y: i32, w: i32, h: i32) -> u64 {
    ((x as u16 as u64) << 48)
        | ((y as u16 as u64) << 32)
        | ((w as u16 as u64) << 16)
        | (h as u16 as u64)
}

#[unsafe(no_mangle)]
pub extern "system" fn glViewport(x: i32, y: i32, width: i32, height: i32) {
    let real = REAL_GL_VIEWPORT.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(i32, i32, i32, i32) = unsafe { core::mem::transmute(real) };
    let (ox, oy, ow, oh) = (x, y, width, height);
    // Record what the engine asked for, before any promotion, so the periodic
    // "frame vps" line shows the battle view's real layout.
    let n = FRAME_VP_COUNT.load(Ordering::Relaxed);
    if n < FRAME_VPS.len() {
        FRAME_VPS[n].store(pack_rect(ox, oy, ow, oh), Ordering::Relaxed);
        FRAME_VP_COUNT.store(n + 1, Ordering::Relaxed);
    }
    let (x, y, width, height) = fix_layout_viewport(ox, oy, ow, oh);
    let (x, y, width, height) = promote_scene_viewport(x, y, width, height);
    let on_main = on_main_framebuffer();
    let (dx, dy, dw, dh) = if on_main {
        let (sx, sy) = scale_xy();
        (
            OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
            OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
            scaled_span(width, sx).max(1),
            scaled_span(height, sy).max(1),
        )
    } else {
        (x, y, width, height)
    };
    unsafe { f(dx, dy, dw, dh) };
    if config().verbose
        && viewport_is_new(
            ((ox as u64 & 0xffff) << 48)
                | ((oy as u64 & 0xffff) << 32)
                | ((ow as u64 & 0xffff) << 16)
                | (oh as u64 & 0xffff),
        )
    {
        if (x, y, width, height) != (ox, oy, ow, oh) {
            log(&format!(
                "glViewport({ox},{oy},{ow},{oh}) promoted -> \
                 ({x},{y},{width},{height}) -> ({dx},{dy},{dw},{dh})"
            ));
        } else {
            log(&format!(
                "glViewport({ox},{oy},{ow},{oh}) -> ({dx},{dy},{dw},{dh})"
            ));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glScissor(x: i32, y: i32, width: i32, height: i32) {
    let real = REAL_GL_SCISSOR.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(i32, i32, i32, i32) = unsafe { core::mem::transmute(real) };
    let (x, y, width, height) = promote_scene_viewport(x, y, width, height);
    if on_main_framebuffer() {
        // Remember the logical rect so wglSwapBuffers can re-apply it each
        // frame after a resize (stale scaled scissor = black screen).
        LAST_SCISSOR_XY.store(
            ((x as u32 as u64) << 32) | (y as u32 as u64),
            Ordering::Relaxed,
        );
        LAST_SCISSOR_WH.store(
            ((width as u32 as u64) << 32) | (height as u32 as u64),
            Ordering::Relaxed,
        );
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
                OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
                scaled_span(width, sx).max(1),
                scaled_span(height, sy).max(1),
            )
        };
    } else {
        unsafe { f(x, y, width, height) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glRasterPos2i(x: i32, y: i32) {
    let real = REAL_GL_RASTER_POS2I.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(i32, i32) = unsafe { core::mem::transmute(real) };
    if on_main_framebuffer() {
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
                OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
            )
        };
    } else {
        unsafe { f(x, y) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glRasterPos3f(x: f32, y: f32, z: f32) {
    let real = REAL_GL_RASTER_POS3F.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(f32, f32, f32) = unsafe { core::mem::transmute(real) };
    if on_main_framebuffer() {
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                OFF_X.load(Ordering::Relaxed) as f32 + x * sx,
                OFF_Y.load(Ordering::Relaxed) as f32 + y * sy,
                z,
            )
        };
    } else {
        unsafe { f(x, y, z) };
    }
}

/// Reads from the framebuffer use physical pixels, so the source rectangle has
/// to be mapped as well. Everything else is forwarded untouched.
#[unsafe(no_mangle)]
pub extern "system" fn glReadPixels(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    format: u32,
    ty: u32,
    pixels: *mut c_void,
) {
    let real = REAL_GL_READ_PIXELS.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut c_void) =
        unsafe { core::mem::transmute(real) };
    let started = if perf::timing() {
        Some(unsafe { perf::now() })
    } else {
        None
    };
    // Screenshots read the back buffer during the frame, i.e. before our
    // swap-time overlay. Re-draw the mod overlay into it so screenshots and
    // recordings include the fan. Throttled: only large default-framebuffer
    // reads count, and at most five times a second.
    // Do NOT draw the mod overlay here. A readback can happen mid-frame, and
    // drawing then lands in a buffer the engine is still composing - that
    // produced ghost/back-buffer frames and was the only place the overlay ran
    // outside the swap point. The overlay is drawn once per frame, before the
    // swap, in `wglSwapBuffers`.
    if READ_FBO.load(Ordering::Relaxed) == 0 && is_scaled() && config().scale_fixes {
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
                OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
                scaled_span(width, sx).max(1),
                scaled_span(height, sy).max(1),
                format,
                ty,
                pixels,
            )
        };
    } else {
        unsafe { f(x, y, width, height, format, ty, pixels) };
    }
    perf::read_pixels();
    if let Some(t) = started {
        perf::read_pixels_ns(perf::ticks_to_ns(unsafe { perf::now() }.saturating_sub(t)));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glCopyTexSubImage2D(
    target: u32,
    level: i32,
    xoffset: i32,
    yoffset: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let real = REAL_GL_COPY_TEX_SUB_IMAGE2D.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, i32, i32) =
        unsafe { core::mem::transmute(real) };
    if READ_FBO.load(Ordering::Relaxed) == 0 && is_scaled() && config().scale_fixes {
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                target,
                level,
                xoffset,
                yoffset,
                OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
                OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
                scaled_span(width, sx).max(1),
                scaled_span(height, sy).max(1),
            )
        };
    } else {
        unsafe { f(target, level, xoffset, yoffset, x, y, width, height) };
    }
    perf::copy_tex();
}

#[unsafe(no_mangle)]
pub extern "system" fn glCopyTexImage2D(
    target: u32,
    level: i32,
    internal: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    border: i32,
) {
    let real = REAL_GL_COPY_TEX_IMAGE2D.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, i32, i32) =
        unsafe { core::mem::transmute(real) };
    if READ_FBO.load(Ordering::Relaxed) == 0 && is_scaled() && config().scale_fixes {
        let (sx, sy) = scale_xy();
        unsafe {
            f(
                target,
                level,
                internal,
                OFF_X.load(Ordering::Relaxed) + scaled_span(x, sx),
                OFF_Y.load(Ordering::Relaxed) + scaled_span(y, sy),
                scaled_span(width, sx).max(1),
                scaled_span(height, sy).max(1),
                border,
            )
        };
    } else {
        unsafe { f(target, level, internal, x, y, width, height, border) };
    }
    perf::copy_tex();
}

pub unsafe extern "system" fn hook_bind_framebuffer(target: u32, framebuffer: u32) {
    unsafe {
        match target {
            GL_FRAMEBUFFER => {
                DRAW_FBO.store(framebuffer, Ordering::Relaxed);
                READ_FBO.store(framebuffer, Ordering::Relaxed);
            }
            GL_DRAW_FRAMEBUFFER => DRAW_FBO.store(framebuffer, Ordering::Relaxed),
            GL_READ_FRAMEBUFFER => READ_FBO.store(framebuffer, Ordering::Relaxed),
            _ => {}
        }
        // A non-zero bind means an offscreen pass: the engine is drawing a 3D
        // scene. This is what the scene-viewport promotion keys off, so record when
        // it last happened. (The comment above used to claim this was diagnostic.)
        if framebuffer != 0 {
            FBO_LAST_TICK.store(GetTickCount(), Ordering::Relaxed);
        }
        let real = REAL_BIND_FRAMEBUFFER.load(Ordering::Relaxed);
        if real != 0 {
            let f: unsafe extern "system" fn(u32, u32) = core::mem::transmute(real);
            f(target, framebuffer);
        }
    }
}

// ------------------------------------------------------------ profiled calls
//
// These exports used to be generated jump thunks. They are small Rust wrappers
// now so the proxy can count the engine's display-list, state and upload traffic
// for `coke/` profiler mods. The real entry points are resolved in `init.rs`.

#[unsafe(no_mangle)]
pub extern "system" fn glCallList(list: u32) {
    let real = REAL_GL_CALL_LIST.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32) = unsafe { core::mem::transmute(real) };
    perf::call_list();
    let started = if perf::timing() {
        Some(unsafe { perf::now() })
    } else {
        None
    };
    unsafe { f(list) };
    if let Some(t) = started {
        perf::call_list_ns(perf::ticks_to_ns(unsafe { perf::now() }.saturating_sub(t)));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glBindTexture(target: u32, texture: u32) {
    let real = REAL_GL_BIND_TEXTURE.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, u32) = unsafe { core::mem::transmute(real) };
    perf::bind_texture();
    let started = if perf::timing() {
        Some(unsafe { perf::now() })
    } else {
        None
    };
    unsafe { f(target, texture) };
    if let Some(t) = started {
        perf::bind_texture_ns(perf::ticks_to_ns(unsafe { perf::now() }.saturating_sub(t)));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn glBegin(mode: u32) {
    let real = REAL_GL_BEGIN.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32) = unsafe { core::mem::transmute(real) };
    perf::begin_end();
    unsafe { f(mode) };
}

#[unsafe(no_mangle)]
pub extern "system" fn glEnd() {
    let real = REAL_GL_END.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { core::mem::transmute(real) };
    perf::begin_end();
    unsafe { f() };
}

#[unsafe(no_mangle)]
pub extern "system" fn glPushMatrix() {
    let real = REAL_GL_PUSH_MATRIX.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { core::mem::transmute(real) };
    perf::push_pop();
    unsafe { f() };
}

#[unsafe(no_mangle)]
pub extern "system" fn glPopMatrix() {
    let real = REAL_GL_POP_MATRIX.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { core::mem::transmute(real) };
    perf::push_pop();
    unsafe { f() };
}

#[unsafe(no_mangle)]
pub extern "system" fn glDrawArrays(mode: u32, first: i32, count: i32) {
    let real = REAL_GL_DRAW_ARRAYS.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, i32) = unsafe { core::mem::transmute(real) };
    perf::draw_arrays(count.max(0) as u32);
    unsafe { f(mode, first, count) };
}

#[unsafe(no_mangle)]
pub extern "system" fn glDrawElements(mode: u32, count: i32, ty: u32, indices: *const c_void) {
    let real = REAL_GL_DRAW_ELEMENTS.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, u32, *const c_void) =
        unsafe { core::mem::transmute(real) };
    perf::draw_elements(count.max(0) as u32);
    unsafe { f(mode, count, ty, indices) };
}

#[unsafe(no_mangle)]
pub extern "system" fn glTexImage2D(
    target: u32,
    level: i32,
    internal: i32,
    width: i32,
    height: i32,
    border: i32,
    format: u32,
    ty: u32,
    pixels: *const c_void,
) {
    let real = REAL_GL_TEX_IMAGE2D.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void) =
        unsafe { core::mem::transmute(real) };
    let bytes = if width > 0 && height > 0 {
        width as u64 * height as u64 * 4
    } else {
        0
    };
    perf::tex_upload(bytes);
    unsafe {
        f(
            target, level, internal, width, height, border, format, ty, pixels,
        )
    };
}

#[unsafe(no_mangle)]
pub extern "system" fn glTexSubImage2D(
    target: u32,
    level: i32,
    xoffset: i32,
    yoffset: i32,
    width: i32,
    height: i32,
    format: u32,
    ty: u32,
    pixels: *const c_void,
) {
    let real = REAL_GL_TEX_SUB_IMAGE2D.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void) =
        unsafe { core::mem::transmute(real) };
    let bytes = if width > 0 && height > 0 {
        width as u64 * height as u64 * 4
    } else {
        0
    };
    perf::tex_upload(bytes);
    unsafe {
        f(
            target, level, xoffset, yoffset, width, height, format, ty, pixels,
        )
    };
}

#[unsafe(no_mangle)]
pub extern "system" fn glNewList(list: u32, mode: u32) {
    let real = REAL_GL_NEW_LIST.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, u32) = unsafe { core::mem::transmute(real) };
    perf::list_new();
    unsafe { f(list, mode) };
}

#[unsafe(no_mangle)]
pub extern "system" fn glEndList() {
    let real = REAL_GL_END_LIST.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { core::mem::transmute(real) };
    unsafe { f() };
}

#[unsafe(no_mangle)]
pub extern "system" fn glGenLists(range: i32) -> u32 {
    let real = REAL_GL_GEN_LISTS.load(Ordering::Relaxed);
    if real == 0 {
        return 0;
    }
    let f: unsafe extern "system" fn(i32) -> u32 = unsafe { core::mem::transmute(real) };
    unsafe { f(range) }
}

#[unsafe(no_mangle)]
pub extern "system" fn glDeleteLists(list: u32, range: i32) {
    let real = REAL_GL_DELETE_LISTS.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(u32, i32) = unsafe { core::mem::transmute(real) };
    perf::list_deleted();
    unsafe { f(list, range) };
}

#[unsafe(no_mangle)]
pub extern "system" fn glCallLists(n: i32, ty: u32, lists: *const c_void) {
    let real = REAL_GL_CALL_LISTS.load(Ordering::Relaxed);
    if real == 0 {
        return;
    }
    let f: unsafe extern "system" fn(i32, u32, *const c_void) =
        unsafe { core::mem::transmute(real) };
    perf::list_called();
    unsafe { f(n, ty, lists) };
}

#[unsafe(no_mangle)]
pub extern "system" fn wglCreateContext(hdc: Hdc) -> Hglrc {
    let f: unsafe extern "system" fn(Hdc) -> Hglrc =
        unsafe { core::mem::transmute(REAL_WGL_CREATE_CONTEXT.load(Ordering::Relaxed)) };
    unsafe { f(hdc) }
}
#[unsafe(no_mangle)]
pub extern "system" fn wglMakeCurrent(hdc: Hdc, hglrc: Hglrc) -> i32 {
    let f: unsafe extern "system" fn(Hdc, Hglrc) -> i32 =
        unsafe { core::mem::transmute(REAL_WGL_MAKE_CURRENT.load(Ordering::Relaxed)) };
    let ok = unsafe { f(hdc, hglrc) };
    // wglMakeCurrent(hdc, NULL) succeeds but leaves no context current, so no
    // context-dependent work may run in that case.
    if ok != 0 && !hglrc.is_null() {
        if IDENTITY_LOGGED.swap(1, Ordering::Relaxed) == 0 {
            unsafe { log_gl_identity() };
        }
        // A fresh context starts with the default framebuffer bound.
        DRAW_FBO.store(0, Ordering::Relaxed);
        READ_FBO.store(0, Ordering::Relaxed);
        // The context captures its default viewport from the drawable at
        // creation time. If the window is going to end up a different size,
        // set that size now: the intro never calls glViewport again.
        let tw = TARGET_W.load(Ordering::Relaxed);
        let th = TARGET_H.load(Ordering::Relaxed);
        let vp = REAL_GL_VIEWPORT.load(Ordering::Relaxed);
        if tw > 0 && th > 0 && vp != 0 {
            let f: unsafe extern "system" fn(i32, i32, i32, i32) =
                unsafe { core::mem::transmute(vp) };
            unsafe { f(0, 0, tw, th) };
        }
        // glWindowPos2i is OpenGL 1.4: it is not in opengl32.dll's 1.1 export
        // table and only resolves through wglGetProcAddress once a context is
        // current. glGetBooleanv/glGetIntegerv are resolved in init().
        unsafe {
            if REAL_GL_WINDOW_POS.load(Ordering::Relaxed) == 0 {
                let wgpa: unsafe extern "system" fn(*const c_char) -> *mut c_void =
                    core::mem::transmute(REAL_WGL_GET_PROC_ADDRESS.load(Ordering::Relaxed));
                let p = wgpa(b"glWindowPos2i\0".as_ptr() as *const c_char);
                if !p.is_null() {
                    REAL_GL_WINDOW_POS.store(p as usize, Ordering::Relaxed);
                }
            }
        }
        // Swap interval is per-context state, so re-assert it on every context
        // switch. The engine resolves wglSwapIntervalEXT through our
        // wglGetProcAddress hook, which is where the real pointer comes from.
        unsafe { apply_vsync() };
    }
    ok
}

pub unsafe fn apply_vsync() {
    unsafe {
        let Some(want) = config().vsync else { return };
        let real = REAL_SWAP_INTERVAL.load(Ordering::Relaxed);
        if real == 0 {
            return;
        }
        let f: unsafe extern "system" fn(i32) -> i32 = core::mem::transmute(real);
        f(want);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn wglDeleteContext(hglrc: Hglrc) -> i32 {
    let f: unsafe extern "system" fn(Hglrc) -> i32 =
        unsafe { core::mem::transmute(REAL_WGL_DELETE_CONTEXT.load(Ordering::Relaxed)) };
    unsafe { f(hglrc) }
}

#[unsafe(no_mangle)]
pub extern "system" fn wglShareLists(a: Hglrc, b: Hglrc) -> i32 {
    let f: unsafe extern "system" fn(Hglrc, Hglrc) -> i32 =
        unsafe { core::mem::transmute(REAL_WGL_SHARE_LISTS.load(Ordering::Relaxed)) };
    unsafe { f(a, b) }
}

#[unsafe(no_mangle)]
pub extern "system" fn wglGetCurrentContext() -> Hglrc {
    let f: unsafe extern "system" fn() -> Hglrc =
        unsafe { core::mem::transmute(REAL_WGL_GET_CURRENT_CONTEXT.load(Ordering::Relaxed)) };
    unsafe { f() }
}

#[unsafe(no_mangle)]
pub extern "system" fn wglGetCurrentDC() -> Hdc {
    let f: unsafe extern "system" fn() -> Hdc =
        unsafe { core::mem::transmute(REAL_WGL_GET_CURRENT_DC.load(Ordering::Relaxed)) };
    unsafe { f() }
}

pub unsafe extern "system" fn hook_swap_interval(interval: i32) -> i32 {
    unsafe {
        let cfg = config().clone();
        if VSYNC_LOGGED.swap(1, Ordering::Relaxed) == 0 {
            log(&format!(
                "engine asked for wglSwapIntervalEXT({interval}); vsync={:?}",
                cfg.vsync
            ));
        }
        let want = cfg.vsync.unwrap_or(interval);
        let real = REAL_SWAP_INTERVAL.load(Ordering::Relaxed);
        if real == 0 {
            return 0;
        }
        let f: unsafe extern "system" fn(i32) -> i32 = core::mem::transmute(real);
        f(want)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn wglGetProcAddress(name: *const c_char) -> *mut c_void {
    let f: unsafe extern "system" fn(*const c_char) -> *mut c_void =
        unsafe { core::mem::transmute(REAL_WGL_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let p = unsafe { f(name) };
    if p.is_null() {
        return p;
    }

    // Always take the swap-interval entry point so the engine's own choice can
    // be logged, and overridden when the ini asks for it.
    if cstr_is(name, "wglSwapIntervalEXT") {
        REAL_SWAP_INTERVAL.store(p as usize, Ordering::Relaxed);
        return hook_swap_interval as *const () as *mut c_void;
    }
    // Framebuffer binding decides whether a viewport is the window or an
    // offscreen pass, so the mapping hooks need to see it.
    for alias in [
        "glBindFramebuffer",
        "glBindFramebufferEXT",
        "glBindFramebufferARB",
    ] {
        if cstr_is(name, alias) {
            REAL_BIND_FRAMEBUFFER.store(p as usize, Ordering::Relaxed);
            return hook_bind_framebuffer as *const () as *mut c_void;
        }
    }
    p
}

#[unsafe(no_mangle)]
pub extern "system" fn wglSwapBuffers(hdc: Hdc) -> i32 {
    unsafe { crate::range::install_from_swap() };
    let perf_entry = unsafe { perf::frame_begin() };
    let f: unsafe extern "system" fn(Hdc) -> i32 =
        unsafe { core::mem::transmute(REAL_WGL_SWAP_BUFFERS.load(Ordering::Relaxed)) };

    // If the window was resized since the last swap, the scissor rect the engine
    // set earlier this frame was scaled for the old size. Re-apply it now scaled
    // to the current physical size so it doesn't clip everything to black.
    unsafe {
        let sx_bits = SCALE_X.load(Ordering::Relaxed);
        let sy_bits = SCALE_Y.load(Ordering::Relaxed);
        let scale_changed = sx_bits != LAST_SWAP_SCALE_X.load(Ordering::Relaxed)
            || sy_bits != LAST_SWAP_SCALE_Y.load(Ordering::Relaxed);
        if scale_changed {
            LAST_SWAP_SCALE_X.store(sx_bits, Ordering::Relaxed);
            LAST_SWAP_SCALE_Y.store(sy_bits, Ordering::Relaxed);
            RESIZE_DIAG.store(8, Ordering::Relaxed);
            let sc_xy = LAST_SCISSOR_XY.load(Ordering::Relaxed);
            let sc_wh = LAST_SCISSOR_WH.load(Ordering::Relaxed);
            if sc_xy != u64::MAX && sc_wh != u64::MAX {
                let real_sc = REAL_GL_SCISSOR.load(Ordering::Relaxed);
                if real_sc != 0 {
                    let sc_x = (sc_xy >> 32) as i32;
                    let sc_y = (sc_xy & 0xffff_ffff) as i32;
                    let sc_w = (sc_wh >> 32) as i32;
                    let sc_h = (sc_wh & 0xffff_ffff) as i32;
                    let sx = f32::from_bits(sx_bits);
                    let sy = f32::from_bits(sy_bits);
                    let ox = OFF_X.load(Ordering::Relaxed);
                    let oy = OFF_Y.load(Ordering::Relaxed);
                    let f_sc: unsafe extern "system" fn(i32, i32, i32, i32) =
                        core::mem::transmute(real_sc);
                    f_sc(
                        ox + scaled_span(sc_x, sx),
                        oy + scaled_span(sc_y, sy),
                        scaled_span(sc_w, sx).max(1),
                        scaled_span(sc_h, sy).max(1),
                    );
                }
            }

            // The engine also assumes the default viewport still covers the
            // whole window: the intro, for example, never calls glViewport and
            // gets its viewport from context creation, i.e. the old fullscreen
            // size. Re-assert the full-window viewport for the next frame.
            let real_vp = REAL_GL_VIEWPORT.load(Ordering::Relaxed);
            let pw = PHYS_W.load(Ordering::Relaxed);
            let ph = PHYS_H.load(Ordering::Relaxed);
            if real_vp != 0 && pw > 0 && ph > 0 {
                let f_vp: unsafe extern "system" fn(i32, i32, i32, i32) =
                    core::mem::transmute(real_vp);
                f_vp(0, 0, pw, ph);
            }
        }
    }

    // Temporary diagnostic for the black-on-resize screens: dump the live
    // viewport/scissor the engine left behind for a few frames after a resize.
    unsafe {
        let left = RESIZE_DIAG.load(Ordering::Relaxed);
        if left > 0 {
            let geti = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
            let getb = REAL_GL_GET_BOOLEANV.load(Ordering::Relaxed);
            if geti != 0 {
                let g: unsafe extern "system" fn(u32, *mut i32) = core::mem::transmute(geti);
                let mut vp = [0i32; 4];
                let mut sb = [0i32; 4];
                g(0x0BA2, vp.as_mut_ptr()); // GL_VIEWPORT
                g(0x0C10, sb.as_mut_ptr()); // GL_SCISSOR_BOX
                let mut sc: u8 = 0;
                if getb != 0 {
                    let gb: unsafe extern "system" fn(u32, *mut u8) = core::mem::transmute(getb);
                    gb(0x0C11, &mut sc); // GL_SCISSOR_TEST
                }
                log(&format!(
                    "RESIZE state: vp=({},{},{},{}) scissor_test={} box=({},{},{},{}) \
                     phys={}x{} scale={:.4}x{:.4} fbo={}",
                    vp[0],
                    vp[1],
                    vp[2],
                    vp[3],
                    sc,
                    sb[0],
                    sb[1],
                    sb[2],
                    sb[3],
                    PHYS_W.load(Ordering::Relaxed),
                    PHYS_H.load(Ordering::Relaxed),
                    f32::from_bits(SCALE_X.load(Ordering::Relaxed)),
                    f32::from_bits(SCALE_Y.load(Ordering::Relaxed)),
                    DRAW_FBO.load(Ordering::Relaxed)
                ));
            }
            RESIZE_DIAG.store(left - 1, Ordering::Relaxed);
        }
    }

    // Loaded `coke/` mods get a chance to draw before the host overlay and the
    // real swap. Runs on the GL thread, so mods may call OpenGL via the host.
    unsafe {
        let pw = PHYS_W.load(Ordering::Relaxed);
        let ph = PHYS_H.load(Ordering::Relaxed);
        if pw > 0 && ph > 0 {
            coke_frame(pw, ph, FPS_CACHED.load(Ordering::Relaxed) as f32 / 10.0);
            coke_hud(pw, ph, FPS_CACHED.load(Ordering::Relaxed) as f32 / 10.0);
        }
    }

    // Draw FPS overlay onto the completed frame BEFORE swapping.
    unsafe {
        if fps_visible() {
            let fps10 = FPS_CACHED.load(Ordering::Relaxed);
            if fps10 > 0 {
                let pw = PHYS_W.load(Ordering::Relaxed);
                let ph = PHYS_H.load(Ordering::Relaxed);
                if pw > 0 && ph > 0 {
                    draw_fps_overlay(fps10, pw, ph);
                }
            }
        }
    }

    // Diagnostic: which rectangles did the engine draw this frame? Answers
    // "is the battle view the full canvas or its own subrectangle".
    unsafe {
        let now = GetTickCount();
        if now.wrapping_sub(LAST_SCENE_LOG.load(Ordering::Relaxed)) >= 5000 {
            LAST_SCENE_LOG.store(now, Ordering::Relaxed);
            let n = FRAME_VP_COUNT
                .swap(0, Ordering::Relaxed)
                .min(FRAME_VPS.len());
            if n > 0 {
                let mut s = String::from("frame vps:");
                for i in 0..n {
                    let k = FRAME_VPS[i].load(Ordering::Relaxed);
                    s.push_str(&format!(
                        " ({},{},{},{})",
                        ((k >> 48) & 0xffff) as u16 as i16,
                        ((k >> 32) & 0xffff) as u16 as i16,
                        ((k >> 16) & 0xffff) as u16 as i16,
                        (k & 0xffff) as u16 as i16
                    ));
                }
                log(&format!(
                    "{s} phys={}x{} logical={}x{} scale={:.4}x{:.4} mode={} fbo_age={}ms",
                    PHYS_W.load(Ordering::Relaxed),
                    PHYS_H.load(Ordering::Relaxed),
                    LOGICAL_W.load(Ordering::Relaxed),
                    LOGICAL_H.load(Ordering::Relaxed),
                    f32::from_bits(SCALE_X.load(Ordering::Relaxed)),
                    f32::from_bits(SCALE_Y.load(Ordering::Relaxed)),
                    WINDOW_MODE.load(Ordering::Relaxed),
                    fbo_age_ms()
                ));
            }
        }
    }

    let result = unsafe { f(hdc) };
    unsafe { perf::frame_end(perf_entry) };

    unsafe {
        SWAP_FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
        // The extension pointer is resolved after the first make-current, so a
        // second chance here keeps the forced setting from being missed.
        apply_vsync();

        let freq = qpc_freq();
        if freq > 0 {
            let now = qpc();
            let start = FPS_START.load(Ordering::Relaxed);
            if start == 0 {
                FPS_START.store(now, Ordering::Relaxed);
            } else {
                let frames = FPS_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
                let elapsed = now.saturating_sub(start);
                if elapsed >= freq * 2 {
                    let fps10 =
                        (frames as f64 * freq as f64 / elapsed as f64 * 10.0).round() as u32;
                    FPS_CACHED.store(fps10, Ordering::Relaxed);
                    FPS_START.store(now, Ordering::Relaxed);
                    FPS_FRAMES.store(0, Ordering::Relaxed);
                    if config().verbose {
                        let fps = fps10 as f64 / 10.0;
                        log(&format!("fps: {fps:.1}"));
                    }
                }
            }
        }

        // Frame cap: sleep out the rest of the frame period. The engine runs
        // uncapped otherwise (several hundred fps), burning CPU and GPU for
        // frames nobody sees.
        let limit = config().limit_fps;
        if limit > 0 && config().vsync.is_none() && freq > 0 {
            let period = freq / limit as u64;
            let now = qpc();
            let mut next = NEXT_FRAME.load(Ordering::Relaxed);
            // Resync only after a real stall (loading, alt-tab pause). Ordinary
            // sleep overshoot is *not* discarded, so the schedule catches up
            // and the long-run rate stays at exactly `limit_fps`.
            if next == 0 || now.saturating_sub(next) > 2 * period {
                next = now;
            }
            let target = next + period;
            NEXT_FRAME.store(target, Ordering::Relaxed);
            let delta = target.saturating_sub(now);
            let ms = (delta * 1000 / freq) as u32;
            if ms > 0 {
                Sleep(ms);
            }
        }
    }
    result
}
