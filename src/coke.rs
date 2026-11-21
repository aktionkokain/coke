//! `coke/` native mod loader.
//!
//! The proxy is loaded before the engine and runs in-process, so it can host
//! small native DLLs without touching `CM Black Sea.exe`. The contract is
//! deliberately paranoid:
//!
//! * `coke/` is created next to the game executable.
//! * only top-level `coke/*.dll` files are considered;
//! * a DLL is loaded only when `coke/user_consent_<dll-stem>.txt` exists.
//!
//! A mod may export:
//!   `coke_init(*const CokeHost) -> i32`  (optional; 0 = decline/abort)
//!   `coke_frame(i32 width, i32 height, f32 fps)` (optional; called before swap)
//!
//! The host API is intentionally small for now. It can grow without breaking
//! old mods by bumping `COKE_HOST_API` and adding fields at the end.

use crate::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::ffi::{CStr, c_char};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static CAMERA_WHEEL_NOTCHES: AtomicI32 = AtomicI32::new(0);
const MAX_MOD_BUFFER: usize = 4 * 1024 * 1024;

/// Validate a native mod's output buffer before turning it into a Rust slice.
/// The ABI is intentionally unsafe, but a bad plugin must not make slice
/// construction itself invoke undefined behavior.
unsafe fn valid_mod_buffer(
    ptr: *const c_void,
    len: usize,
    writable: bool,
    alignment: usize,
) -> bool {
    unsafe {
        if ptr.is_null() || len == 0 || len > MAX_MOD_BUFFER {
            return false;
        }
        let start = ptr as usize;
        if !start.is_multiple_of(alignment) {
            return false;
        }
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        let mut cursor = start;
        while cursor < end {
            let mut info: MemBasicInfo = core::mem::zeroed();
            if VirtualQuery(
                cursor as *const c_void,
                &mut info,
                core::mem::size_of::<MemBasicInfo>(),
            ) == 0
                || info.state != MEM_COMMIT
                || info.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            {
                return false;
            }
            if writable
                && !matches!(
                    info.protect & 0xff,
                    PAGE_READWRITE
                        | PAGE_WRITECOPY
                        | PAGE_EXECUTE_READWRITE
                        | PAGE_EXECUTE_WRITECOPY
                )
            {
                return false;
            }
            let Some(region_end) = (info.base_address as usize).checked_add(info.region_size)
            else {
                return false;
            };
            if region_end <= cursor {
                return false;
            }
            cursor = region_end.min(end);
        }
        true
    }
}

// The ABI types live in `crates/coke-host` so the proxy and every mod share
// one definition. Re-exported here because the rest of the crate reaches them
// through `crate::*`.
pub use coke_host::{
    COKE_HOST_API, CokeCamera, CokeCameraCommand, CokeHost, CokeTerrain, WorldVertex,
};

unsafe extern "system" fn host_log(msg: *const c_char) {
    unsafe {
        if msg.is_null() {
            return;
        }
        let text = CStr::from_ptr(msg).to_string_lossy();
        log(&format!("coke: {text}"));
    }
}

unsafe extern "system" fn host_draw_text(x: i32, y: i32, text: *const c_char, rgb: u32) {
    unsafe {
        if text.is_null() {
            return;
        }
        let w = PHYS_W.load(Ordering::Relaxed);
        let h = PHYS_H.load(Ordering::Relaxed);
        if w <= 0 || h <= 0 {
            return;
        }
        let text = CStr::from_ptr(text).to_string_lossy();
        draw_text_overlay(x, y, &text, rgb, w, h);
    }
}

unsafe extern "system" fn host_get_width() -> i32 {
    PHYS_W.load(Ordering::Relaxed)
}

unsafe extern "system" fn host_get_height() -> i32 {
    PHYS_H.load(Ordering::Relaxed)
}

unsafe extern "system" fn host_get_fps() -> f32 {
    FPS_CACHED.load(Ordering::Relaxed) as f32 / 10.0
}

unsafe extern "system" fn host_read_u32(addr: u32) -> u32 {
    unsafe { rd_u32(addr as usize).unwrap_or(0) }
}

unsafe extern "system" fn host_read_f32(addr: u32) -> f32 {
    unsafe { rd_f32(addr as usize).unwrap_or(f32::NAN) }
}

unsafe extern "system" fn host_read_bytes(addr: u32, out: *mut u8, len: u32) -> i32 {
    unsafe {
        if !valid_mod_buffer(out.cast(), len as usize, true, core::mem::align_of::<u8>())
            || len > 16 * 1024 * 1024
        {
            return 0;
        }
        let buf = core::slice::from_raw_parts_mut(out, len as usize);
        rd_bytes(addr as usize, buf) as i32
    }
}

unsafe extern "system" fn host_module_base() -> u32 {
    unsafe { GetModuleHandleW(core::ptr::null()) as u32 }
}

unsafe extern "system" fn host_module_size() -> u32 {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            0
        } else {
            module_image_size(base) as u32
        }
    }
}

unsafe extern "system" fn host_class_name(vt: u32, out: *mut c_char, len: u32) -> i32 {
    unsafe {
        if out.is_null() || len == 0 {
            return 0;
        }
        let Some(name) = class_name_of_vtable(vt as usize) else {
            return 0;
        };
        let bytes = name.as_bytes();
        let n = bytes.len().min(len as usize - 1);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, n);
        *out.add(n) = 0;
        n as i32
    }
}

unsafe extern "system" fn host_scene_rect(out: *mut i32) -> i32 {
    unsafe {
        if out.is_null() {
            return 0;
        }
        match scene_rect() {
            Some(r) => {
                core::ptr::copy_nonoverlapping(r.as_ptr(), out, 4);
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_camera(out: *mut CokeCamera) -> i32 {
    unsafe {
        if out.is_null() {
            return 0;
        }
        let Some(c) = camera() else { return 0 };
        *out = CokeCamera {
            pos: [c.pos.x, c.pos.y, c.pos.z],
            dir: [c.dir.x, c.dir.y, c.dir.z],
            up: [c.up.x, c.up.y, c.up.z],
            fov: c.fov,
            aspect: c.aspect,
            near: c.near,
            far: c.far,
        };
        1
    }
}

unsafe extern "system" fn host_world_to_screen(
    x: f32,
    y: f32,
    z: f32,
    sx: *mut f32,
    sy: *mut f32,
) -> i32 {
    unsafe {
        if sx.is_null() || sy.is_null() {
            return 0;
        }
        let Some(cam) = stable_camera() else { return 0 };
        let pw = PHYS_W.load(Ordering::Relaxed).max(1);
        let ph = PHYS_H.load(Ordering::Relaxed).max(1);
        let rect = overlay_rect(pw, ph);
        match project(&cam, rect, Vec3 { x, y, z }) {
            Some((px, py)) => {
                *sx = px;
                *sy = py;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_cursor_world(x: *mut f32, y: *mut f32, z: *mut f32) -> i32 {
    unsafe {
        if x.is_null() || y.is_null() || z.is_null() {
            return 0;
        }
        match cursor_world() {
            Some(p) => {
                *x = p.x;
                *y = p.y;
                *z = p.z;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_terrain(out: *mut CokeTerrain) -> i32 {
    unsafe {
        if out.is_null() {
            return 0;
        }
        let Some(t) = terrain() else { return 0 };
        *out = CokeTerrain {
            map: t.map as u32,
            grid_w: t.grid_w,
            grid_h: t.grid_h,
            world_w: t.world_w,
            world_h: t.world_h,
            heights: t.height_ptr as u32,
        };
        1
    }
}

unsafe extern "system" fn host_terrain_height(x: f32, z: f32) -> f32 {
    unsafe {
        match terrain() {
            Some(t) => terrain_height(&t, x, z),
            None => 0.0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn host_los_scan(
    ox: f32,
    oz: f32,
    eye: f32,
    range: f32,
    cell: f32,
    target_h: f32,
    vis: *mut u8,
    vis_len: u32,
    heights: *mut f32,
    heights_len: u32,
    nx: *mut i32,
    nz: *mut i32,
) -> i32 {
    unsafe {
        if vis.is_null() || heights.is_null() || nx.is_null() || nz.is_null() {
            return 0;
        }
        if !valid_mod_buffer(
            vis.cast(),
            vis_len as usize,
            true,
            core::mem::align_of::<u8>(),
        ) || !valid_mod_buffer(
            heights.cast(),
            heights_len as usize,
            true,
            core::mem::align_of::<f32>(),
        ) {
            return 0;
        }
        let v = core::slice::from_raw_parts_mut(vis, vis_len as usize);
        let h = core::slice::from_raw_parts_mut(heights, heights_len as usize);
        match los_scan(ox, oz, eye, range, cell, target_h, v, h) {
            Some(info) => {
                *nx = info.nx;
                *nz = info.nz;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_los_segment(
    ax: f32,
    az: f32,
    ah: f32,
    bx: f32,
    bz: f32,
    target_h: f32,
) -> i32 {
    unsafe {
        let Some(t) = terrain() else { return 0 };
        // Engine coords: x east, y north, z height.
        let a = Vec3 {
            x: ax,
            y: az,
            z: ah,
        };
        let b = Vec3 {
            x: bx,
            y: bz,
            z: terrain_height(&t, bx, bz),
        };
        los_segment(a, b, target_h) as i32
    }
}

unsafe extern "system" fn host_selected_unit() -> u32 {
    unsafe { selected_unit().unwrap_or(0) as u32 }
}

unsafe extern "system" fn host_unit_pos(unit: u32, x: *mut f32, y: *mut f32, z: *mut f32) -> i32 {
    unsafe {
        if x.is_null() || y.is_null() || z.is_null() {
            return 0;
        }
        match unit_pos(unit as usize) {
            Some(p) => {
                *x = p.x;
                *y = p.y;
                *z = p.z;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_draw_world_triangles(verts: *const WorldVertex, count: u32) {
    unsafe {
        if verts.is_null() || count == 0 || count > 1_000_000 || !count.is_multiple_of(3) {
            return;
        }
        let v = core::slice::from_raw_parts(verts, count as usize);
        draw_world_triangles(v);
    }
}

unsafe extern "system" fn host_key_down(vk: i32) -> i32 {
    unsafe { (GetAsyncKeyState(vk) < 0) as i32 }
}

unsafe extern "system" fn host_focused() -> i32 {
    unsafe {
        // Accept any window owned by this process, not just the cached main handle.
        // The engine destroys and recreates its window during startup, so comparing
        // against `MAIN_WINDOW` could silently disable every mod hotkey; the process
        // id stays the same no matter which window (or borderless mode) is active.
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return 0;
        }
        if IsIconic(fg) != 0 {
            return 0;
        }
        let mut pid = 0u32;
        if GetWindowThreadProcessId(fg, &mut pid) == 0 {
            return 0;
        }
        (pid == GetCurrentProcessId()) as i32
    }
}

unsafe extern "system" fn host_debug_dump() {
    unsafe {
        debug_dump();
    }
}

unsafe extern "system" fn host_time_ms() -> u32 {
    unsafe { GetTickCount() }
}

unsafe extern "system" fn host_draw_world_lines(verts: *const WorldVertex, count: u32, width: f32) {
    unsafe {
        if verts.is_null() || count < 2 || !count.is_multiple_of(2) || count > 1_000_000 {
            return;
        }
        let v = core::slice::from_raw_parts(verts, count as usize);
        draw_world_lines(v, width);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn host_los_scan_map(
    ox: f32,
    oz: f32,
    eye: f32,
    cell: f32,
    vis: *mut u8,
    vis_len: u32,
    heights: *mut f32,
    heights_len: u32,
    nx: *mut i32,
    nz: *mut i32,
) -> i32 {
    unsafe {
        if vis.is_null() || heights.is_null() || nx.is_null() || nz.is_null() {
            return 0;
        }
        if !valid_mod_buffer(
            vis.cast(),
            vis_len as usize,
            true,
            core::mem::align_of::<u8>(),
        ) || !valid_mod_buffer(
            heights.cast(),
            heights_len as usize,
            true,
            core::mem::align_of::<f32>(),
        ) {
            return 0;
        }
        let v = core::slice::from_raw_parts_mut(vis, vis_len as usize);
        let h = core::slice::from_raw_parts_mut(heights, heights_len as usize);
        match los_scan_map(ox, oz, eye, cell, v, h) {
            Some((a, b)) => {
                *nx = a;
                *nz = b;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_cursor_ground(x: *mut f32, y: *mut f32, z: *mut f32) -> i32 {
    unsafe {
        if x.is_null() || y.is_null() || z.is_null() {
            return 0;
        }
        match cursor_ground() {
            Some(p) => {
                *x = p.x;
                *y = p.y;
                *z = p.z;
                1
            }
            None => 0,
        }
    }
}

unsafe extern "system" fn host_camera_apply(cmd: *const CokeCameraCommand) -> i32 {
    unsafe {
        if cmd.is_null() {
            return 0;
        }
        camera_apply_command(&*cmd) as i32
    }
}

unsafe extern "system" fn host_camera_wheel_delta() -> f32 {
    CAMERA_WHEEL_NOTCHES.swap(0, Ordering::Relaxed) as f32
}

unsafe extern "system" fn host_camera_input_capture(captured: i32) -> i32 {
    // Bit 0 claims native camera input; bit 1 enables free flight and R/F
    // suppression; bit 2 disables terrain clearance in free-camera mode.
    set_camera_input_capture(captured);
    1
}

/// Record wheel input for camera mods before the native window procedure
/// consumes it. Mods decide whether to use the delta; the host never changes
/// native wheel behavior on their behalf.
pub fn coke_note_wheel(notches: i32) {
    CAMERA_WHEEL_NOTCHES.fetch_add(notches.clamp(-120, 120), Ordering::Relaxed);
}

/// Set while a mod's `coke_init` runs, so `set_layer` knows which mod it means.
static INIT_ACTIVE: AtomicU32 = AtomicU32::new(0);
static PENDING_LAYER: AtomicI32 = AtomicI32::new(0);

unsafe extern "system" fn host_register_hotkey(
    vk: i32,
    cb: Option<unsafe extern "system" fn()>,
) -> i32 {
    match cb {
        Some(cb) => hotkeys::register(vk, cb) as i32,
        None => 0,
    }
}

unsafe extern "system" fn host_set_layer(layer: i32) {
    if INIT_ACTIVE.load(Ordering::Relaxed) == 0 {
        log("coke: set_layer ignored (call it from coke_init)");
        return;
    }
    PENDING_LAYER.store(layer, Ordering::Relaxed);
}

static COKE_HOST: CokeHost = CokeHost {
    api_version: COKE_HOST_API,
    log: Some(host_log),
    draw_text: Some(host_draw_text),
    get_width: Some(host_get_width),
    get_height: Some(host_get_height),
    get_fps: Some(host_get_fps),
    size: core::mem::size_of::<CokeHost>() as u32,
    read_u32: Some(host_read_u32),
    read_f32: Some(host_read_f32),
    read_bytes: Some(host_read_bytes),
    module_base: Some(host_module_base),
    module_size: Some(host_module_size),
    class_name: Some(host_class_name),
    scene_rect: Some(host_scene_rect),
    camera: Some(host_camera),
    world_to_screen: Some(host_world_to_screen),
    cursor_world: Some(host_cursor_world),
    terrain: Some(host_terrain),
    terrain_height: Some(host_terrain_height),
    los_scan: Some(host_los_scan),
    los_segment: Some(host_los_segment),
    selected_unit: Some(host_selected_unit),
    unit_pos: Some(host_unit_pos),
    draw_world_triangles: Some(host_draw_world_triangles),
    key_down: Some(host_key_down),
    focused: Some(host_focused),
    debug_dump: Some(host_debug_dump),
    time_ms: Some(host_time_ms),
    draw_world_lines: Some(host_draw_world_lines),
    cursor_ground: Some(host_cursor_ground),
    los_scan_map: Some(host_los_scan_map),
    register_hotkey: Some(host_register_hotkey),
    set_layer: Some(host_set_layer),
    camera_apply: Some(host_camera_apply),
    camera_wheel_delta: Some(host_camera_wheel_delta),
    camera_input_capture: Some(host_camera_input_capture),
};

type CokeFrameFn = unsafe extern "system" fn(i32, i32, f32);
type CokeHudFn = unsafe extern "system" fn(i32, i32, f32);
type CokeTickFn = unsafe extern "system" fn();
type CokeInitFn = unsafe extern "system" fn(*const CokeHost) -> i32;

struct CokeMod {
    /// Keep the module loaded for the life of the process. `usize` keeps the
    /// static list `Send`.
    _lib: usize,
    frame: Option<CokeFrameFn>,
    /// API v3: called after every mod's `coke_frame`, when all world-space
    /// overlays are finished. Text/HUD drawing belongs here so a world overlay
    /// cannot cover it.
    hud: Option<CokeHudFn>,
    tick: Option<CokeTickFn>,
    /// API v4: requested draw layer. Higher draws later (on top).
    layer: i32,
    /// Deterministic tie-breaker: the sorted load order.
    order: u32,
}

/// Sort a mod list by the documented draw-order rule: `(layer, load order)`.
/// Mods that never call `set_layer` all have layer 0 and keep their alphabetical
/// load order, so the default stays deterministic.
pub(crate) fn sort_by_draw_order<T, F: Fn(&T) -> (i32, u32)>(items: &mut [T], key: F) {
    items.sort_by_key(key);
}

static COKE_MODS: OnceLock<Mutex<Vec<CokeMod>>> = OnceLock::new();
static COKE_LOADED: AtomicU32 = AtomicU32::new(0);

fn mods() -> &'static Mutex<Vec<CokeMod>> {
    COKE_MODS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Create `coke/` and load every consent-gated DLL in it. Called once from the
/// watcher thread (never from `DllMain`, where `LoadLibrary` can deadlock under
/// the loader lock).
pub unsafe fn load_coke_mods() {
    unsafe {
        if COKE_LOADED.swap(1, Ordering::Relaxed) != 0 {
            return;
        }

        let dir = exe_dir().join(MOD_DIR);
        let dir_w = wide(&dir.to_string_lossy());
        CreateDirectoryW(dir_w.as_ptr(), core::ptr::null_mut());
        log(&format!("coke: folder {}", dir.display()));

        let pattern = wide(&dir.join("*.dll").to_string_lossy());
        let mut find: Win32FindDataW = core::mem::zeroed();
        let handle = FindFirstFileW(pattern.as_ptr(), &mut find);
        if handle == (-1isize) as Handle {
            log("coke: no mod DLLs found");
            return;
        }

        let mut names: Vec<String> = Vec::new();
        loop {
            let is_dir = find.dw_file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            if !is_dir {
                names.push(str_from_wide(find.c_file_name.as_ptr()));
            }
            if FindNextFileW(handle, &mut find) == 0 {
                break;
            }
        }
        FindClose(handle);
        // Deterministic default draw order: sort by name instead of relying on
        // the order the filesystem happened to return.
        names.sort_by_key(|n| n.to_ascii_lowercase());

        let mut loaded = 0usize;
        for (order, name) in names.into_iter().enumerate() {
            let path = dir.join(&name);
            let stem = Path::new(&name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let consent = dir.join(consent_file(&stem));

            if stem.is_empty() || !consent.is_file() {
                log(&format!(
                    "coke: {name} skipped (missing {})",
                    consent
                        .file_name()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default()
                ));
            } else {
                let path_w = wide(&path.to_string_lossy());
                let lib = LoadLibraryExW(
                    path_w.as_ptr(),
                    core::ptr::null_mut(),
                    LOAD_WITH_ALTERED_SEARCH_PATH,
                );
                if lib.is_null() {
                    log(&format!(
                        "coke: {name} failed to load (error {})",
                        GetLastError()
                    ));
                } else {
                    let init_ptr = resolve(lib, "coke_init");
                    let mut accepted = true;
                    let mut layer = 0i32;
                    if !init_ptr.is_null() {
                        let init: CokeInitFn = core::mem::transmute(init_ptr);
                        INIT_ACTIVE.store(1, Ordering::Relaxed);
                        PENDING_LAYER.store(0, Ordering::Relaxed);
                        accepted = init(&COKE_HOST) != 0;
                        INIT_ACTIVE.store(0, Ordering::Relaxed);
                        layer = PENDING_LAYER.load(Ordering::Relaxed);
                        if !accepted {
                            log(&format!("coke: {name} declined initialization"));
                        }
                    }
                    if accepted {
                        let frame_ptr = resolve(lib, "coke_frame");
                        let frame: Option<CokeFrameFn> = if frame_ptr.is_null() {
                            None
                        } else {
                            Some(core::mem::transmute::<*mut c_void, CokeFrameFn>(frame_ptr))
                        };
                        let hud_ptr = resolve(lib, "coke_hud");
                        let hud: Option<CokeHudFn> = if hud_ptr.is_null() {
                            None
                        } else {
                            Some(core::mem::transmute::<*mut c_void, CokeHudFn>(hud_ptr))
                        };
                        let tick_ptr = resolve(lib, "coke_tick");
                        let tick: Option<CokeTickFn> = if tick_ptr.is_null() {
                            None
                        } else {
                            Some(core::mem::transmute::<*mut c_void, CokeTickFn>(tick_ptr))
                        };
                        {
                            let mut guard = mods().lock().unwrap();
                            guard.push(CokeMod {
                                _lib: lib as usize,
                                frame,
                                hud,
                                tick,
                                layer,
                                order: order as u32,
                            });
                            sort_by_draw_order(&mut guard, |m| (m.layer, m.order));
                        }
                        loaded += 1;
                        log(&format!(
                            "coke: loaded {name} (frame={} hud={} tick={} layer={layer})",
                            if frame.is_some() { "yes" } else { "no" },
                            if hud.is_some() { "yes" } else { "no" },
                            if tick.is_some() { "yes" } else { "no" }
                        ));
                    } else {
                        FreeLibrary(lib);
                    }
                }
            }
        }
        log(&format!("coke: {loaded} mod(s) loaded"));
        if loaded > 0 {
            start_mod_tick_thread();
        }
    }
}

/// Call every loaded mod's `coke_frame` hook. Runs on the GL thread inside
/// `wglSwapBuffers`; `try_lock` keeps a stuck mod from stalling presentation.
/// Iteration follows the draw-order rule `(layer, load order)`, so overlapping
/// world-space drawing respects a mod's requested layer.
pub unsafe fn coke_frame(width: i32, height: i32, fps: f32) {
    unsafe {
        if COKE_LOADED.load(Ordering::Relaxed) == 0 {
            return;
        }
        if let Ok(guard) = mods().try_lock() {
            for m in guard.iter() {
                if let Some(frame) = m.frame {
                    frame(width, height, fps);
                }
            }
        }
    }
}

/// Call every loaded mod's `coke_hud` hook. Runs on the GL thread after all
/// `coke_frame` (world-space) drawing is complete, so 2D text cannot be covered
/// by a later world overlay. The host's own FPS overlay is drawn after this.
/// Iteration uses the same `(layer, load order)` rule as `coke_frame`.
pub unsafe fn coke_hud(width: i32, height: i32, fps: f32) {
    unsafe {
        if COKE_LOADED.load(Ordering::Relaxed) == 0 {
            return;
        }
        if let Ok(guard) = mods().try_lock() {
            for m in guard.iter() {
                if let Some(hud) = m.hud {
                    hud(width, height, fps);
                }
            }
        }
    }
}

/// Call every loaded mod's `coke_tick` hook. Runs on the dedicated worker
/// thread, never on the GL thread, so data scans cannot stall rendering.
pub unsafe fn coke_tick() {
    unsafe {
        if COKE_LOADED.load(Ordering::Relaxed) == 0 {
            return;
        }
        if let Ok(guard) = mods().try_lock() {
            for m in guard.iter() {
                if let Some(tick) = m.tick {
                    tick();
                }
            }
        }
    }
}

/// Pick a CPU the game's main thread is not using and pin this worker to it.
unsafe fn pin_worker_to_own_core() {
    unsafe {
        let mut pmask = 0usize;
        let mut smask = 0usize;
        if GetProcessAffinityMask(GetCurrentProcess(), &mut pmask, &mut smask) == 0 || pmask == 0 {
            pmask = 1;
        }
        let mut cpu = pmask.trailing_zeros() as usize;
        // Prefer the highest-numbered allowed CPU (usually the least used by a
        // game that starts its streams on the low cores).
        if pmask.count_ones() > 1 {
            for i in (0..usize::BITS).rev() {
                if pmask & (1usize << i) != 0 {
                    cpu = i as usize;
                    break;
                }
            }
            let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
            if !hwnd.is_null() {
                let mut pid = 0u32;
                let tid = GetWindowThreadProcessId(hwnd, &mut pid);
                let th = OpenThread(THREAD_QUERY_INFORMATION, 0, tid);
                if !th.is_null() {
                    // GetThreadIdealProcessor is not in the 32-bit import library;
                    // resolve it dynamically and fall back to the core choice above.
                    let k32 = LoadLibraryW(wide("kernel32.dll").as_ptr());
                    let p = if k32.is_null() {
                        core::ptr::null_mut()
                    } else {
                        GetProcAddress(k32, b"GetThreadIdealProcessor\0".as_ptr() as *const c_char)
                    };
                    if !p.is_null() {
                        let f: unsafe extern "system" fn(Handle) -> u32 = core::mem::transmute(p);
                        let ideal = f(th);
                        if ideal < usize::BITS && ideal as usize == cpu {
                            for i in 0..usize::BITS {
                                if pmask & (1usize << i) != 0 && i as usize != cpu {
                                    cpu = i as usize;
                                    break;
                                }
                            }
                        }
                    }
                    CloseHandle(th);
                }
            }
        }
        SetThreadAffinityMask(GetCurrentThread(), 1usize << cpu);
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        log(&format!(
            "coke: worker pinned to cpu {cpu} (process mask 0x{pmask:x}), priority below normal"
        ));
    }
}

extern "system" fn mod_tick_thread(_param: *mut c_void) -> u32 {
    unsafe {
        // The engine window appears a few seconds after startup; pin once it
        // exists so the chosen core can avoid the main thread's ideal processor.
        let t0 = GetTickCount();
        while MAIN_WINDOW.load(Ordering::Relaxed) == 0 && GetTickCount().wrapping_sub(t0) < 15_000 {
            Sleep(50);
        }
        pin_worker_to_own_core();
        loop {
            coke_tick();
            Sleep(10);
        }
    }
}

pub unsafe fn start_mod_tick_thread() {
    unsafe {
        let mut tid = 0u32;
        CreateThread(
            core::ptr::null_mut(),
            256 * 1024,
            mod_tick_thread,
            core::ptr::null_mut(),
            0,
            &mut tid,
        );
        log(&format!("coke: worker thread {tid}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_order_is_layer_then_load_order() {
        // (layer, load order) pairs as they would come from the loader.
        let mut mods = vec![(0, 2), (5, 0), (0, 0), (5, 1), (-1, 0), (0, 1)];
        sort_by_draw_order(&mut mods, |m| *m);
        assert_eq!(
            mods,
            vec![(-1, 0), (0, 0), (0, 1), (0, 2), (5, 0), (5, 1)],
            "higher layers draw later and equal layers keep load order"
        );
    }

    #[test]
    fn host_api_reports_v6() {
        assert_eq!(COKE_HOST.api_version, 6);
    }
}
