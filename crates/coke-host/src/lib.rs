//! Host ABI shared by the `opengl32.dll` proxy and every `coke/` mod.
//!
//! This is the single definition of the `CokeHost` layout. The proxy builds the
//! table and the mods read it; keeping one copy in a crate means a field cannot
//! be added, removed or retyped in one place and silently missed in another.
//!
//! The ABI is append-only: v1 fields stay first, and each later block is added
//! at the end. A mod must check `api_version` and `size` before touching a field
//! newer than the block it was compiled against.
//!
//! Call registration and layer selection synchronously from `coke_init`.
//! Draw services run on the presentation thread inside `coke_frame` or
//! `coke_hud`. The worker may call `coke_tick` concurrently, and hotkeys
//! run on the watcher thread. Mods synchronize their own shared data.
//! Buffers belong to the caller for the duration of each service call;
//! all output spans must be writable, aligned and disjoint.

#![no_std]

use core::ffi::c_char;

pub const COKE_PROF_VERSION: u32 = 2;

/// Snapshot handed to `coke_prof_read`. All counters are cumulative since the
/// last `coke_prof_reset`. Times are nanoseconds (QPC converted).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeProfStats {
    pub version: u32,
    pub size: u32,

    pub frames: u64,
    pub frame_ns: u64,
    pub last_frame_ns: u64,

    pub swap_calls: u64,
    pub swap_ns: u64,
    pub last_swap_ns: u64,

    pub call_list: u64,
    pub call_list_ns: u64,
    pub bind_texture: u64,
    pub bind_texture_ns: u64,

    pub begin_end: u64,
    pub push_pop: u64,
    pub draw_arrays: u64,
    pub draw_elements: u64,
    pub vertices: u64,

    pub tex_upload: u64,
    pub tex_upload_bytes: u64,
    pub read_pixels: u64,
    pub read_pixels_ns: u64,
    pub copy_tex: u64,

    pub lists_new: u64,
    pub lists_called: u64,
    pub lists_deleted: u64,

    /// Frames for which the CPU-side frame time was measurable.
    pub cpu_frames: u64,
    /// Cumulative CPU-side frame time (engine work, excluding present).
    pub cpu_frame_ns: u64,
    /// CPU-side time of the most recent frame.
    pub last_cpu_frame_ns: u64,
}

/// Current host API version. Bumped whenever the table grows.
pub const COKE_HOST_API: u32 = 8;

/// Size of the original v2 prefix, through the time callback.
pub const COKE_HOST_V2_SIZE: usize = core::mem::offset_of!(CokeHost, draw_world_lines);

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
/// Size through the v7 range-preview ABI.
pub const COKE_HOST_V7_SIZE: usize = core::mem::offset_of!(CokeHost, range_capabilities);

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

/// Local visibility grid in world metres. Buffer lengths count elements
/// (`u8` visibility and `f32` height), with row-major output and X varying first.
/// The last two outputs are the grid dimensions.
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

/// Whole-map visibility grid, with the same buffer and dimension units as
/// `LosScanFn`. The grid starts at world (0, 0).
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
/// Set the shared range source height from `point.z`; `point.x` and `point.y`
/// are ignored. Accepted heights are 0.25 through 6.0 metres. Existing v7
/// table layout is unchanged.
pub const RANGE_SET_SOURCE_HEIGHT: u32 = 5;
/// Replace the transient LOS terrain tint with the supplied tile set. A zero
/// tile count clears the tint. The opaque map value must match the live scene.
pub const RANGE_PAINT_TERRAIN: u32 = 6;
/// Toggle native visibility paint using the rendered surface under the cursor.
pub const RANGE_TOGGLE_TERRAIN_LOS: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CokeTerrainTile {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// 1 = visible (green), 0 = blocked (red); 2 is the blue test palette.
    pub visible: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CokeTerrainTiles {
    pub map: u32,
    pub world_w: f32,
    pub world_h: f32,
    pub tiles: *const CokeTerrainTile,
    pub count: u32,
}

/// `range_capabilities` flags.
pub const RANGE_CAP_PROFILED: u32 = 1 << 0;
pub const RANGE_CAP_HOOK_INSTALLED: u32 = 1 << 1;
pub const RANGE_CAP_TERRAIN_PAINT: u32 = 1 << 2;
pub const RANGE_CAP_TERRAIN_LOS: u32 = 1 << 3;

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
pub type RangeCapabilitiesFn = unsafe extern "system" fn() -> u32;

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
    /// Register from `coke_init`. Accepted registrations become active after
    /// initialization succeeds. F9 and F11 are reserved by the host.
    pub register_hotkey: Option<RegisterHotkeyFn>,
    /// Set this mod's draw-order layer. Call once from `coke_init`; higher
    /// layers draw later (on top). Mods that never call it use layer 0 and keep
    /// their deterministic load order.
    pub set_layer: Option<unsafe extern "system" fn(i32)>,

    // ---- v5 (appended; mods must check `api_version` and `size`) ----
    /// Queue a validated camera command for the presentation thread to apply
    /// between frames, while the originating battle window is focused.
    pub camera_apply: Option<CameraApplyFn>,
    /// Return and clear wheel notches observed by the host window procedure.
    pub camera_wheel_delta: Option<CameraWheelFn>,
    /// Camera capture bits: 1 claims native camera input, 2 enables free
    /// flight and R/F suppression, 4 disables free-flight terrain clearance.
    /// Pass 0 to release capture.
    pub camera_input_capture: Option<unsafe extern "system" fn(i32) -> i32>,

    // ---- v7 (appended) ----
    /// Control the shared native range preview. A return of 1 accepts the request.
    /// RANGE_SET_ORIGIN takes a ground point in world metres, x east, y north, z up.
    /// RANGE_SET_SOURCE_HEIGHT reads only point.z, in metres. Other actions
    /// ignore the pointer. During initialization, only RANGE_ENABLE_KEYS is
    /// accepted and it takes effect after the mod accepts initialization.
    pub range_control: Option<RangeControlFn>,
    /// Copy the latest active preview for the caller. A return of 0 means idle,
    /// unsupported or no current scene. Callable from any phase.
    pub range_read: Option<RangeReadFn>,

    // ---- v8 (appended) ----
    /// Return `RANGE_CAP_*` bits for the active matched title/build. A profiled
    /// backend is configured; a hook-installed backend is ready to draw.
    pub range_capabilities: Option<RangeCapabilitiesFn>,
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
