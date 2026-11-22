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
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::cell::RefCell;
use std::ffi::{CStr, c_char};
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static CAMERA_WHEEL_NOTCHES: AtomicI32 = AtomicI32::new(0);
const MAX_READ_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOS_ELEMENTS: usize = 4_000_000;
const MAX_DRAW_ELEMENTS: usize = 1_000_000;

/// Native callers own allocation, lifetime and exclusive access to outputs.
pub(crate) unsafe fn mod_span<T>(
    ptr: *const T,
    count: usize,
    writable: bool,
) -> Option<Range<usize>> {
    let len = count.checked_mul(size_of::<T>())?;
    let start = ptr as usize;
    let end = start.checked_add(len)?;
    if ptr.is_null() || len == 0 || len > isize::MAX as usize || !ptr.is_aligned() {
        return None;
    }
    unsafe {
        let mut cursor = start;
        let mut allocation = None;
        while cursor < end {
            let mut info: MemBasicInfo = core::mem::zeroed();
            if VirtualQuery(
                cursor as *const c_void,
                &mut info,
                size_of::<MemBasicInfo>(),
            ) == 0
                || info.state != MEM_COMMIT
                || info.protect & PAGE_GUARD != 0
                || *allocation.get_or_insert(info.allocation_base) != info.allocation_base
            {
                return None;
            }
            let access = info.protect & 0xff;
            if !(matches!(access, 0x04 | 0x08 | 0x40 | 0x80)
                || !writable && matches!(access, 0x02 | 0x20))
            {
                return None;
            }
            let next = (info.base_address as usize).checked_add(info.region_size)?;
            if next <= cursor {
                return None;
            }
            cursor = next;
        }
    }
    Some(start..end)
}

fn separate_outputs<const N: usize>(outputs: [Option<Range<usize>>; N]) -> bool {
    outputs.iter().enumerate().all(|(index, output)| {
        output.as_ref().is_some_and(|output| {
            outputs[..index]
                .iter()
                .flatten()
                .all(|other| output.end <= other.start || other.end <= output.start)
        })
    })
}

unsafe fn los_outputs(
    vis: *mut u8,
    vis_len: u32,
    heights: *mut f32,
    heights_len: u32,
    nx: *mut i32,
    nz: *mut i32,
) -> bool {
    unsafe {
        vis_len as usize <= MAX_LOS_ELEMENTS
            && heights_len as usize <= MAX_LOS_ELEMENTS
            && separate_outputs([
                mod_span(vis, vis_len as usize, true),
                mod_span(heights, heights_len as usize, true),
                mod_span(nx, 1, true),
                mod_span(nz, 1, true),
            ])
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
    if !IN_RENDER_CALLBACK.get() {
        return;
    }
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
        if len as usize > MAX_READ_BYTES || mod_span(out, len as usize, true).is_none() {
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
        if mod_span(out, n + 1, true).is_none() {
            return 0;
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, n);
        *out.add(n) = 0;
        n as i32
    }
}

unsafe extern "system" fn host_scene_rect(out: *mut i32) -> i32 {
    unsafe {
        if mod_span(out, 4, true).is_none() {
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
        if mod_span(out, 1, true).is_none() {
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
        if !separate_outputs([mod_span(sx, 1, true), mod_span(sy, 1, true)]) {
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
        if !separate_outputs([
            mod_span(x, 1, true),
            mod_span(y, 1, true),
            mod_span(z, 1, true),
        ]) {
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
        if mod_span(out, 1, true).is_none() {
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
        if !los_outputs(vis, vis_len, heights, heights_len, nx, nz) {
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
        if !separate_outputs([
            mod_span(x, 1, true),
            mod_span(y, 1, true),
            mod_span(z, 1, true),
        ]) {
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
    if !IN_RENDER_CALLBACK.get() {
        return;
    }
    unsafe {
        if count as usize > MAX_DRAW_ELEMENTS
            || !count.is_multiple_of(3)
            || mod_span(verts, count as usize, false).is_none()
        {
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
        let fg = GetForegroundWindow();
        (current_window(fg) && IsIconic(fg) == 0) as i32
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
    if !IN_RENDER_CALLBACK.get() {
        return;
    }
    unsafe {
        if count as usize > MAX_DRAW_ELEMENTS
            || !count.is_multiple_of(2)
            || mod_span(verts, count as usize, false).is_none()
        {
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
        if !los_outputs(vis, vis_len, heights, heights_len, nx, nz) {
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
        if !separate_outputs([
            mod_span(x, 1, true),
            mod_span(y, 1, true),
            mod_span(z, 1, true),
        ]) {
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

const MAX_CAMERA_COMMANDS: usize = 8;
static CAMERA_COMMANDS: Mutex<Option<CameraCommands>> = Mutex::new(None);

struct CameraCommands {
    scene: u32,
    window: u32,
    queued_at: u32,
    commands: [CokeCameraCommand; MAX_CAMERA_COMMANDS],
    len: usize,
}

unsafe extern "system" fn host_camera_apply(cmd: *const CokeCameraCommand) -> i32 {
    unsafe {
        if MOD_INIT.with(|init| init.borrow().is_some())
            || !profile_ok()
            || host_focused() == 0
            || mod_span(cmd, 1, false).is_none()
        {
            return 0;
        }
        let command = cmd.read();
        if !camera_command_valid(&command) {
            return 0;
        }
        let Ok(mut queued) = CAMERA_COMMANDS.try_lock() else {
            return 0;
        };
        let scene = SCENE_GENERATION.load(Ordering::Relaxed);
        let window = WINDOW_GENERATION.load(Ordering::Acquire);
        let now = GetTickCount();
        if queued.as_ref().is_none_or(|batch| {
            batch.scene != scene
                || batch.window != window
                || now.wrapping_sub(batch.queued_at) > 150
        }) {
            *queued = Some(CameraCommands {
                scene,
                window,
                queued_at: now,
                commands: [CokeCameraCommand::default(); MAX_CAMERA_COMMANDS],
                len: 0,
            });
        }
        let batch = queued.as_mut().unwrap();
        if batch.len == batch.commands.len() {
            return 0;
        }
        batch.commands[batch.len] = command;
        batch.len += 1;
        1
    }
}

/// The game's presentation thread owns controller camera mutations between frames.
pub(crate) unsafe fn apply_queued_camera(battle: bool) {
    let pending = CAMERA_COMMANDS
        .try_lock()
        .ok()
        .and_then(|mut queue| queue.take());
    if let Some(batch) = pending
        && battle
        && batch.scene == SCENE_GENERATION.load(Ordering::Relaxed)
        && batch.window == WINDOW_GENERATION.load(Ordering::Acquire)
        && unsafe { GetTickCount().wrapping_sub(batch.queued_at) <= 150 && host_focused() != 0 }
    {
        for command in &batch.commands[..batch.len] {
            unsafe { camera_apply_command(command) };
        }
    }
}

unsafe extern "system" fn host_camera_wheel_delta() -> f32 {
    CAMERA_WHEEL_NOTCHES.swap(0, Ordering::Relaxed) as f32
}

unsafe extern "system" fn host_camera_input_capture(captured: i32) -> i32 {
    // Bit 0 claims native camera input; bit 1 enables free flight and R/F
    // suppression; bit 2 disables terrain clearance in free-camera mode.
    MOD_INIT.with(|init| {
        if let Some(init) = init.borrow_mut().as_mut() {
            init.camera_capture = Some(captured);
        } else {
            set_camera_input_capture(captured);
        }
    });
    1
}

/// Record wheel input for camera mods before the native window procedure
/// consumes it. Mods decide whether to use the delta; the host never changes
/// native wheel behavior on their behalf.
pub fn coke_note_wheel(notches: i32) {
    CAMERA_WHEEL_NOTCHES.fetch_add(notches.clamp(-120, 120), Ordering::Relaxed);
}

#[derive(Default)]
struct ModInit {
    layer: i32,
    hotkeys: Vec<(i32, hotkeys::HotkeyFn)>,
    enable_range_keys: bool,
    camera_capture: Option<i32>,
}

thread_local! {
    static IN_RENDER_CALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static MOD_INIT: RefCell<Option<ModInit>> = const { RefCell::new(None) };
}

unsafe extern "system" fn host_register_hotkey(vk: i32, cb: Option<hotkeys::HotkeyFn>) -> i32 {
    MOD_INIT.with(|init| match (init.borrow_mut().as_mut(), cb) {
        (Some(init), Some(cb)) => hotkeys::stage(&mut init.hotkeys, vk, cb) as i32,
        _ => 0,
    })
}

unsafe extern "system" fn host_set_layer(layer: i32) {
    MOD_INIT.with(|init| {
        if let Some(init) = init.borrow_mut().as_mut() {
            init.layer = layer;
        }
    });
}

unsafe extern "system" fn host_range_control(
    action: u32,
    point: *const coke_host::CokeRangePoint,
) -> i32 {
    MOD_INIT.with(|init| {
        if let Some(init) = init.borrow_mut().as_mut() {
            if action == coke_host::RANGE_ENABLE_KEYS {
                init.enable_range_keys = true;
                return 1;
            }
            return 0;
        }
        unsafe { crate::range::control(action, point) }
    })
}

fn initialize_mod(init: CokeInitFn) -> Option<ModInit> {
    MOD_INIT.with(|pending| pending.replace(Some(ModInit::default())));
    let accepted = unsafe { init(&COKE_HOST) } != 0;
    let pending = MOD_INIT.with(|pending| pending.take());
    if accepted { pending } else { None }
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
    range_control: Some(host_range_control),
    range_read: Some(crate::range::read),
    range_capabilities: Some(crate::range::capabilities),
};

type CokeFrameFn = unsafe extern "system" fn(i32, i32, f32);
type CokeHudFn = unsafe extern "system" fn(i32, i32, f32);
type CokeTickFn = unsafe extern "system" fn();
type CokeInitFn = unsafe extern "system" fn(*const CokeHost) -> i32;

struct CokeMod {
    /// Keep the module loaded for the life of the process. `usize` keeps the
    /// static list `Send`.
    _lib: usize,
    name: String,
    frame_ticks: AtomicU64,
    hud_ticks: AtomicU64,
    worker_ticks: AtomicU64,
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

static COKE_MODS: OnceLock<Vec<CokeMod>> = OnceLock::new();
static COKE_LOADED: AtomicU32 = AtomicU32::new(0);

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

        let mut callbacks = Vec::new();
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
                    let pending = if init_ptr.is_null() {
                        Some(ModInit::default())
                    } else {
                        initialize_mod(core::mem::transmute::<*mut c_void, CokeInitFn>(init_ptr))
                    };
                    if let Some(pending) = pending {
                        let layer = pending.layer;
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
                        callbacks.push(CokeMod {
                            _lib: lib as usize,
                            name: name.to_string(),
                            frame_ticks: AtomicU64::new(0),
                            hud_ticks: AtomicU64::new(0),
                            worker_ticks: AtomicU64::new(0),
                            frame,
                            hud,
                            tick,
                            layer,
                            order: order as u32,
                        });
                        if pending.enable_range_keys {
                            crate::range::control(coke_host::RANGE_ENABLE_KEYS, core::ptr::null());
                        }
                        if let Some(captured) = pending.camera_capture {
                            set_camera_input_capture(captured);
                        }
                        hotkeys::publish(lib as usize, pending.hotkeys);
                        loaded += 1;
                        log(&format!(
                            "coke: loaded {name} (frame={} hud={} tick={} layer={layer})",
                            if frame.is_some() { "yes" } else { "no" },
                            if hud.is_some() { "yes" } else { "no" },
                            if tick.is_some() { "yes" } else { "no" }
                        ));
                    } else {
                        log(&format!("coke: {name} declined initialization"));
                        FreeLibrary(lib);
                    }
                }
            }
        }
        sort_by_draw_order(&mut callbacks, |m| (m.layer, m.order));
        let _ = COKE_MODS.set(callbacks);
        log(&format!("coke: {loaded} mod(s) loaded"));
        if loaded > 0 {
            start_mod_tick_thread();
        }
    }
}

/// Call every loaded mod's `coke_frame` hook. Runs on the GL thread inside
/// `wglSwapBuffers`, using the immutable callback list.
/// Iteration follows the draw-order rule `(layer, load order)`, so overlapping
/// world-space drawing respects a mod's requested layer.
pub unsafe fn coke_frame(width: i32, height: i32, fps: f32) {
    unsafe {
        if let Some(callbacks) = COKE_MODS.get() {
            for m in callbacks {
                if let Some(frame) = m.frame {
                    let previous = IN_RENDER_CALLBACK.replace(true);
                    timed_callback(&m.frame_ticks, || frame(width, height, fps));
                    IN_RENDER_CALLBACK.set(previous);
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
        if let Some(callbacks) = COKE_MODS.get() {
            for m in callbacks {
                if let Some(hud) = m.hud {
                    let previous = IN_RENDER_CALLBACK.replace(true);
                    timed_callback(&m.hud_ticks, || hud(width, height, fps));
                    IN_RENDER_CALLBACK.set(previous);
                }
            }
        }
    }
}

/// Call every loaded mod's `coke_tick` hook. Runs on the dedicated worker
/// thread, never on the GL thread, so data scans cannot stall rendering.
pub unsafe fn coke_tick() {
    unsafe {
        if let Some(callbacks) = COKE_MODS.get() {
            for m in callbacks {
                if let Some(tick) = m.tick {
                    timed_callback(&m.worker_ticks, || tick());
                }
            }
        }
    }
}

fn timed_callback(counter: &AtomicU64, callback: impl FnOnce()) {
    if perf::timing() {
        let start = unsafe { qpc() };
        callback();
        let elapsed = unsafe { qpc() }.saturating_sub(start);
        counter.fetch_add(elapsed, Ordering::Relaxed);
    } else {
        callback();
    }
}

fn report_callback_times() {
    if let Some(mods) = COKE_MODS.get() {
        for m in mods {
            let millis = |counter: &AtomicU64| {
                perf::ticks_to_ns(counter.swap(0, Ordering::Relaxed)) as f64 / 1_000_000.0
            };
            log(&format!(
                "coke: {} callback time over 5 s: world={:.2} ms hud={:.2} ms worker={:.2} ms",
                m.name,
                millis(&m.frame_ticks),
                millis(&m.hud_ticks),
                millis(&m.worker_ticks)
            ));
        }
    }
}

/// Pick a CPU the game's main thread is not using and pin this worker to it.
unsafe fn pin_worker_to_own_core() {
    unsafe {
        let mut pmask = 0usize;
        let mut smask = 0usize;
        if GetProcessAffinityMask(GetCurrentProcess(), &mut pmask, &mut smask) == 0 || pmask == 0 {
            log(&format!(
                "coke: GetProcessAffinityMask failed: {}",
                GetLastError()
            ));
            return;
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
                    let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
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
        if SetThreadAffinityMask(GetCurrentThread(), 1usize << cpu) == 0 {
            log(&format!(
                "coke: could not pin mod worker to cpu {cpu}: {}",
                GetLastError()
            ));
        } else {
            log(&format!(
                "coke: mod worker pinned to cpu {cpu} (process mask 0x{pmask:x})"
            ));
        }
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) == 0 {
            log(&format!(
                "coke: could not lower mod worker priority: {}",
                GetLastError()
            ));
        }
    }
}

extern "system" fn mod_tick_thread(_param: *mut c_void) -> u32 {
    unsafe {
        if config().mod_worker_affinity {
            let t0 = GetTickCount();
            while MAIN_WINDOW.load(Ordering::Acquire) == 0
                && GetTickCount().wrapping_sub(t0) < 15_000
            {
                Sleep(50);
            }
            pin_worker_to_own_core();
        }
        let mut reported_at = GetTickCount();
        loop {
            coke_tick();
            if perf::timing() {
                let now = GetTickCount();
                if now.wrapping_sub(reported_at) >= 5000 {
                    reported_at = now;
                    report_callback_times();
                }
            }
            Sleep(10);
        }
    }
}

pub unsafe fn start_mod_tick_thread() {
    unsafe {
        let mut tid = 0u32;
        let worker = CreateThread(
            core::ptr::null_mut(),
            256 * 1024,
            mod_tick_thread,
            core::ptr::null_mut(),
            0,
            &mut tid,
        );
        if worker.is_null() {
            log(&format!("coke: worker thread failed: {}", GetLastError()));
        } else {
            CloseHandle(worker);
            log(&format!("coke: worker thread {tid}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_mod_never_publishes_its_hotkey() {
        static FIRED: AtomicU32 = AtomicU32::new(0);
        unsafe extern "system" fn fired() {
            FIRED.fetch_add(1, Ordering::Relaxed);
        }
        unsafe extern "system" fn decline(host: *const CokeHost) -> i32 {
            let register = unsafe { (*host).register_hotkey.unwrap() };
            assert_eq!(unsafe { register(0x71, Some(fired)) }, 1);
            0
        }
        assert!(initialize_mod(decline).is_none());
        hotkeys::press_for_test(0x71);
        assert_eq!(FIRED.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn los_validates_element_spans_and_disjoint_outputs() {
        let mut heights = [0.0f32; 8];
        let mut vis = [0u8; 8];
        let mut nx = 0;
        let mut nz = 0;
        unsafe {
            let h = heights.as_mut_ptr();
            assert_eq!(mod_span(h, 8, true).unwrap().len(), 32);
            assert!(los_outputs(vis.as_mut_ptr(), 8, h, 8, &mut nx, &mut nz));
            assert!(!los_outputs(
                h.cast::<u8>().add(16),
                8,
                h,
                8,
                &mut nx,
                &mut nz
            ));
            assert!(!los_outputs(
                vis.as_mut_ptr(),
                8,
                h,
                8,
                &raw mut nx,
                &raw mut nx
            ));
            assert!(!los_outputs(
                vis.as_mut_ptr(),
                8,
                h,
                8,
                h.cast::<i32>().add(4),
                &mut nz
            ));
            assert!(!los_outputs(
                vis.as_mut_ptr(),
                8,
                h,
                u32::MAX,
                &mut nx,
                &mut nz
            ));
        }
    }

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
    fn legacy_host_copy_exposes_only_the_advertised_prefix() {
        assert_eq!(coke_host::COKE_HOST_V6_SIZE, 144);
        let mut source = COKE_HOST;
        source.api_version = 6;
        source.size = coke_host::COKE_HOST_V6_SIZE as u32;
        let mut prefix = [0usize; coke_host::COKE_HOST_V6_SIZE / core::mem::size_of::<usize>()];
        let copied = unsafe {
            core::ptr::copy_nonoverlapping(
                (&source as *const CokeHost).cast::<u8>(),
                prefix.as_mut_ptr().cast::<u8>(),
                coke_host::COKE_HOST_V6_SIZE,
            );
            CokeHost::copy_from_raw(prefix.as_ptr().cast())
        }
        .unwrap();
        assert!(copied.camera_apply.is_some());
        assert!(copied.range_control.is_none());
        assert!(copied.range_read.is_none());
        assert!(copied.range_capabilities.is_none());
    }

    const _: () = {
        assert!(core::mem::offset_of!(CokeHost, size) == 24);
        assert!(core::mem::offset_of!(CokeHost, draw_world_lines) == 112);
        assert!(coke_host::COKE_HOST_V2_1_SIZE == 124);
        assert!(coke_host::COKE_HOST_V4_SIZE == 132);
        assert!(coke_host::COKE_HOST_V6_SIZE == 144);
        assert!(coke_host::COKE_HOST_V7_SIZE == 152);
        assert!(size_of::<CokeHost>() == 156);
    };

    #[test]
    fn host_api_reports_v8() {
        assert_eq!(COKE_HOST.api_version, 8);
    }
}
