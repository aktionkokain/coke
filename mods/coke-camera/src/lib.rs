//! Experimental reusable RTS camera controller.
//!
//! The host owns camera validation and writes. This mod owns only input policy
//! and controller state, so future mods can reuse the same API without knowing
//! CMBS addresses or object layouts.

#![allow(clippy::missing_safety_doc)]

use coke_host::{CokeCamera, CokeCameraCommand, CokeHost};
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const VK_W: i32 = 0x57;
const VK_A: i32 = 0x41;
const VK_S: i32 = 0x53;
const VK_D: i32 = 0x44;
const VK_Q: i32 = 0x51;
const VK_E: i32 = 0x45;
const VK_R: i32 = 0x52;
const VK_F: i32 = 0x46;
const VK_RBUTTON: i32 = 0x02;
const VK_MBUTTON: i32 = 0x04;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_F1: i32 = 0x70;
const MAX_MOUSE_DELTA: i32 = 128;
const EDGE_MARGIN: i32 = 12;

static HOST: OnceLock<CokeHost> = OnceLock::new();
static CFG: OnceLock<Config> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State {
    last_ms: 0,
    cursor_x: 0,
    cursor_y: 0,
    dragging: false,
    middle_cursor_x: 0,
    middle_cursor_y: 0,
    middle_dragging: false,
});
static LOGGED_UNAVAILABLE: AtomicU32 = AtomicU32::new(0);
static INPUT_TRACE_AT: AtomicU32 = AtomicU32::new(0);
static CAPTURE_ACTIVE: AtomicU32 = AtomicU32::new(0);
static CAMERA_MISSING_SINCE: AtomicU32 = AtomicU32::new(0);
static FREECAM_ACTIVE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct Config {
    enabled: bool,
    pan_speed: f32,
    yaw_per_pixel: f32,
    pitch_per_pixel: f32,
    wheel_step: f32,
    shift_multiplier: f32,
    ctrl_multiplier: f32,
    mmb_sensitivity: f32,
    freecam_fkey: u8,
    freecam_noclip: bool,
    zoom_reference_fov: f32,
    zoom_drag_floor: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            pan_speed: 60.0,
            yaw_per_pixel: 0.003,
            pitch_per_pixel: 0.002,
            wheel_step: 6.0,
            shift_multiplier: 4.0,
            ctrl_multiplier: 0.5,
            mmb_sensitivity: 0.36,
            freecam_fkey: 6,
            freecam_noclip: false,
            zoom_reference_fov: 45.0,
            zoom_drag_floor: 0.12,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct State {
    last_ms: u32,
    cursor_x: i32,
    cursor_y: i32,
    dragging: bool,
    middle_cursor_x: i32,
    middle_cursor_y: i32,
    middle_dragging: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetCursorPos(point: *mut Point) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn GetForegroundWindow() -> isize;
    fn GetClientRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn ClientToScreen(hwnd: isize, point: *mut Point) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

fn load_config() -> Config {
    let mut cfg = Config::default();
    let mut paths = Vec::new();
    if let Some(dir) = exe_dir() {
        paths.push(dir.join("coke-camera.ini"));
        paths.push(dir.join("coke").join("coke-camera.ini"));
    }
    paths.push(PathBuf::from("coke-camera.ini"));
    paths.push(PathBuf::from("coke\\coke-camera.ini"));
    let Some(path) = paths.into_iter().find(|p| p.is_file()) else {
        return cfg;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return cfg;
    };
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let yes = matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
        let number = value.parse::<f32>().ok();
        match key.as_str() {
            "enabled" => cfg.enabled = yes,
            "pan_speed" => cfg.pan_speed = number.unwrap_or(cfg.pan_speed).clamp(1.0, 200.0),
            "yaw_per_pixel" => {
                cfg.yaw_per_pixel = number.unwrap_or(cfg.yaw_per_pixel).clamp(0.0001, 0.05)
            }
            "pitch_per_pixel" => {
                cfg.pitch_per_pixel = number.unwrap_or(cfg.pitch_per_pixel).clamp(0.0001, 0.05)
            }
            "wheel_step" => cfg.wheel_step = number.unwrap_or(cfg.wheel_step).clamp(0.1, 20.0),
            "shift_multiplier" => {
                cfg.shift_multiplier = number.unwrap_or(cfg.shift_multiplier).clamp(1.0, 8.0)
            }
            "ctrl_multiplier" => {
                cfg.ctrl_multiplier = number.unwrap_or(cfg.ctrl_multiplier).clamp(0.1, 1.0)
            }
            "mmb_sensitivity" => {
                cfg.mmb_sensitivity = number.unwrap_or(cfg.mmb_sensitivity).clamp(0.01, 2.0)
            }
            "freecam_fkey" => {
                cfg.freecam_fkey = number
                    .map(|n| n.round() as i32)
                    .filter(|n| (1..=12).contains(n) && *n != 9 && *n != 11)
                    .unwrap_or(cfg.freecam_fkey as i32) as u8
            }
            "freecam_noclip" => cfg.freecam_noclip = yes,
            "zoom_reference_fov" => {
                cfg.zoom_reference_fov = number.unwrap_or(cfg.zoom_reference_fov).clamp(5.0, 120.0)
            }
            "zoom_drag_floor" => {
                cfg.zoom_drag_floor = number.unwrap_or(cfg.zoom_drag_floor).clamp(0.02, 0.8)
            }
            _ => {}
        }
    }
    cfg
}

unsafe fn log(text: &str) {
    unsafe {
        if let Some(f) = HOST.get().and_then(|h| h.log) {
            let msg = CString::new(text.replace('\0', " ")).unwrap_or_default();
            f(msg.as_ptr());
        }
    }
}

fn host() -> Option<&'static CokeHost> {
    HOST.get()
}

fn reset_state(now: u32) {
    if let Ok(mut state) = STATE.lock() {
        state.last_ms = now;
        state.dragging = false;
        state.middle_dragging = false;
    }
}

fn mouse_position() -> Option<Point> {
    let mut p = Point::default();
    unsafe { (GetCursorPos(&mut p) != 0).then_some(p) }
}

fn drag_bounds() -> Option<(Point, Rect)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut r = Rect::default();
        if GetClientRect(hwnd, &mut r) == 0 || r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        let mut p = Point {
            x: (r.left + r.right) / 2,
            y: (r.top + r.bottom) / 2,
        };
        if ClientToScreen(hwnd, &mut p) == 0 {
            return None;
        }
        let mut top_left = Point {
            x: r.left,
            y: r.top,
        };
        if ClientToScreen(hwnd, &mut top_left) == 0 {
            return None;
        }
        Some((
            p,
            Rect {
                left: top_left.x,
                top: top_left.y,
                right: top_left.x + r.right - r.left,
                bottom: top_left.y + r.bottom - r.top,
            },
        ))
    }
}

fn wrap_cursor_at_edge(p: Point) -> Option<Point> {
    let (center, bounds) = drag_bounds()?;
    if p.x <= bounds.left + EDGE_MARGIN
        || p.x >= bounds.right - EDGE_MARGIN
        || p.y <= bounds.top + EDGE_MARGIN
        || p.y >= bounds.bottom - EDGE_MARGIN
    {
        unsafe {
            SetCursorPos(center.x, center.y);
        }
        Some(center)
    } else {
        None
    }
}

unsafe fn apply(cmd: &CokeCameraCommand) -> bool {
    unsafe {
        host()
            .and_then(|h| h.camera_apply)
            .is_some_and(|f| f(cmd) != 0)
    }
}

fn camera_snapshot() -> Option<CokeCamera> {
    let mut camera = CokeCamera::default();
    unsafe {
        host()
            .and_then(|h| h.camera)
            .is_some_and(|f| f(&mut camera) != 0)
            .then_some(camera)
    }
}

fn key_down(vk: i32) -> bool {
    unsafe { host().and_then(|h| h.key_down).is_some_and(|f| f(vk) != 0) }
}

fn focused() -> bool {
    unsafe { host().and_then(|h| h.focused).is_some_and(|f| f() != 0) }
}

unsafe extern "system" fn toggle_freecam() {
    let active = FREECAM_ACTIVE.fetch_xor(1, Ordering::AcqRel) == 0;
    if let Ok(mut state) = STATE.lock() {
        state.dragging = false;
        state.middle_dragging = false;
    }
    unsafe {
        log(if active {
            "camera: free-camera mode enabled (WASD fly, R/F vertical, RMB look)"
        } else {
            "camera: free-camera mode disabled"
        });
    }
}

unsafe fn tick() {
    unsafe {
        let cfg = *CFG.get_or_init(load_config);
        let Some(host) = host() else { return };
        let Some(now) = host.time_ms.map(|f| f()) else {
            return;
        };
        let active = cfg.enabled && focused() && host.camera_apply.is_some();
        let live_camera = active.then(camera_snapshot).flatten();
        let capture = if !active {
            CAMERA_MISSING_SINCE.store(0, Ordering::Relaxed);
            CAPTURE_ACTIVE.store(0, Ordering::Release);
            false
        } else if live_camera.is_some() {
            CAMERA_MISSING_SINCE.store(0, Ordering::Relaxed);
            CAPTURE_ACTIVE.store(1, Ordering::Release);
            true
        } else if CAPTURE_ACTIVE.load(Ordering::Acquire) != 0 {
            // A render camera pointer can be absent briefly during an auxiliary
            // pass. Keep native camera keys masked across that gap so Q/E cannot
            // leak through to the floor/elevation action for a frame.
            let since = CAMERA_MISSING_SINCE.load(Ordering::Relaxed);
            if since == 0 {
                CAMERA_MISSING_SINCE.store(now.max(1), Ordering::Relaxed);
                true
            } else if now.wrapping_sub(since) < 750 {
                true
            } else {
                CAPTURE_ACTIVE.store(0, Ordering::Release);
                false
            }
        } else {
            false
        };
        let freecam = capture && FREECAM_ACTIVE.load(Ordering::Acquire) != 0;
        if let Some(capture_fn) = host.camera_input_capture {
            capture_fn(
                (capture as i32)
                    | ((freecam as i32) << 1)
                    | (((freecam && cfg.freecam_noclip) as i32) << 2),
            );
        }
        let Some(live_camera) = live_camera else {
            reset_state(now);
            return;
        };
        let dt = if let Ok(mut state) = STATE.lock() {
            let dt = now.wrapping_sub(state.last_ms).clamp(1, 50) as f32 / 1000.0;
            state.last_ms = now;
            dt
        } else {
            return;
        };

        let speed_multiplier = if key_down(VK_SHIFT) {
            cfg.shift_multiplier
        } else {
            1.0
        } * if key_down(VK_CONTROL) {
            cfg.ctrl_multiplier
        } else {
            1.0
        };
        let mut cmd = CokeCameraCommand::default();
        let pan_speed = cfg.pan_speed * speed_multiplier;
        let input_right = (key_down(VK_D) as i32 - key_down(VK_A) as i32) as f32;
        let input_forward = (key_down(VK_W) as i32 - key_down(VK_S) as i32) as f32;
        if freecam {
            let dir = live_camera.dir;
            let up = live_camera.up;
            let dir_len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            let right3 = [
                dir[1] * up[2] - dir[2] * up[1],
                dir[2] * up[0] - dir[0] * up[2],
                dir[0] * up[1] - dir[1] * up[0],
            ];
            let right_len =
                (right3[0] * right3[0] + right3[1] * right3[1] + right3[2] * right3[2]).sqrt();
            if dir_len > 1.0e-5 && right_len > 1.0e-5 {
                let vertical = (key_down(VK_R) as i32 - key_down(VK_F) as i32) as f32;
                let mut movement = [
                    input_forward * dir[0] / dir_len + input_right * right3[0] / right_len,
                    input_forward * dir[1] / dir_len + input_right * right3[1] / right_len,
                    input_forward * dir[2] / dir_len
                        + input_right * right3[2] / right_len
                        + vertical,
                ];
                let length = (movement[0] * movement[0]
                    + movement[1] * movement[1]
                    + movement[2] * movement[2])
                    .sqrt();
                if length > 1.0 {
                    movement.iter_mut().for_each(|v| *v /= length);
                }
                let delta = pan_speed * dt;
                let (fx, fy) = {
                    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                    if len > 1.0e-5 {
                        (dir[0] / len, dir[1] / len)
                    } else {
                        (0.0, 1.0)
                    }
                };
                let ground_right = [fy, -fx];
                cmd.pan_right =
                    (movement[0] * ground_right[0] + movement[1] * ground_right[1]) * delta;
                cmd.pan_forward = (movement[0] * fx + movement[1] * fy) * delta;
                cmd.zoom = movement[2] * delta;
            }
        } else {
            cmd.pan_right = input_right * pan_speed * dt;
            cmd.pan_forward = input_forward * pan_speed * dt;
        }
        // Orbit in the mod path. The engine hook masks Q/E so these keys can
        // never also invoke the native elevation bindings.
        cmd.yaw =
            (key_down(VK_E) as i32 - key_down(VK_Q) as i32) as f32 * 1.4 * speed_multiplier * dt;
        let rmb = key_down(VK_RBUTTON);
        let mmb = key_down(VK_MBUTTON);
        let close_camera_factor =
            (live_camera.fov / cfg.zoom_reference_fov).clamp(cfg.zoom_drag_floor, 1.0);
        if let Some(p) = mouse_position()
            && let Ok(mut state) = STATE.lock()
        {
            if mmb && !state.middle_dragging {
                state.middle_dragging = true;
                state.middle_cursor_x = p.x;
                state.middle_cursor_y = p.y;
                state.dragging = false;
            } else if mmb {
                let dx = (p.x - state.middle_cursor_x).clamp(-MAX_MOUSE_DELTA, MAX_MOUSE_DELTA);
                let dy = (p.y - state.middle_cursor_y).clamp(-MAX_MOUSE_DELTA, MAX_MOUSE_DELTA);
                let scale = cfg.mmb_sensitivity
                    * (live_camera.pos[2] / 60.0).clamp(0.35, 2.0)
                    * close_camera_factor
                    * speed_multiplier;
                let dir = live_camera.dir;
                let up = live_camera.up;
                let fwd_len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                let right3 = [
                    dir[1] * up[2] - dir[2] * up[1],
                    dir[2] * up[0] - dir[0] * up[2],
                    dir[0] * up[1] - dir[1] * up[0],
                ];
                let right_len =
                    (right3[0] * right3[0] + right3[1] * right3[1] + right3[2] * right3[2]).sqrt();
                if fwd_len > 1.0e-5 && right_len > 1.0e-5 {
                    let forward = [dir[0] / fwd_len, dir[1] / fwd_len];
                    let ground_right = [forward[1], -forward[0]];
                    let right3 = [
                        right3[0] / right_len,
                        right3[1] / right_len,
                        right3[2] / right_len,
                    ];
                    // Dragging the scene follows the cursor, so the camera
                    // translates opposite both screen axes. Project the 3D
                    // view-plane translation into the host's ground pan and
                    // vertical elevation command components.
                    let move_x = -(dx as f32 * right3[0] + dy as f32 * up[0]) * scale;
                    let move_y = -(dx as f32 * right3[1] + dy as f32 * up[1]) * scale;
                    cmd.pan_right += move_x * ground_right[0] + move_y * ground_right[1];
                    cmd.pan_forward += move_x * forward[0] + move_y * forward[1];
                    cmd.zoom += -(dx as f32 * right3[2] + dy as f32 * up[2]) * scale;
                }
                if let Some(center) = wrap_cursor_at_edge(p) {
                    state.middle_cursor_x = center.x;
                    state.middle_cursor_y = center.y;
                } else {
                    state.middle_cursor_x = p.x;
                    state.middle_cursor_y = p.y;
                }
            } else {
                state.middle_dragging = false;
            }
            if !mmb && rmb && !state.dragging {
                state.dragging = true;
                state.cursor_x = p.x;
                state.cursor_y = p.y;
            } else if !mmb && rmb && state.dragging {
                let dx = p.x - state.cursor_x;
                let dy = p.y - state.cursor_y;
                if dx.abs() <= MAX_MOUSE_DELTA && dy.abs() <= MAX_MOUSE_DELTA {
                    cmd.yaw += dx as f32 * -cfg.yaw_per_pixel * close_camera_factor;
                    cmd.pitch += dy as f32 * -cfg.pitch_per_pixel * close_camera_factor;
                }
                // The OS cursor is otherwise hard-stopped at the client edge,
                // which makes a long RTS drag stick on the map boundary. Once
                // the pointer reaches an edge, wrap it to the client centre;
                // the next delta continues the same drag gesture.
                if let Some(center) = wrap_cursor_at_edge(p) {
                    state.cursor_x = center.x;
                    state.cursor_y = center.y;
                } else {
                    state.cursor_x = p.x;
                    state.cursor_y = p.y;
                }
            } else if !mmb {
                state.dragging = false;
            }
        }
        if let Some(f) = host.camera_wheel_delta {
            // Coalesce high-polling wheel input into a bounded camera step.
            // The host rejects commands above ±50 world units; a fast wheel
            // can otherwise accumulate dozens of notches between worker ticks.
            cmd.zoom += f().clamp(-4.0, 4.0) * cfg.wheel_step * speed_multiplier;
        }
        // Keep every tick within the host's accepted bounds. A single oversized
        // fast-pan or drag frame must not discard the whole camera update.
        cmd.pan_right = cmd.pan_right.clamp(-50.0, 50.0);
        cmd.pan_forward = cmd.pan_forward.clamp(-50.0, 50.0);
        cmd.yaw = cmd.yaw.clamp(-0.5, 0.5);
        cmd.pitch = cmd.pitch.clamp(-0.5, 0.5);
        cmd.zoom = cmd.zoom.clamp(-50.0, 50.0);
        if (cmd.pan_right != 0.0
            || cmd.pan_forward != 0.0
            || cmd.yaw != 0.0
            || cmd.pitch != 0.0
            || cmd.zoom != 0.0)
            && now.wrapping_sub(INPUT_TRACE_AT.load(Ordering::Relaxed)) >= 1000
        {
            INPUT_TRACE_AT.store(now, Ordering::Relaxed);
            log(&format!(
                "camera: input pan=({:.2},{:.2}) yaw={:.3} pitch={:.3} wheel={:.2} rmb={rmb} mmb={mmb}",
                cmd.pan_right, cmd.pan_forward, cmd.yaw, cmd.pitch, cmd.zoom
            ));
        }
        // Re-assert the current camera even on an input-idle tick. The engine
        // rebuilds its published camera from its internal controller state;
        // without this idle write, a wheel elevation can visibly snap back on
        // the very next engine update.
        if !apply(&cmd) && LOGGED_UNAVAILABLE.swap(1, Ordering::Relaxed) == 0 {
            log("camera: host rejected a command; will retry next tick");
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_init(host: *const CokeHost) -> i32 {
    unsafe {
        let Some(copy) = CokeHost::copy_from_raw(host) else {
            return 0;
        };
        if copy.api_version < 6 || (copy.size as usize) < coke_host::COKE_HOST_V6_SIZE {
            return 0;
        }
        let _ = HOST.set(copy);
        let _ = CFG.set(load_config());
        let cfg = *CFG.get().expect("camera config initialized");
        let vk = VK_F1 + cfg.freecam_fkey as i32 - 1;
        let registered = copy
            .register_hotkey
            .is_some_and(|register| register(vk, Some(toggle_freecam)) != 0);
        if registered {
            log(&format!(
                "camera: free-camera toggle F{} registered",
                cfg.freecam_fkey
            ));
        } else {
            log("camera: free-camera toggle registration failed");
        }
        log("camera: controller loaded and enabled by config");
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_tick() {
    unsafe { tick() }
}
