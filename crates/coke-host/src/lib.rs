//! Host ABI shared by the `opengl32.dll` proxy and every `coke/` mod.
//!
//! This is the single definition of the `CokeHost` layout. The proxy builds the
//! table and the mods read it; keeping one copy in a crate means a field cannot
//! be added, removed or retyped in one place and silently missed in another.
//!
//! The ABI is append-only: v1 fields stay first, and each later block is added
//! at the end. A mod must check `api_version` and `size` before touching a field
//! newer than the block it was compiled against.

#![no_std]

use core::ffi::c_char;

/// Current host API version. Bumped whenever the table grows.
pub const COKE_HOST_API: u32 = 4;

/// Size of the API v2.1 prefix (everything up to and including `los_scan_map`).
///
/// Mods that support older proxies compare the host-reported `size` against
/// this instead of `size_of::<CokeHost>()`, so they keep working with a host
/// that does not provide the v4 tail. The last two fields (`register_hotkey`,
/// `set_layer`) are the only ones added after v2.1.
pub const COKE_HOST_V2_1_SIZE: usize =
    core::mem::size_of::<CokeHost>() - 2 * core::mem::size_of::<usize>();

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeCamera {
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub up: [f32; 3],
    pub fov: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeTerrain {
    pub map: u32,
    pub grid_w: i32,
    pub grid_h: i32,
    pub world_w: f32,
    pub world_h: f32,
    pub heights: u32,
}

/// World-space vertex handed to `draw_world_triangles` / `draw_world_lines`.
/// `rgba` is little-endian bytes R,G,B,A (so `0xAABBGGRR`), matching
/// `glColorPointer`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorldVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rgba: u32,
}

/// Whole-map / ray LOS scan callback (v2 / v2.1).
pub type LosScanFn = unsafe extern "system" fn(
    f32,
    f32,
    f32,
    f32,
    f32,
    f32,
    *mut u8,
    u32,
    *mut f32,
    u32,
    *mut i32,
    *mut i32,
) -> i32;

/// Whole-map visibility grid callback (v2.1).
pub type LosScanMapFn = unsafe extern "system" fn(
    f32,
    f32,
    f32,
    f32,
    *mut u8,
    u32,
    *mut f32,
    u32,
    *mut i32,
    *mut i32,
) -> i32;

/// Hotkey registration callback (v4).
pub type RegisterHotkeyFn =
    unsafe extern "system" fn(i32, Option<unsafe extern "system" fn()>) -> i32;

/// Host services exposed to a `coke` mod. `#[repr(C)]` so a C or Rust DLL can
/// use the same layout. See the module docs for the append-only rule.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CokeHost {
    pub api_version: u32,
    pub log: Option<unsafe extern "system" fn(*const c_char)>,
    pub draw_text: Option<unsafe extern "system" fn(i32, i32, *const c_char, u32)>,
    pub get_width: Option<unsafe extern "system" fn() -> i32>,
    pub get_height: Option<unsafe extern "system" fn() -> i32>,
    pub get_fps: Option<unsafe extern "system" fn() -> f32>,

    // ---- v2 ----
    pub size: u32,
    pub read_u32: Option<unsafe extern "system" fn(u32) -> u32>,
    pub read_f32: Option<unsafe extern "system" fn(u32) -> f32>,
    pub read_bytes: Option<unsafe extern "system" fn(u32, *mut u8, u32) -> i32>,
    pub module_base: Option<unsafe extern "system" fn() -> u32>,
    pub module_size: Option<unsafe extern "system" fn() -> u32>,
    pub class_name: Option<unsafe extern "system" fn(u32, *mut c_char, u32) -> i32>,
    pub scene_rect: Option<unsafe extern "system" fn(*mut i32) -> i32>,
    pub camera: Option<unsafe extern "system" fn(*mut CokeCamera) -> i32>,
    pub world_to_screen:
        Option<unsafe extern "system" fn(f32, f32, f32, *mut f32, *mut f32) -> i32>,
    pub cursor_world: Option<unsafe extern "system" fn(*mut f32, *mut f32, *mut f32) -> i32>,
    pub terrain: Option<unsafe extern "system" fn(*mut CokeTerrain) -> i32>,
    pub terrain_height: Option<unsafe extern "system" fn(f32, f32) -> f32>,
    pub los_scan: Option<LosScanFn>,
    pub los_segment: Option<unsafe extern "system" fn(f32, f32, f32, f32, f32, f32) -> i32>,
    pub selected_unit: Option<unsafe extern "system" fn() -> u32>,
    pub unit_pos: Option<unsafe extern "system" fn(u32, *mut f32, *mut f32, *mut f32) -> i32>,
    pub draw_world_triangles: Option<unsafe extern "system" fn(*const WorldVertex, u32)>,
    pub key_down: Option<unsafe extern "system" fn(i32) -> i32>,
    pub focused: Option<unsafe extern "system" fn() -> i32>,
    pub debug_dump: Option<unsafe extern "system" fn()>,
    pub time_ms: Option<unsafe extern "system" fn() -> u32>,

    // ---- v2.1 (appended; mods must check `size` before using) ----
    /// Draw world-space line segments (pairs of vertices) at a constant screen
    /// width in physical pixels. `rgba` is per-vertex, like the triangles.
    pub draw_world_lines: Option<unsafe extern "system" fn(*const WorldVertex, u32, f32)>,
    /// Ground point under the mouse, from the live camera ray (stable).
    pub cursor_ground: Option<unsafe extern "system" fn(*mut f32, *mut f32, *mut f32) -> i32>,
    /// Whole-map visibility grid (cell = `cell` metres, origin at world 0,0).
    pub los_scan_map: Option<LosScanMapFn>,

    // ---- v4 (appended; mods must check `api_version` and `size`) ----
    /// Register an F1-F12 callback with the host-owned hotkey bus. The host
    /// owns polling, focus checks and rising-edge de-duplication, so one press
    /// fires each registered callback exactly once. Returns 1 when accepted.
    /// A key outside F1-F12 is rejected. See the key map in `docs/ai/CONFIG.md`.
    pub register_hotkey: Option<RegisterHotkeyFn>,
    /// Set this mod's draw-order layer. Call once from `coke_init`; higher
    /// layers draw later (on top). Mods that never call it use layer 0 and keep
    /// their deterministic load order.
    pub set_layer: Option<unsafe extern "system" fn(i32)>,
}
