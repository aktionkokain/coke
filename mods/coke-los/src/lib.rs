//! cokelos — line-of-sight fan for Combat Mission: Black Sea.
//!
//! Loaded by the `coke` proxy from `coke/cokelos.dll` when
//! `coke/user_consent_cokelos.txt` exists.
//!
//! v0.1 shows the unit's **terrain** line of sight as a terrain-conforming
//! fan. Tree/building occluders are the next step: the engine's objects are
//! not wired into the host yet, so the fan is labelled as terrain-only in the
//! overlay rather than pretending to be exact spotting.
//!
//! Hotkeys (the game's own `Data/hotkeys.txt` cannot bind modifiers):
//!   Ctrl+L        cycle off -> unit fan -> cursor probe -> off
//!   Ctrl+Shift+L  jump straight to the cursor probe
//!   Ctrl+Shift+D  dump the host's LOS data to `coke.log`
//!
//! All scanning runs in `coke_tick`, which the host calls on a dedicated
//! worker thread pinned away from the game's main core. `coke_frame` only
//! submits the cached mesh.

// Exported C entry points have fixed signatures; the host contract lives in
// Pinned to the shared `coke-host` crate; these lints only restate that.
#![allow(clippy::missing_safety_doc, clippy::type_complexity)]

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------- host ABI

use coke_host::*;

static HOST: OnceLock<CokeHost> = OnceLock::new();
static HUD_MODE: AtomicU32 = AtomicU32::new(0);

fn host() -> Option<&'static CokeHost> {
    HOST.get()
}

unsafe fn log(msg: &str) {
    unsafe {
        if let Some(h) = host()
            && let Some(f) = h.log
        {
            let c = std::ffi::CString::new(msg.replace('\0', " ")).unwrap_or_default();
            f(c.as_ptr());
        }
    }
}

unsafe fn draw_text(x: i32, y: i32, text: &str, rgb: u32) {
    unsafe {
        if let Some(h) = host()
            && let Some(f) = h.draw_text
        {
            let c = std::ffi::CString::new(text.replace('\0', " ")).unwrap_or_default();
            f(x, y, c.as_ptr(), rgb);
        }
    }
}

// ------------------------------------------------------------------ config

#[derive(Clone, Copy)]
struct Config {
    enabled: bool,
    cell: f32,
    eye: f32,
    target_h: f32,
    show_blocked: bool,
    show_status: bool,
    alpha: f32,
    lane: f32,
    line_width: f32,
    rays: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Off unless the user explicitly opts in. The host also requires a
            // consent file before the DLL is loaded at all.
            enabled: false,
            cell: 8.0,
            eye: 2.0,
            target_h: 1.6,
            show_blocked: false,
            show_status: true,
            // Filled green region by default; the contour and rays stay on top.
            alpha: 0.20,
            lane: 1.2,
            line_width: 2.0,
            rays: 64,
        }
    }
}

fn load_config() -> Config {
    let mut cfg = Config::default();
    // The working directory is not guaranteed to be the game folder (Steam and
    // shortcuts can differ), so resolve the ini next to the actual executable
    // first and fall back to the working directory.
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("coke").join("cokelos.ini"));
        paths.push(dir.join("cokelos.ini"));
    }
    paths.push(std::path::PathBuf::from("coke\\cokelos.ini"));
    paths.push(std::path::PathBuf::from("cokelos.ini"));
    let Some(text) = paths.iter().find_map(|p| std::fs::read_to_string(p).ok()) else {
        return cfg;
    };
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
        let f = |v: &str| v.parse::<f32>().ok();
        match k.as_str() {
            "enabled" | "active" => cfg.enabled = matches!(v.as_str(), "1" | "true" | "yes" | "on"),
            "cell" | "los_cell" => {
                if let Some(x) = f(&v) {
                    cfg.cell = x.clamp(2.0, 32.0);
                }
            }
            "eye" | "eye_height" => {
                if let Some(x) = f(&v) {
                    cfg.eye = x.clamp(0.5, 10.0);
                }
            }
            "target_height" | "target_h" => {
                if let Some(x) = f(&v) {
                    cfg.target_h = x.clamp(0.0, 6.0);
                }
            }
            "show_blocked" => cfg.show_blocked = matches!(v.as_str(), "1" | "true" | "yes" | "on"),
            "show_status" | "status" => {
                cfg.show_status = matches!(v.as_str(), "1" | "true" | "yes" | "on")
            }
            "alpha" | "fill_alpha" => {
                if let Some(x) = f(&v) {
                    cfg.alpha = x.clamp(0.0, 0.9);
                }
            }
            "lane" => {
                if let Some(x) = f(&v) {
                    cfg.lane = x.clamp(0.2, 8.0);
                }
            }
            "line_width" | "width_px" => {
                if let Some(x) = f(&v) {
                    cfg.line_width = x.clamp(0.5, 8.0);
                }
            }
            "rays" | "ray_count" => {
                if let Ok(x) = v.parse::<u32>() {
                    cfg.rays = x.clamp(0, 720);
                }
            }
            _ => {}
        }
    }
    cfg
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn cfg() -> Config {
    *CONFIG.get_or_init(load_config)
}

// ------------------------------------------------------------------- state

const MODE_OFF: u8 = 0;
const MODE_PROBE: u8 = 1;

static MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
/// Set when the fan must be (re)computed: once on activation, not per frame.
static NEEDS_SCAN: AtomicU32 = AtomicU32::new(0);
static SCAN_SEQUENCE: AtomicU32 = AtomicU32::new(1);
static TRY_MS: AtomicU32 = AtomicU32::new(0);
static DUMP_REQUESTED: AtomicU32 = AtomicU32::new(0);
#[derive(Default)]
struct Mesh {
    /// Segment pairs for `draw_world_lines`.
    lines: Vec<WorldVertex>,
    /// Triangles for `draw_world_triangles` (probe ribbon, optional fill).
    tris: Vec<WorldVertex>,
}

#[derive(Default)]
struct ScanBuffers {
    vis: Vec<u8>,
    heights: Vec<f32>,
}

static SCAN_BUFFERS: Mutex<ScanBuffers> = Mutex::new(ScanBuffers {
    vis: Vec::new(),
    heights: Vec::new(),
});
static MESH: Mutex<Option<Arc<Mesh>>> = Mutex::new(None);
static ORIGIN: OnceLock<Mutex<(f32, f32, f32)>> = OnceLock::new();
static SCAN_FAILED: AtomicU32 = AtomicU32::new(0);
static FAN_BUILT: AtomicU32 = AtomicU32::new(0);

// Status line colours, 0x00RRGGBB (the host rasteriser reads it that way).
const COL_IDLE: u32 = 0x00b8b8b8;
const COL_OK: u32 = 0x0060ff80;
const COL_WARN: u32 = 0x00ffc040;
const COL_ERR: u32 = 0x00ff5040;

struct Status {
    text: String,
    rgb: u32,
}

static STATUS: OnceLock<Mutex<Status>> = OnceLock::new();
static FLASH_UNTIL: AtomicU32 = AtomicU32::new(0);

fn fan_origin() -> &'static Mutex<(f32, f32, f32)> {
    ORIGIN.get_or_init(|| Mutex::new((0.0, 0.0, 0.0)))
}

/// Last successfully scanned origin, per mode. Transient failures (unit walks
/// off-screen, cursor leaves the map, one bad scan) reuse it instead of
/// clearing the overlay, which was the visible on/off flicker.
static LAST_GOOD: OnceLock<Mutex<(u8, f32, f32, f32, u32)>> = OnceLock::new();

fn last_good() -> &'static Mutex<(u8, f32, f32, f32, u32)> {
    LAST_GOOD.get_or_init(|| Mutex::new((0, 0.0, 0.0, 0.0, 0)))
}

fn remember_origin(mode: u8, p: (f32, f32, f32)) {
    if let Ok(mut g) = last_good().try_lock() {
        *g = (mode, p.0, p.1, p.2, now_ms());
    }
}

fn status() -> &'static Mutex<Status> {
    STATUS.get_or_init(|| {
        Mutex::new(Status {
            text: String::from("LOS LOADING"),
            rgb: COL_IDLE,
        })
    })
}

fn now_ms() -> u32 {
    unsafe { host().and_then(|h| h.time_ms).map(|f| f()).unwrap_or(0) }
}

/// Persistent state line. Ignored while a hotkey flash owns the line.
fn set_status(text: &str, rgb: u32) {
    let now = now_ms();
    if (FLASH_UNTIL.load(Ordering::Relaxed).wrapping_sub(now) as i32) > 0 {
        return;
    }
    if let Ok(mut s) = status().try_lock()
        && (s.text != text || s.rgb != rgb)
    {
        s.text.clear();
        s.text.push_str(text);
        s.rgb = rgb;
    }
}

/// Short-lived message shown right after a hotkey, so every key press has
/// immediate on-screen feedback (including the log dump, which had none).
fn flash(text: &str, rgb: u32) {
    // Claim the line first so a concurrent `set_status` cannot overwrite it.
    FLASH_UNTIL.store(now_ms().wrapping_add(1600), Ordering::Relaxed);
    if let Ok(mut s) = status().try_lock() {
        s.text.clear();
        s.text.push_str(text);
        s.rgb = rgb;
    }
}

#[inline]
fn col(r: u8, g: u8, b: u8, a: f32) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | (((a * 255.0) as u32) << 24)
}

// Host input helpers.
unsafe fn key(vk: i32) -> bool {
    unsafe {
        host()
            .and_then(|h| h.key_down)
            .map(|f| f(vk) != 0)
            .unwrap_or(false)
    }
}

unsafe fn focused() -> bool {
    unsafe {
        host()
            .and_then(|h| h.focused)
            .map(|f| f() != 0)
            .unwrap_or(false)
    }
}

// --------------------------------------------------------------- hotkeys

static PREV_PROBE: AtomicU8 = AtomicU8::new(0);
static PREV_DUMP: AtomicU8 = AtomicU8::new(0);

unsafe fn poll_hotkeys() {
    unsafe {
        if !focused() {
            PREV_PROBE.store(0, Ordering::Relaxed);
            PREV_DUMP.store(0, Ordering::Relaxed);
            return;
        }
        const VK_CONTROL: i32 = 0x11;
        const VK_SHIFT: i32 = 0x10;
        const VK_L: i32 = 0x4c;
        const VK_D: i32 = 0x44;

        let ctrl = key(VK_CONTROL);
        let shift = key(VK_SHIFT);
        let l = key(VK_L);
        let d = key(VK_D);

        // Probe is the only mode now (unit-centred LOS needs more RE). Ctrl+L and
        // Ctrl+Shift+L both toggle it.
        let probe_combo = (ctrl && l) as u8;
        let dump_combo = (ctrl && shift && d) as u8;

        if probe_combo != 0 && PREV_PROBE.swap(probe_combo, Ordering::Relaxed) == 0 {
            let next = if MODE.load(Ordering::Relaxed) == MODE_PROBE {
                MODE_OFF
            } else {
                MODE_PROBE
            };
            MODE.store(next, Ordering::Relaxed);
            if next == MODE_PROBE {
                let request = SCAN_SEQUENCE.fetch_add(1, Ordering::Relaxed).max(1);
                NEEDS_SCAN.store(request, Ordering::Release);
            } else {
                NEEDS_SCAN.store(0, Ordering::Release);
            }
            log(&format!(
                "cokelos: probe {}",
                if next == MODE_OFF { "off" } else { "on" }
            ));
            flash(
                if next == MODE_OFF {
                    "LOS MODE: OFF"
                } else {
                    "LOS MODE: CURSOR PROBE"
                },
                COL_OK,
            );
        } else if probe_combo == 0 {
            PREV_PROBE.store(0, Ordering::Relaxed);
        }

        if dump_combo != 0 && PREV_DUMP.swap(dump_combo, Ordering::Relaxed) == 0 {
            // The dump scans the heap and ray-marches the map, so it must not run on
            // the render thread (this function is also polled from `coke_frame`).
            // Hand it to the worker and confirm on screen immediately.
            DUMP_REQUESTED.store(1, Ordering::Relaxed);
            log("cokelos: dump requested");
            flash("LOS DUMP WRITTEN TO COKE.LOG", COL_OK);
        } else if dump_combo == 0 {
            PREV_DUMP.store(0, Ordering::Relaxed);
        }
    }
}

// ------------------------------------------------------------- fan builder

unsafe fn rebuild_fan(origin: (f32, f32, f32), mode: u8, request: u32, label: &str, rgb: u32) {
    unsafe {
        let Some(h) = host() else { return };
        let Some(scan) = h.los_scan_map else {
            set_status("LOS: HOST MISSING MAP SCAN API (VERSION MISMATCH)", COL_ERR);
            return;
        };
        let c = cfg();
        let mut t = CokeTerrain::default();
        if h.terrain.map(|f| f(&mut t)).unwrap_or(0) == 0 || t.world_w <= 0.0 {
            set_status("LOS SCAN FAILED - TERRAIN NOT READY", COL_ERR);
            return;
        }
        // Whole map: the grid is anchored at world (0,0) and sized from the terrain.
        let step = c.cell.clamp(2.0, 32.0);
        let nx = (t.world_w / step).floor() as usize + 1;
        let nz = (t.world_h / step).floor() as usize + 1;
        let n = nx * nz;
        if n == 0 || n > 4_000_000 {
            set_status("LOS SCAN FAILED - MAP TOO LARGE", COL_ERR);
            return;
        }
        let Ok(mut buffers) = SCAN_BUFFERS.lock() else {
            return;
        };
        let ScanBuffers { vis, heights } = &mut *buffers;
        vis.resize(n, 0);
        heights.resize(n, 0.0);
        let (mut ox, mut oz) = (0i32, 0i32);
        let scan_t0 = now_ms();
        let ok = scan(
            origin.0,
            origin.1,
            c.eye,
            step,
            vis.as_mut_ptr(),
            n as u32,
            heights.as_mut_ptr(),
            n as u32,
            &mut ox,
            &mut oz,
        );
        if ok == 0 || ox <= 0 || oz <= 0 {
            if SCAN_FAILED.swap(1, Ordering::Relaxed) == 0 {
                log("cokelos: LOS scan failed (terrain/map not ready)");
            }
            set_status("LOS SCAN FAILED - TERRAIN/MAP NOT READY", COL_ERR);
            return;
        }
        let nx = ox as usize;
        let nz = oz as usize;
        if nx * nz > n {
            return;
        }
        let gx0 = 0.0f32;
        let gy0 = 0.0f32;

        let seen = |i: i64, j: i64| -> bool {
            i >= 0
                && j >= 0
                && (i as usize) < nx
                && (j as usize) < nz
                && vis[j as usize * nx + i as usize] != 0
        };
        // Bilinear ground height over the scan grid.
        let height_at = |x: f32, y: f32| -> f32 {
            let fx = ((x - gx0) / step).clamp(0.0, (nx - 1) as f32);
            let fy = ((y - gy0) / step).clamp(0.0, (nz - 1) as f32);
            let i0 = fx.floor() as usize;
            let j0 = fy.floor() as usize;
            let i1 = (i0 + 1).min(nx - 1);
            let j1 = (j0 + 1).min(nz - 1);
            let tx = fx - i0 as f32;
            let ty = fy - j0 as f32;
            let h00 = heights[j0 * nx + i0];
            let h10 = heights[j0 * nx + i1];
            let h01 = heights[j1 * nx + i0];
            let h11 = heights[j1 * nx + i1];
            (h00 * (1.0 - tx) + h10 * tx) * (1.0 - ty) + (h01 * (1.0 - tx) + h11 * tx) * ty
        };

        // Sit above the ground just enough to avoid intersecting the terrain mesh.
        const Z_OFF: f32 = 0.45;
        let contour = col(90, 255, 130, 0.95);
        let ray_col = col(70, 230, 120, 0.60);
        let mut lines: Vec<WorldVertex> = Vec::with_capacity(nx * nz);
        let mut tris: Vec<WorldVertex> = Vec::new();

        let push_seg = |v: &mut Vec<WorldVertex>, a: (f32, f32), b: (f32, f32), rgba: u32| {
            v.push(WorldVertex {
                x: a.0,
                y: a.1,
                z: height_at(a.0, a.1) + Z_OFF,
                rgba,
            });
            v.push(WorldVertex {
                x: b.0,
                y: b.1,
                z: height_at(b.0, b.1) + Z_OFF,
                rgba,
            });
        };

        let hx = step * 0.5;
        let mut visible_cells = 0usize;
        let mut total_cells = 0usize;
        for j in 0..nz {
            for i in 0..nx {
                let vx = gx0 + i as f32 * step;
                let vy = gy0 + j as f32 * step;
                total_cells += 1;
                let is_visible = seen(i as i64, j as i64);
                if is_visible {
                    visible_cells += 1;
                }

                // Optional translucent fill; off by default (`alpha=0`).
                if c.alpha > 0.0 && (is_visible || c.show_blocked) {
                    let rgba = if is_visible {
                        col(40, 220, 90, c.alpha)
                    } else {
                        col(230, 60, 50, (c.alpha * 0.5).min(0.3))
                    };
                    let z00 = height_at(vx - hx, vy - hx) + 0.2;
                    let z10 = height_at(vx + hx, vy - hx) + 0.2;
                    let z01 = height_at(vx - hx, vy + hx) + 0.2;
                    let z11 = height_at(vx + hx, vy + hx) + 0.2;
                    let a = WorldVertex {
                        x: vx - hx,
                        y: vy - hx,
                        z: z00,
                        rgba,
                    };
                    let b = WorldVertex {
                        x: vx + hx,
                        y: vy - hx,
                        z: z10,
                        rgba,
                    };
                    let cc = WorldVertex {
                        x: vx + hx,
                        y: vy + hx,
                        z: z11,
                        rgba,
                    };
                    let d = WorldVertex {
                        x: vx - hx,
                        y: vy + hx,
                        z: z01,
                        rgba,
                    };
                    tris.extend_from_slice(&[a, b, cc, a, cc, d]);
                }

                if !is_visible {
                    continue;
                }
                let (ii, jj) = (i as i64, j as i64);

                // Boundary of the visible region: every edge shared with a blocked
                // or out-of-range cell. This is the contour the tool exists for.
                if !seen(ii, jj + 1) {
                    push_seg(&mut lines, (vx - hx, vy + hx), (vx + hx, vy + hx), contour);
                }
                if !seen(ii, jj - 1) {
                    push_seg(&mut lines, (vx - hx, vy - hx), (vx + hx, vy - hx), contour);
                }
                if !seen(ii + 1, jj) {
                    push_seg(&mut lines, (vx + hx, vy - hx), (vx + hx, vy + hx), contour);
                }
                if !seen(ii - 1, jj) {
                    push_seg(&mut lines, (vx - hx, vy - hx), (vx - hx, vy + hx), contour);
                }
            }
        }

        // Radial rays from the origin out to the first blocked cell, like a
        // "visibility sunburst": they stop exactly at the contour.
        if c.rays > 0 {
            let count = c.rays.min(720) as usize;
            let start = (step * 0.75).max(3.0);
            let dash = (step * 0.25).max(1.5);
            let max_d = (t.world_w.max(t.world_h) * 1.5).max(64.0);
            for k in 0..count {
                let ang = k as f32 * core::f32::consts::TAU / count as f32;
                let (sx, sy) = (ang.cos(), ang.sin());
                let mut last = start;
                let mut d = start;
                while d <= max_d {
                    let i = ((origin.0 + sx * d - gx0) / step).round() as i64;
                    let j = ((origin.1 + sy * d - gy0) / step).round() as i64;
                    if !seen(i, j) {
                        break;
                    }
                    last = d;
                    d += dash;
                }
                if last > start + 0.5 {
                    push_seg(
                        &mut lines,
                        (origin.0 + sx * start, origin.1 + sy * start),
                        (origin.0 + sx * last, origin.1 + sy * last),
                        ray_col,
                    );
                }
            }
        }

        // Probe line from the selected unit to the probe point.
        if mode == MODE_PROBE
            && let (Some(sel), Some(segment)) = (h.selected_unit, h.los_segment)
        {
            let u = sel();
            if u != 0
                && let Some(up) = h.unit_pos
            {
                let (mut ux, mut uy, mut uz) = (0f32, 0f32, 0f32);
                if up(u, &mut ux, &mut uy, &mut uz) != 0 {
                    let visible = segment(ux, uy, uz + c.eye, origin.0, origin.1, c.target_h) != 0;
                    let lc = if visible {
                        col(60, 240, 110, 0.9)
                    } else {
                        col(255, 70, 60, 0.9)
                    };
                    let z0 = uz + c.eye;
                    let z1 = origin.2 + c.target_h;
                    push_lane(
                        &mut tris,
                        (ux, uy, z0),
                        (origin.0, origin.1, z1),
                        c.lane,
                        lc,
                    );
                    // Diamond marker at the probe.
                    let m = 3.0;
                    let z = z1 + 0.2;
                    let rgba = col(240, 240, 240, 0.85);
                    let p0 = WorldVertex {
                        x: origin.0,
                        y: origin.1 - m,
                        z,
                        rgba,
                    };
                    let p1 = WorldVertex {
                        x: origin.0 + m,
                        y: origin.1,
                        z,
                        rgba,
                    };
                    let p2 = WorldVertex {
                        x: origin.0,
                        y: origin.1 + m,
                        z,
                        rgba,
                    };
                    let p3 = WorldVertex {
                        x: origin.0 - m,
                        y: origin.1,
                        z,
                        rgba,
                    };
                    tris.extend_from_slice(&[p0, p1, p2, p0, p2, p3]);
                }
            }
        }

        let mut current = CokeTerrain::default();
        if NEEDS_SCAN.load(Ordering::Acquire) != request
            || MODE.load(Ordering::Relaxed) != mode
            || h.terrain.map(|f| f(&mut current)).unwrap_or(0) == 0
            || (t.map, t.heights, t.grid_w, t.grid_h)
                != (current.map, current.heights, current.grid_w, current.grid_h)
        {
            return;
        }

        if FAN_BUILT.swap(1, Ordering::Relaxed) == 0 {
            log(&format!(
                "cokelos: fan {label} at ({:.0},{:.0}) {} line verts, {visible_cells}/{total_cells} visible, scan {}ms",
                origin.0,
                origin.1,
                lines.len(),
                now_ms().wrapping_sub(scan_t0)
            ));
        }
        let pct = if total_cells > 0 {
            visible_cells as f32 * 100.0 / total_cells as f32
        } else {
            0.0
        };
        set_status(
            &if visible_cells == 0 {
                format!("LOS {label}: NO VISIBLE GROUND")
            } else {
                format!(
                    "LOS {label}: {} OF {} CELLS ({:.0}%)",
                    visible_cells, total_cells, pct
                )
            },
            if visible_cells == 0 { COL_WARN } else { rgb },
        );

        remember_origin(mode, origin);
        if let Ok(mut o) = fan_origin().lock() {
            *o = origin;
        }
        if let Ok(mut mesh) = MESH.lock() {
            *mesh = Some(Arc::new(Mesh { lines, tris }));
        }
    }
}

/// A flat ribbon between two engine-space points `(x, north, height)`.
fn push_lane(
    v: &mut Vec<WorldVertex>,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    width: f32,
    rgba: u32,
) {
    let dx = b.0 - a.0;
    let dn = b.1 - a.1;
    let len = (dx * dx + dn * dn).sqrt().max(0.01);
    // Perpendicular in the horizontal (x, north) plane.
    let px = -dn / len * width * 0.5;
    let pn = dx / len * width * 0.5;
    let a0 = WorldVertex {
        x: a.0 + px,
        y: a.1 + pn,
        z: a.2,
        rgba,
    };
    let a1 = WorldVertex {
        x: a.0 - px,
        y: a.1 - pn,
        z: a.2,
        rgba,
    };
    let b0 = WorldVertex {
        x: b.0 + px,
        y: b.1 + pn,
        z: b.2,
        rgba,
    };
    let b1 = WorldVertex {
        x: b.0 - px,
        y: b.1 - pn,
        z: b.2,
        rgba,
    };
    v.extend_from_slice(&[a0, a1, b1, a0, b1, b0]);
}

// ---------------------------------------------------------------- exported

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_init(host: *const CokeHost) -> i32 {
    unsafe {
        let Some(h) = CokeHost::copy_from_raw(host) else {
            return 0;
        };
        if h.api_version < 2 || (h.size as usize) < COKE_HOST_V2_1_SIZE {
            return 0;
        }
        if HOST.set(h).is_err() {
            return 1;
        }
        HUD_MODE.store(u32::from(h.api_version >= 3), Ordering::Relaxed);
        let c = cfg();
        if !c.enabled {
            log("cokelos: disabled (set enabled=1 in cokelos.ini to load)");
            return 0;
        }
        log(&format!(
            "cokelos v0.1: api={} size={} cell={:.0} eye={:.1}",
            h.api_version, h.size, c.cell, c.eye
        ));
        log("cokelos: Ctrl+L cycle (unit/probe), Ctrl+Shift+L probe, Ctrl+Shift+D dump");
        set_status("LOS READY - CTRL+L FAN, CTRL+SHIFT+L PROBE", COL_IDLE);
        1
    }
}

unsafe fn cursor_origin(h: &CokeHost) -> Option<(f32, f32, f32)> {
    unsafe {
        let (mut x, mut y, mut z) = (0f32, 0f32, 0f32);
        // Prefer the mouse ray: the engine's cached cursor world could lag, follow
        // a stale target or drift to the map edge.
        if h.cursor_ground
            .map(|f| f(&mut x, &mut y, &mut z))
            .unwrap_or(0)
            != 0
        {
            return Some((x, y, z));
        }
        if h.cursor_world
            .map(|f| f(&mut x, &mut y, &mut z))
            .unwrap_or(0)
            != 0
        {
            Some((x, y, z))
        } else {
            None
        }
    }
}

/// Runs on the host's worker thread. Only scans on demand.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_tick() {
    unsafe {
        poll_hotkeys();
        if DUMP_REQUESTED.swap(0, Ordering::Relaxed) != 0
            && let Some(h) = host()
            && let Some(f) = h.debug_dump
        {
            f();
        }
        let mode = MODE.load(Ordering::Relaxed);
        if mode == MODE_OFF {
            if let Ok(mut mesh) = MESH.try_lock() {
                *mesh = None;
            }
            // Fully off: no mesh and no persistent status line (a hotkey flash still
            // shows for ~1.6 s when toggling).
            if cfg().show_status {
                set_status("", COL_IDLE);
            }
            return;
        }
        // Compute ONCE per activation. The mesh is cached and just re-submitted by
        // `coke_frame`; no periodic rescans and no cursor tracking, which is what
        // made the overlay churn and flicker.
        let request = NEEDS_SCAN.load(Ordering::Acquire);
        if request == 0 {
            return;
        }
        let now = now_ms();
        if now.wrapping_sub(TRY_MS.load(Ordering::Relaxed)) < 200 {
            return;
        }
        TRY_MS.store(now, Ordering::Relaxed);

        let Some(h) = host() else { return };
        let Some(origin) = cursor_origin(h) else {
            set_status("LOS PROBE: AIM AT THE GROUND, THEN CTRL+L", COL_WARN);
            return;
        };

        // Anchor to the terrain surface under the cursor.
        let ground = h
            .terrain_height
            .map(|f| f(origin.0, origin.1))
            .unwrap_or(origin.2);
        rebuild_fan((origin.0, origin.1, ground), mode, request, "PROBE", COL_OK);
        let _ = NEEDS_SCAN.compare_exchange(request, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_frame(_w: i32, _h: i32, _fps: f32) {
    unsafe {
        // Poll here as well as in `coke_tick`: the worker can be busy in a scan, and
        // the render thread always runs. Edge detection uses atomic swaps, so
        // polling from both threads cannot double-toggle a mode.
        poll_hotkeys();
        let mode = MODE.load(Ordering::Relaxed);
        let Some(h) = host() else { return };

        if mode != MODE_OFF
            && let Some(m) = MESH.lock().ok().and_then(|mesh| mesh.clone())
        {
            if !m.lines.is_empty()
                && let Some(f) = h.draw_world_lines
            {
                f(m.lines.as_ptr(), m.lines.len() as u32, cfg().line_width);
            }
            if !m.tris.is_empty()
                && let Some(f) = h.draw_world_triangles
            {
                f(m.tris.as_ptr(), m.tris.len() as u32);
            }
        }

        // On host API v2 the text has to be drawn here. API v3 calls `coke_hud`
        // after all world overlays, which keeps the text above the LOS fill.
        if HUD_MODE.load(Ordering::Relaxed) == 0 {
            draw_hud_text(h);
        }
    }
}

/// 2D text for the LOS overlay. Runs after the world pass on API v3.
unsafe fn draw_hud_text(h: &CokeHost) {
    unsafe {
        let mode = MODE.load(Ordering::Relaxed);
        if mode != MODE_OFF {
            // Independent of the triangle path: project the fan origin with the
            // host's world->screen helper and mark it, so a hotkey always has a
            // visible result even if the mesh is rejected later in the 3D overlay.
            if let (Some(w2s), Ok(o)) = (h.world_to_screen, fan_origin().lock()) {
                let (mut sx, mut sy) = (0f32, 0f32);
                if w2s(o.0, o.1, o.2, &mut sx, &mut sy) != 0 && sx > -8.0 && sy > -8.0 {
                    draw_text(sx as i32 - 6, sy as i32 - 9, "+", COL_OK);
                }
            }
        }

        if !cfg().show_status {
            return;
        }
        let line = status()
            .try_lock()
            .ok()
            .map(|s| (s.text.clone(), s.rgb))
            .unwrap_or_default();
        if !line.0.is_empty() {
            draw_text(8, 30, &line.0, line.1);
        }
    }
}

/// API v3 HUD pass: after all world overlays.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_hud(_w: i32, _h: i32, _fps: f32) {
    unsafe {
        if HUD_MODE.load(Ordering::Relaxed) == 0 {
            return;
        }
        let Some(h) = host() else { return };
        draw_hud_text(h);
    }
}
