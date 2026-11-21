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
pub const COKE_HOST_API: u32 = 7;

/// Size of the API v2.1 prefix (everything up to and including `los_scan_map`).
///
/// Mods that support older proxies compare the host-reported `size` against
/// this instead of `size_of::<CokeHost>()`, so they keep working with a host
/// that does not provide the v4 or v5 tail.
pub const COKE_HOST_V2_1_SIZE: usize = core::mem::offset_of!(CokeHost, register_hotkey);

/// Size through the v4 tail, excluding the v5 camera extensions.
pub const COKE_HOST_V4_SIZE: usize = core::mem::offset_of!(CokeHost, camera_apply);

/// Size through the v6 camera controls.
pub const COKE_HOST_V6_SIZE: usize = core::mem::offset_of!(CokeHost, range_control);

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

/// One bounded camera command in engine coordinates.
///
/// `pan_right` and `pan_forward` are horizontal world-space metres for one
/// host call. `yaw` and `pitch` are radians. `zoom` is a signed world-space
/// elevation delta; hosts may clamp or reject it at terrain clearance.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeCameraCommand {
    pub pan_right: f32,
    pub pan_forward: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
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

pub type CameraApplyFn = unsafe extern "system" fn(*const CokeCameraCommand) -> i32;
pub type CameraWheelFn = unsafe extern "system" fn() -> f32;

/// Range preview actions accepted by `range_control`.
pub const RANGE_ENABLE_KEYS: u32 = 1;
pub const RANGE_FROM_CURSOR: u32 = 2;
pub const RANGE_SET_ORIGIN: u32 = 3;
pub const RANGE_CANCEL: u32 = 4;

/// Range preview status values returned in `CokeRangeResult::status`.
pub const RANGE_WAITING: u32 = 1;
pub const RANGE_CLEAR: u32 = 2;
pub const RANGE_BLOCKED: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeRangePoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeRangeResult {
    pub status: u32,
    pub origin: CokeRangePoint,
    pub target: CokeRangePoint,
    pub distance_m: f32,
    pub visible_fraction: f32,
    /// Cursor in physical client pixels, with a top-left origin.
    pub cursor_x: i32,
    pub cursor_y: i32,
}

pub type RangeControlFn = unsafe extern "system" fn(u32, *const CokeRangePoint) -> i32;
pub type RangeReadFn = unsafe extern "system" fn(*mut CokeRangeResult) -> i32;

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

    // ---- v5 (appended; mods must check `api_version` and `size`) ----
    /// Apply one validated camera command to the live camera object.
    pub camera_apply: Option<CameraApplyFn>,
    /// Return and clear wheel notches observed by the host window procedure.
    pub camera_wheel_delta: Option<CameraWheelFn>,
    /// Claim the native camera input boundary while non-zero.
    pub camera_input_capture: Option<unsafe extern "system" fn(i32) -> i32>,

    // ---- v7 (appended) ----
    /// Control the shared native range preview. A return of 1 accepts the request.
    /// RANGE_SET_ORIGIN takes a ground point in world metres, x east, y north, z up.
    /// Other actions ignore the pointer. Commands may be submitted from any phase.
    pub range_control: Option<RangeControlFn>,
    /// Copy the latest active preview for the caller. A return of 0 means idle,
    /// unsupported or no current scene. Callable from any phase.
    pub range_read: Option<RangeReadFn>,
}

impl CokeHost {
    /// Copy the advertised host prefix and leave any missing function pointers null.
    ///
    /// # Safety
    /// The pointer must come from `coke_init` and must be readable through the
    /// v1 prefix. Hosts at v2 and above must provide the size field and the
    /// advertised number of initialized bytes.
    pub unsafe fn copy_from_raw(host: *const Self) -> Option<Self> {
        if host.is_null() || !host.is_aligned() {
            return None;
        }
        let version = unsafe { core::ptr::addr_of!((*host).api_version).read() };
        let v1_size = core::mem::offset_of!(Self, size);
        let bytes = match version {
            0 => return None,
            1 => v1_size,
            _ => {
                let advertised = unsafe { core::ptr::addr_of!((*host).size).read() } as usize;
                if advertised < v1_size + core::mem::size_of::<u32>() {
                    return None;
                }
                advertised.min(core::mem::size_of::<Self>()) / core::mem::size_of::<usize>()
                    * core::mem::size_of::<usize>()
            }
        };
        let mut copy: Self = unsafe { core::mem::zeroed() };
        unsafe {
            core::ptr::copy_nonoverlapping(
                host.cast::<u8>(),
                (&mut copy as *mut Self).cast::<u8>(),
                bytes,
            )
        };
        Some(copy)
    }
}
