//! Experimental. The `cokelos` overlay is disabled by default and still needs
//! more RE (occluders, projection and world bounds); see docs/ai/FINDINGS.md.
//! Engine data access for the `cokelos` mod: terrain heightfield, camera,
//! cursor world point, selected unit and a terrain line-of-sight scan.
//!
//! Everything here is read-only and addressed through `ReadProcessMemory`, so
//! a freed map or a wrong build cannot fault the game.
//!
//! Addresses come from the RE pass on the verified shipped build:
//! * `[0x00d7a6ac]` = world object, `world+0x310` = `ActionMap`.
//! * `ActionMap` inherits `TerrainMap`: height array pointer at `+0x9c`,
//!   vertex-grid width/height at `+0xa0`/`+0xa4`, world size (map units) at
//!   `+0x04`/`+0x08`.
//! * `MapView` (RTTI `.?AVMapView@@`, vtable `0x00b9a5dc`) is the battle
//!   view: `+0x48` = the map view model, `+0x6c..+0x70` = the selected-unit
//!   pointer vector, `+0xe0/+0xe4` = the cursor's world position.
//! * `Camera` (RTTI `.?AVCamera@@`, vtable `0x00b98b00`): pos `+0x08`,
//!   dir `+0x18`, up `+0x28`, fov `+0x34`, aspect `+0x38`, near `+0x3c`,
//!   far `+0x40`.

use crate::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

/// `cokelos` engine anchors come from the profile's optional `[los]` block.
/// Absence means the overlay is unavailable on this build; it is not an error.
/// Every value below is resolved as `module_base + rva`.
fn los_world_ptr() -> Option<usize> {
    profile_addr(profile::los()?.world_ptr)
}

fn los_scenario_ptr() -> Option<usize> {
    profile_addr(profile::los()?.scenario_ptr)
}

fn los_app_ptr() -> Option<usize> {
    profile_addr(profile::canvas()?.app_ptr)
}

/// Runtime `MapView` vtable, from the profile; 0 when no `[los]` block exists.
fn vt_mapview() -> u32 {
    profile::los()
        .map(|l| profile_vt(l.vt_mapview))
        .unwrap_or(0)
}

fn vt_camera() -> u32 {
    profile::los().map(|l| profile_vt(l.vt_camera)).unwrap_or(0)
}

fn vt_camera_persp() -> u32 {
    profile::los()
        .map(|l| profile_vt(l.vt_camera_persp))
        .unwrap_or(0)
}

fn vt_unit() -> u32 {
    profile::los().map(|l| profile_vt(l.vt_unit)).unwrap_or(0)
}

/// Terrain field offsets, with the layout defaults when omitted.
fn los_offsets() -> (usize, usize, usize, usize, usize) {
    let l = profile::los();
    (
        l.map(|v| v.height_off as usize).unwrap_or(0x9c),
        l.map(|v| v.grid_w_off as usize).unwrap_or(0xa0),
        l.map(|v| v.grid_h_off as usize).unwrap_or(0xa4),
        l.map(|v| v.world_w_off as usize).unwrap_or(0x04),
        l.map(|v| v.world_h_off as usize).unwrap_or(0x08),
    )
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy)]
pub struct Camera {
    pub pos: Vec3,
    pub dir: Vec3,
    pub up: Vec3,
    pub fov: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Clone, Copy)]
pub struct Terrain {
    pub map: usize,
    pub grid_w: i32,
    pub grid_h: i32,
    pub world_w: f32,
    pub world_h: f32,
    pub height_ptr: usize,
}

// ------------------------------------------------------------- safe reads

#[inline]
pub unsafe fn rd_u32(addr: usize) -> Option<u32> {
    unsafe {
        if addr < 0x10000 {
            return None;
        }
        let mut b = [0u8; 4];
        let mut got = 0usize;
        if ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            b.as_mut_ptr() as *mut c_void,
            4,
            &mut got,
        ) != 0
            && got == 4
        {
            Some(u32::from_le_bytes(b))
        } else {
            None
        }
    }
}

#[inline]
pub unsafe fn rd_f32(addr: usize) -> Option<f32> {
    unsafe { rd_u32(addr).map(f32::from_bits) }
}

pub unsafe fn rd_bytes(addr: usize, out: &mut [u8]) -> bool {
    unsafe {
        if addr < 0x10000 || out.is_empty() {
            return false;
        }
        let mut got = 0usize;
        ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            out.as_mut_ptr() as *mut c_void,
            out.len(),
            &mut got,
        ) != 0
            && got == out.len()
    }
}

#[inline]
pub unsafe fn committed(addr: usize, len: usize) -> bool {
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
        m.state == MEM_COMMIT
            && m.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
            && m.region_size >= len
    }
}

pub unsafe fn module_range() -> (usize, usize) {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            return (0, 0);
        }
        (base, base.saturating_add(module_image_size(base)))
    }
}

unsafe fn vtable_of(addr: usize) -> Option<u32> {
    unsafe {
        if !committed(addr, 4) {
            return None;
        }
        rd_u32(addr)
    }
}

// ---------------------------------------------------------------- world/map

pub unsafe fn world() -> Option<usize> {
    unsafe {
        let w = rd_u32(los_world_ptr()?)? as usize;
        if w < 0x10000 || !committed(w, 0x400) {
            return None;
        }
        Some(w)
    }
}

static TERRAIN_MAP: AtomicUsize = AtomicUsize::new(0);
static TERRAIN_NEXT: AtomicU32 = AtomicU32::new(0);

unsafe fn terrain_from(map: usize) -> Option<Terrain> {
    unsafe {
        if map < 0x10000 || !committed(map, 0x100) {
            return None;
        }
        let l = profile::los()?;
        let vt = vtable_of(map).unwrap_or(0);
        if vt != profile_vt(l.vt_action_map) && vt != profile_vt(l.vt_terrain_map) {
            return None;
        }
        let (height_off, grid_w_off, grid_h_off, world_w_off, world_h_off) = los_offsets();
        let (Some(grid_w), Some(grid_h), Some(world_w), Some(world_h), Some(height_ptr)) = (
            rd_u32(map + grid_w_off),
            rd_u32(map + grid_h_off),
            rd_f32(map + world_w_off),
            rd_f32(map + world_h_off),
            rd_u32(map + height_off),
        ) else {
            return None;
        };
        let (grid_w, grid_h) = (grid_w as i32, grid_h as i32);
        let height_ptr = height_ptr as usize;
        if !(2..=16385).contains(&grid_w)
            || !(2..=16385).contains(&grid_h)
            || !(world_w.is_finite() && world_h.is_finite())
            || world_w < 16.0
            || world_h < 16.0
            || world_w > 20000.0
            || world_h > 20000.0
            || height_ptr < 0x10000
            || !committed(height_ptr, 64)
        {
            return None;
        }
        let cells = (grid_w as usize).checked_mul(grid_h as usize)?;
        if cells > 64 * 1024 * 1024 / 4 {
            return None;
        }
        Some(Terrain {
            map,
            grid_w,
            grid_h,
            world_w,
            world_h,
            height_ptr,
        })
    }
}

pub unsafe fn terrain() -> Option<Terrain> {
    unsafe {
        let l = profile::los()?;
        // The global anchors are authoritative and change per battle; re-read them
        // before trusting the cache so a new map cannot be served stale data.
        for g in &l.map_globals {
            if let Some(v) = profile_addr(*g).and_then(|a| rd_u32(a)) {
                let map = v as usize;
                if map > 0x10000
                    && let Some(t) = terrain_from(map)
                {
                    TERRAIN_MAP.store(map, Ordering::Relaxed);
                    return Some(t);
                }
            }
        }
        let cached = TERRAIN_MAP.load(Ordering::Relaxed);
        if cached != 0 {
            if let Some(t) = terrain_from(cached) {
                return Some(t);
            }
            TERRAIN_MAP.store(0, Ordering::Relaxed);
        }
        let now = GetTickCount();
        if now.wrapping_sub(TERRAIN_NEXT.load(Ordering::Relaxed)) > 0x8000_0000 {
            return None;
        }
        // The location has moved between screens. Try the documented
        // `[app+0x310]`, the static-object reading, then a pointer scan of the
        // application object for an `ActionMap`/`TerrainMap`.
        let mut maps = [0usize; 6];
        let mut n = 0usize;
        let app = los_app_ptr().and_then(|g| rd_u32(g)).unwrap_or(0) as usize;
        // Verified global anchors first: these survive menus and battles.
        for g in &l.map_globals {
            if let Some(v) = profile_addr(*g).and_then(|a| rd_u32(a)) {
                maps[n] = v as usize;
                n += 1;
            }
        }
        for root in [world().unwrap_or(0), app] {
            if root < 0x10000 {
                continue;
            }
            if let Some(v) = rd_u32(root + l.action_map_off as usize) {
                maps[n] = v as usize;
                n += 1;
            }
        }
        if let Some(v) = los_world_ptr().and_then(|a| rd_u32(a + l.action_map_off as usize)) {
            maps[n] = v as usize;
            n += 1;
        }
        for &map in &maps[..n] {
            if let Some(t) = terrain_from(map) {
                TERRAIN_MAP.store(map, Ordering::Relaxed);
                log(&format!(
                    "cokelos: terrain map=0x{map:x} grid={}x{} world={:.0}x{:.0}",
                    t.grid_w, t.grid_h, t.world_w, t.world_h
                ));
                return Some(t);
            }
        }
        for vt in [profile_vt(l.vt_action_map), profile_vt(l.vt_terrain_map)] {
            if let Some(map) = find_ptr_with_vtable(app, 0x20000, vt)
                && let Some(t) = terrain_from(map)
            {
                TERRAIN_MAP.store(map, Ordering::Relaxed);
                log(&format!(
                    "cokelos: terrain map=0x{map:x} (scan) grid={}x{} world={:.0}x{:.0}",
                    t.grid_w, t.grid_h, t.world_w, t.world_h
                ));
                return Some(t);
            }
        }
        TERRAIN_NEXT.store(GetTickCount().wrapping_add(2000), Ordering::Relaxed);
        None
    }
}

/// Bilinear terrain height at a world position (map units, usually metres).
pub unsafe fn terrain_height(t: &Terrain, x: f32, z: f32) -> f32 {
    unsafe {
        let fx = (x / t.world_w * (t.grid_w - 1) as f32).clamp(0.0, (t.grid_w - 1) as f32);
        let fz = (z / t.world_h * (t.grid_h - 1) as f32).clamp(0.0, (t.grid_h - 1) as f32);
        let x0 = fx.floor() as i32;
        let z0 = fz.floor() as i32;
        let x1 = (x0 + 1).min(t.grid_w - 1);
        let z1 = (z0 + 1).min(t.grid_h - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let w = t.grid_w as usize;
        let at = |gx: i32, gz: i32| -> f32 {
            rd_f32(t.height_ptr + (gz as usize * w + gx as usize) * 4).unwrap_or(0.0)
        };
        let h00 = at(x0, z0);
        let h10 = at(x1, z0);
        let h01 = at(x0, z1);
        let h11 = at(x1, z1);
        (h00 * (1.0 - tx) + h10 * tx) * (1.0 - tz) + (h01 * (1.0 - tx) + h11 * tx) * tz
    }
}

/// Copy a sub-rectangle of the height array into `out` (row-major, `nx*nz`).
/// Returns the map-unit origin of the copy and its vertex step.
unsafe fn copy_heights(
    t: &Terrain,
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
) -> Option<(Vec<f32>, i32, i32, f32, f32, i32, i32)> {
    unsafe {
        let sx = (t.grid_w - 1) as f32 / t.world_w;
        let sz = (t.grid_h - 1) as f32 / t.world_h;
        let gx0 = ((x0 * sx).floor() as i32).clamp(0, t.grid_w - 1);
        let gz0 = ((z0 * sz).floor() as i32).clamp(0, t.grid_h - 1);
        let gx1 = ((x1 * sx).ceil() as i32).clamp(gx0, t.grid_w - 1);
        let gz1 = ((z1 * sz).ceil() as i32).clamp(gz0, t.grid_h - 1);
        let nx = (gx1 - gx0 + 1) as usize;
        let nz = (gz1 - gz0 + 1) as usize;
        if nx == 0 || nz == 0 || nx * nz > 32 * 1024 * 1024 / 4 {
            return None;
        }
        let mut out = vec![0f32; nx * nz];
        let mut row = vec![0u8; nx * 4];
        for j in 0..nz {
            let addr = t.height_ptr + ((gz0 as usize + j) * t.grid_w as usize + gx0 as usize) * 4;
            if !rd_bytes(addr, &mut row) {
                return None;
            }
            for i in 0..nx {
                out[j * nx + i] = f32::from_le_bytes([
                    row[i * 4],
                    row[i * 4 + 1],
                    row[i * 4 + 2],
                    row[i * 4 + 3],
                ]);
            }
        }
        // Map-unit coordinates of the copy's grid origin and one vertex step.
        Some((
            out,
            gx0,
            gz0,
            t.world_w / (t.grid_w - 1) as f32,
            t.world_h / (t.grid_h - 1) as f32,
            nx as i32,
            nz as i32,
        ))
    }
}

#[inline]
fn sample_rows(
    data: &[f32],
    nx: i32,
    nz: i32,
    origin_gx: i32,
    origin_gz: i32,
    step_x: f32,
    step_z: f32,
    x: f32,
    z: f32,
) -> f32 {
    if nx <= 0 || nz <= 0 {
        return 0.0;
    }
    let gx = (x / step_x).round() as i32 - origin_gx;
    let gz = (z / step_z).round() as i32 - origin_gz;
    let gx = gx.clamp(0, nx - 1);
    let gz = gz.clamp(0, nz - 1);
    data[(gz * nx + gx) as usize]
}

/// Grid metadata for a terrain scan. `origin_x`/`origin_z`/`cell` are carried so
/// a caller can map a cell back to world metres; the built-in scan only needs
/// the grid dimensions today.
#[derive(Clone, Copy)]
pub struct ScanInfo {
    pub nx: i32,
    pub nz: i32,
    #[allow(dead_code)]
    pub origin_x: f32,
    #[allow(dead_code)]
    pub origin_z: f32,
    #[allow(dead_code)]
    pub cell: f32,
}

/// Terrain-only visibility scan. Fills `vis` with 1 (visible) / 0 (blocked)
/// and `heights` with the ground height at each grid point, both row-major
/// `nx*nz`. `eye` is metres above the ground at the origin, `target_h` metres
/// above the ground at each target point.
///
/// Returns the grid description on success.
pub unsafe fn los_scan(
    ox: f32,
    oz: f32,
    eye: f32,
    range: f32,
    cell: f32,
    target_h: f32,
    vis: &mut [u8],
    heights: &mut [f32],
) -> Option<ScanInfo> {
    unsafe {
        let t = terrain()?;
        let cell = cell.clamp(1.0, 64.0);
        // 1 km radius is enough for CM spotting and bounds the height copy.
        let range = range.clamp(8.0, 1000.0);
        let nx = ((2.0 * range / cell).floor() as i32 + 1).min(257);
        let nz = nx;
        let need = (nx as usize).checked_mul(nz as usize)?;
        if vis.len() < need || heights.len() < need {
            return None;
        }
        let origin_x = ox - (nx as f32 - 1.0) * 0.5 * cell;
        let origin_z = oz - (nz as f32 - 1.0) * 0.5 * cell;
        let x0 = (origin_x - cell).max(0.0);
        let z0 = (origin_z - cell).max(0.0);
        let x1 = (origin_x + nx as f32 * cell).min(t.world_w);
        let z1 = (origin_z + nz as f32 * cell).min(t.world_h);
        let (data, gx0, gz0, step_x, step_z, cnx, cnz) = copy_heights(&t, x0, z0, x1, z1)?;

        let eye_y = sample_rows(&data, cnx, cnz, gx0, gz0, step_x, step_z, ox, oz) + eye;
        // 1 m sampling: the height grid is metre-resolution, and a longer step can
        // miss a thin crest that the engine's own LOS would treat as blocking.
        let step = 1.0f32;

        for j in 0..nz {
            for i in 0..nx {
                let idx = (j * nx + i) as usize;
                let tx = origin_x + i as f32 * cell;
                let tz = origin_z + j as f32 * cell;
                if tx < 0.0 || tz < 0.0 || tx > t.world_w || tz > t.world_h {
                    vis[idx] = 0;
                    heights[idx] = 0.0;
                    continue;
                }
                let th = sample_rows(&data, cnx, cnz, gx0, gz0, step_x, step_z, tx, tz);
                heights[idx] = th;
                if i == nx / 2 && j == nz / 2 {
                    vis[idx] = 1;
                    continue;
                }
                let target_y = th + target_h;
                let dx = tx - ox;
                let dz = tz - oz;
                let dist = (dx * dx + dz * dz).sqrt();
                if dist < 0.01 {
                    vis[idx] = 1;
                    continue;
                }
                let n = (dist / step).ceil() as i32;
                let mut blocked = false;
                for k in 1..n {
                    let f = k as f32 / n as f32;
                    let px = ox + dx * f;
                    let pz = oz + dz * f;
                    let py = eye_y + (target_y - eye_y) * f;
                    let h = sample_rows(&data, cnx, cnz, gx0, gz0, step_x, step_z, px, pz);
                    // A 20 cm clearance margin: a ray that only grazes a crest is
                    // treated as blocked, which matches the engine far better than
                    // the old 5 cm (that made nearly everything "visible").
                    if h > py + 0.20 {
                        blocked = true;
                        break;
                    }
                }
                vis[idx] = (!blocked) as u8;
            }
        }
        Some(ScanInfo {
            nx,
            nz,
            origin_x,
            origin_z,
            cell,
        })
    }
}

/// Whole-map terrain visibility. Computed by sweeping rays from the eye and
/// keeping a running horizon angle: a sample is visible when its elevation
/// angle is at least the highest angle seen closer along the same ray. This is
/// the same point-to-point test as `los_scan`, but O(rays * distance) instead
/// of O(cells * distance), so it can cover the entire map.
///
/// Fills `vis` (1 = visible) and `heights` for an `nx * nz` grid anchored at
/// world (0,0) with `cell` spacing.
pub unsafe fn los_scan_map(
    ox: f32,
    oz: f32,
    eye: f32,
    cell: f32,
    vis: &mut [u8],
    heights: &mut [f32],
) -> Option<(i32, i32)> {
    unsafe {
        let t = terrain()?;
        let cell = cell.clamp(2.0, 32.0);
        let nx = ((t.world_w / cell).floor() as i32 + 1).max(2);
        let nz = ((t.world_h / cell).floor() as i32 + 1).max(2);
        let need = (nx as usize).checked_mul(nz as usize)?;
        if vis.len() < need || heights.len() < need || need > 4_000_000 {
            return None;
        }

        // Cache the full heightfield: it is static for a battle and 24 MB of
        // per-row reads are not something to repeat at scan rate.
        let mut guard = HEIGHT_CACHE.lock().ok()?;
        let cache = guard.get_or_insert_with(|| HeightCache {
            map: 0,
            w: 0,
            h: 0,
            data: Vec::new(),
        });
        if cache.map != t.map
            || cache.w != t.grid_w
            || cache.h != t.grid_h
            || cache.data.len() != (t.grid_w as usize) * (t.grid_h as usize)
        {
            cache.map = t.map;
            cache.w = t.grid_w;
            cache.h = t.grid_h;
            cache.data.clear();
            if !load_heights(&t, &mut cache.data) {
                *guard = None;
                return None;
            }
        }

        let gw = t.grid_w as usize;
        let gh = t.grid_h as usize;
        let inv_w = 1.0 / t.world_w.max(1.0);
        let inv_h = 1.0 / t.world_h.max(1.0);
        let data = &cache.data;
        let sample = |x: f32, z: f32| -> f32 {
            let gx = ((x * inv_w * (t.grid_w - 1) as f32).round() as i32).clamp(0, t.grid_w - 1)
                as usize;
            let gz = ((z * inv_h * (t.grid_h - 1) as f32).round() as i32).clamp(0, t.grid_h - 1)
                as usize;
            data[gz.min(gh - 1) * gw + gx.min(gw - 1)]
        };

        vis[..need].fill(0);
        let eye_z = sample(ox, oz) + eye;
        let max_d = (t.world_w.max(t.world_h) * 1.6).max(64.0);
        // 4096 rays: at 4 km the spacing is ~6 m, below the default 8 m cell, so no
        // cell is skipped. The inner loop is integer-indexed and unchecked; a
        // closure-per-sample version measured ~2.1 s per scan, which starved the
        // engine (LOD/water artifacts) and is why the overlay "flickered".
        let rays = 4096usize;
        let tau = core::f32::consts::TAU;
        let sgx = (t.grid_w - 1) as f32 / t.world_w.max(1.0);
        let sgz = (t.grid_h - 1) as f32 / t.world_h.max(1.0);
        let gw_i = t.grid_w;
        let gh_i = t.grid_h;
        let inv_cell = 1.0 / cell;
        let step = 3.0f32;

        for k in 0..rays {
            let ang = k as f32 * tau / rays as f32;
            let (sx, sz) = (ang.cos(), ang.sin());
            let mut max_slope = f32::NEG_INFINITY;
            let mut d = 1.0f32;
            while d <= max_d {
                let px = ox + sx * d;
                let pz = oz + sz * d;
                if px < 0.0 || pz < 0.0 || px > t.world_w || pz > t.world_h {
                    break;
                }
                let mut gx = (px * sgx + 0.5) as i32;
                let mut gz = (pz * sgz + 0.5) as i32;
                if gx < 0 {
                    gx = 0;
                } else if gx >= gw_i {
                    gx = gw_i - 1;
                }
                if gz < 0 {
                    gz = 0;
                } else if gz >= gh_i {
                    gz = gh_i - 1;
                }
                let h = *data.get_unchecked((gz * gw_i + gx) as usize);
                let slope = (h - eye_z) / d;
                if slope >= max_slope - 1e-4 {
                    if slope > max_slope {
                        max_slope = slope;
                    }
                    let i = (px * inv_cell + 0.5) as i32;
                    let j = (pz * inv_cell + 0.5) as i32;
                    if i >= 0 && j >= 0 && i < nx && j < nz {
                        vis[(j * nx + i) as usize] = 1;
                    }
                }
                d += step;
            }
        }
        // The observer's own spot is always visible.
        let i0 = (ox / cell).round() as i32;
        let j0 = (oz / cell).round() as i32;
        if i0 >= 0 && j0 >= 0 && i0 < nx && j0 < nz {
            vis[(j0 * nx + i0) as usize] = 1;
        }

        // Sampling a grid with rays leaves single-cell holes at long range, which
        // the contour then traces as an internal lattice. Close a cell when most of
        // its four neighbours are visible; the fan grows by at most one cell.
        if let Ok(mut grow) = GROW_BUF.lock() {
            grow.clear();
            grow.resize(need, 0);
            for j in 1..(nz as usize - 1) {
                for i in 1..(nx as usize - 1) {
                    let idx = j * nx as usize + i;
                    if vis[idx] != 0 {
                        grow[idx] = 1;
                        continue;
                    }
                    let mut n = 0u8;
                    if vis[idx - 1] != 0 {
                        n += 1;
                    }
                    if vis[idx + 1] != 0 {
                        n += 1;
                    }
                    if vis[idx - nx as usize] != 0 {
                        n += 1;
                    }
                    if vis[idx + nx as usize] != 0 {
                        n += 1;
                    }
                    if n >= 3 {
                        grow[idx] = 1;
                    }
                }
            }
            vis[..need].copy_from_slice(&grow[..need]);
        }

        for j in 0..nz {
            for i in 0..nx {
                heights[(j * nx + i) as usize] = sample(i as f32 * cell, j as f32 * cell);
            }
        }
        Some((nx, nz))
    }
}

static HEIGHT_CACHE: Mutex<Option<HeightCache>> = Mutex::new(None);
static GROW_BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct HeightCache {
    map: usize,
    w: i32,
    h: i32,
    data: Vec<f32>,
}

unsafe fn load_heights(t: &Terrain, out: &mut Vec<f32>) -> bool {
    unsafe {
        let gw = t.grid_w.max(1) as usize;
        let gh = t.grid_h.max(1) as usize;
        out.resize(gw * gh, 0.0);
        let mut row = vec![0u8; gw * 4];
        for j in 0..gh {
            let addr = t.height_ptr + j * gw * 4;
            if !rd_bytes(addr, &mut row) {
                return false;
            }
            for i in 0..gw {
                out[j * gw + i] = f32::from_le_bytes([
                    row[i * 4],
                    row[i * 4 + 1],
                    row[i * 4 + 2],
                    row[i * 4 + 3],
                ]);
            }
        }
        true
    }
}

/// Single terrain LOS segment, used by the probe line and tests.
pub unsafe fn los_segment(a: Vec3, b: Vec3, target_h: f32) -> bool {
    unsafe {
        let Some(t) = terrain() else { return false };
        // Engine coordinates: x east, y north, z height.
        let dx = b.x - a.x;
        let dn = b.y - a.y;
        let dist = (dx * dx + dn * dn).sqrt();
        if dist < 0.05 {
            return true;
        }
        let target_z = b.z + target_h;
        let n = (dist / 1.0).ceil() as i32;
        let mut blocked = false;
        for k in 1..n {
            let f = k as f32 / n as f32;
            let px = a.x + dx * f;
            let pn = a.y + dn * f;
            let pz = a.z + (target_z - a.z) * f;
            let h = terrain_height(&t, px, pn);
            if h > pz + 0.20 {
                blocked = true;
                break;
            }
        }
        !blocked
    }
}

// ------------------------------------------------------------------- camera

unsafe fn camera_at(p: usize) -> Option<Camera> {
    unsafe {
        // Read the object in one `ReadProcessMemory`. The engine rewrites the
        // camera while a worker may be reading it; field-by-field reads can mix a
        // new position with an old direction, which projects the overlay at a
        // drifting angle.
        const CAM_LEN: usize = 0x48;
        if p < 0x10000 || !committed(p, CAM_LEN) {
            return None;
        }
        let mut b = [0u8; CAM_LEN];
        if !rd_bytes(p, &mut b) {
            return None;
        }
        let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let f = |o: usize| f32::from_bits(u(o));
        let vt = u(0);
        if vt != vt_camera() && vt != vt_camera_persp() {
            return None;
        }
        let cam = Camera {
            pos: Vec3 {
                x: f(0x08),
                y: f(0x0c),
                z: f(0x10),
            },
            dir: Vec3 {
                x: f(0x18),
                y: f(0x1c),
                z: f(0x20),
            },
            up: Vec3 {
                x: f(0x28),
                y: f(0x2c),
                z: f(0x30),
            },
            fov: f(0x34),
            aspect: f(0x38),
            near: f(0x3c),
            far: f(0x40),
        };
        // +0x38 is not a normal aspect ratio on this build (live value 78.9); the
        // engine derives the true aspect from the GL viewport. Only the direction,
        // up vector and FOV are validated here.
        let dir2 = cam.dir.x * cam.dir.x + cam.dir.y * cam.dir.y + cam.dir.z * cam.dir.z;
        let up2 = cam.up.x * cam.up.x + cam.up.y * cam.up.y + cam.up.z * cam.up.z;
        // Native Z/X zoom can reduce the FOV to 1 degree. Rejecting values
        // below 5 degrees detached the override at close zoom and exposed the
        // engine's origin-based drag behavior.
        let valid_shape = cam.fov.is_finite()
            && (0.1..180.0).contains(&cam.fov)
            && dir2.is_finite()
            && up2.is_finite()
            && dir2 > 0.1
            && up2 > 0.1;
        if valid_shape {
            Some(cam)
        } else {
            if CAMERA_SHAPE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 8 {
                log(&format!(
                    "camera: rejected shape ptr=0x{p:x} vt=0x{vt:08x} fov={:.3} dir2={dir2:.3} up2={up2:.3}",
                    cam.fov
                ));
            }
            None
        }
    }
}

static STABLE_CAM: Mutex<Option<Camera>> = Mutex::new(None);
static LAST_CAMERA_HEIGHT: Mutex<Option<(usize, f32)>> = Mutex::new(None);
static CAMERA_REJECT_LOGGED: AtomicU32 = AtomicU32::new(0);
static CAMERA_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static CAMERA_SHAPE_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

unsafe fn native_elevation_held() -> bool {
    unsafe {
        if camera_freecam_active() && camera_freecam_noclip() {
            return true;
        }
        let real = REAL_ASYNC_KEY_STATE.load(Ordering::Acquire);
        if real == 0 {
            return false;
        }
        let f: unsafe extern "system" fn(i32) -> i16 = core::mem::transmute(real);
        // R/F/Z/X remain native elevation/zoom controls. MMB is a mod-owned
        // 3D translation, so its vertical component must not be height-guarded.
        f(0x52) < 0 || f(0x46) < 0 || f(0x5a) < 0 || f(0x58) < 0 || f(0x04) < 0
    }
}

fn camera_reject(reason: &str) -> bool {
    if CAMERA_REJECT_LOGGED.swap(1, Ordering::Relaxed) == 0 {
        log(&format!("camera: apply rejected: {reason}"));
    }
    false
}

/// The camera for the overlay. A transient failed read reuses the last good
/// camera instead of blanking the overlay for a frame.
///
/// This deliberately does **not** reject large frame-to-frame changes. The old
/// 400 m / 72-degree heuristic treated a real map jump or a fast pan as
/// corruption, returned the stale camera without refreshing it, and then kept
/// rejecting every later frame against that same stale snapshot. That is what
/// made the overlay detach from the live view (and oscillate at the threshold).
pub unsafe fn stable_camera() -> Option<Camera> {
    unsafe {
        let current = camera();
        match STABLE_CAM.lock() {
            Ok(mut g) => match current {
                Some(cam) => {
                    *g = Some(cam);
                    Some(cam)
                }
                None => *g,
            },
            Err(_) => current,
        }
    }
}

pub unsafe fn camera() -> Option<Camera> {
    unsafe {
        profile::los()?;
        let app = los_app_ptr().and_then(|g| rd_u32(g))? as usize;
        if app < 0x10000 || !committed(app, 0x40) {
            return None;
        }
        // `FUN_0054ef80` publishes the camera it just built the view matrix from
        // into both `app+0x2c` and `app+0x30` every frame. Resolve the pointer
        // from those slots on every read instead of latching one: auxiliary
        // passes (e.g. `FUN_0054e700`) publish a temporary clone there and free it
        // afterwards, so a latched clone keeps projecting the overlay with a
        // camera the game is no longer rendering with.
        for off in [0x2c, 0x30] {
            let Some(p) = rd_u32(app + off) else { continue };
            if let Some(cam) = camera_at(p as usize) {
                return Some(cam);
            }
        }
        None
    }
}

/// Native input routines obtain the mutable camera from the scene-owner
/// object's +0x10 field. The canvas app has a different +0x10 field; writing
/// its published render clones moves the view without moving tile streaming.
unsafe fn controller_camera_with_ptr() -> Option<(usize, Camera)> {
    unsafe {
        let owner_global = profile::scene().and_then(|s| profile_addr(s.owner_ptr))?;
        let owner = rd_u32(owner_global)? as usize;
        if owner < 0x10000 || !committed(owner, 0x14) {
            return None;
        }
        let ptr = rd_u32(owner + 0x10)? as usize;
        camera_at(ptr).map(|cam| (ptr, cam))
    }
}

/// Write the live camera's view fields after re-validating its vtable and
/// numeric contents. This is the only host-side mutation exposed to `/coke/`
/// mods; callers never receive the raw pointer or build-specific offsets.
pub unsafe fn camera_apply_command(cmd: &coke_host::CokeCameraCommand) -> bool {
    unsafe {
        if !cmd.pan_right.is_finite()
            || !cmd.pan_forward.is_finite()
            || !cmd.yaw.is_finite()
            || !cmd.pitch.is_finite()
            || !cmd.zoom.is_finite()
            || cmd.pan_right.abs() > 50.0
            || cmd.pan_forward.abs() > 50.0
            || cmd.yaw.abs() > 0.5
            || cmd.pitch.abs() > 0.5
            || cmd.zoom.abs() > 50.0
        {
            return camera_reject("command bounds");
        }
        let Some((ptr, mut cam)) = controller_camera_with_ptr() else {
            return camera_reject("controller camera unavailable");
        };
        // The native elevation path can briefly publish a floor-relative
        // camera while the mod is replacing its horizontal view. Keep a large
        // one-frame drop from turning a valid RTS view into a ground-level
        // view, while still allowing normal R/F and wheel elevation changes.
        let mut guarded_height = None;
        if let Ok(last) = LAST_CAMERA_HEIGHT.lock() {
            if let Some((old_ptr, old_z)) = *last {
                // R/F stays native. When neither key is down, however, the
                // engine's idle camera update must not replace a wheel-driven
                // height or slowly walk the camera down to the terrain.
                if old_ptr == ptr && !native_elevation_held() && (cam.pos.z - old_z).abs() > 0.05 {
                    cam.pos.z = old_z;
                }
            }
            guarded_height = Some(());
        }
        let mut fwd = Vec3 {
            x: cam.dir.x,
            y: cam.dir.y,
            z: 0.0,
        };
        let fl = (fwd.x * fwd.x + fwd.y * fwd.y).sqrt();
        if fl <= 1.0e-5 {
            return camera_reject("camera direction invalid");
        }
        fwd.x /= fl;
        fwd.y /= fl;
        let right = Vec3 {
            x: fwd.y,
            y: -fwd.x,
            z: 0.0,
        };
        cam.pos.x += right.x * cmd.pan_right + fwd.x * cmd.pan_forward;
        cam.pos.y += right.y * cmd.pan_right + fwd.y * cmd.pan_forward;
        cam.pos.z += cmd.zoom;
        if let Some(t) = terrain() {
            if !(camera_freecam_active() && camera_freecam_noclip()) {
                cam.pos.x = cam.pos.x.clamp(0.0, t.world_w);
                cam.pos.y = cam.pos.y.clamp(0.0, t.world_h);
                cam.pos.z = cam
                    .pos
                    .z
                    .max(terrain_height(&t, cam.pos.x, cam.pos.y) + 1.0);
            }
        }
        if guarded_height.is_some()
            && let Ok(mut last) = LAST_CAMERA_HEIGHT.lock()
        {
            *last = Some((ptr, cam.pos.z));
        }

        let (sin_y, cos_y) = cmd.yaw.sin_cos();
        let rotate_z = |v: Vec3| Vec3 {
            x: v.x * cos_y - v.y * sin_y,
            y: v.x * sin_y + v.y * cos_y,
            z: v.z,
        };
        cam.dir = rotate_z(cam.dir);
        cam.up = rotate_z(cam.up);

        // Pitch about the camera's right axis. Re-normalisation is deliberately
        // omitted; the native camera keeps these basis vectors near unit length
        // and the small bounded command avoids accumulating useful error.
        if cmd.pitch != 0.0 {
            let axis = Vec3 {
                x: cam.dir.y * cam.up.z - cam.dir.z * cam.up.y,
                y: cam.dir.z * cam.up.x - cam.dir.x * cam.up.z,
                z: cam.dir.x * cam.up.y - cam.dir.y * cam.up.x,
            };
            let (sin_p, cos_p) = cmd.pitch.sin_cos();
            let rotate_axis = |v: Vec3| {
                let dot = v.x * axis.x + v.y * axis.y + v.z * axis.z;
                let cross = Vec3 {
                    x: axis.y * v.z - axis.z * v.y,
                    y: axis.z * v.x - axis.x * v.z,
                    z: axis.x * v.y - axis.y * v.x,
                };
                Vec3 {
                    x: v.x * cos_p + cross.x * sin_p + axis.x * dot * (1.0 - cos_p),
                    y: v.y * cos_p + cross.y * sin_p + axis.y * dot * (1.0 - cos_p),
                    z: v.z * cos_p + cross.z * sin_p + axis.z * dot * (1.0 - cos_p),
                }
            };
            cam.dir = rotate_axis(cam.dir);
            cam.up = rotate_axis(cam.up);
        }
        // Reject a battle-transition race rather than applying the command to
        // a controller that the scene owner has replaced.
        if controller_camera_with_ptr().is_none_or(|(current, _)| current != ptr) {
            return camera_reject("controller changed before write");
        }
        let ok = write_controller_camera_fields(ptr, &cam);
        let n = CAMERA_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if n < 12 {
            log(&format!(
                "camera: apply controller=0x{ptr:x} pos=({:.1},{:.1},{:.1}) cmd=({:.2},{:.2},{:.3},{:.3},{:.2}) ok={ok}",
                cam.pos.x,
                cam.pos.y,
                cam.pos.z,
                cmd.pan_right,
                cmd.pan_forward,
                cmd.yaw,
                cmd.pitch,
                cmd.zoom
            ));
        }
        if ok {
            true
        } else {
            camera_reject("camera write verification failed")
        }
    }
}

unsafe fn write_controller_camera_fields(ptr: usize, cam: &Camera) -> bool {
    unsafe {
        if ptr < 0x10000 || !committed(ptr, 0x100) {
            return false;
        }
        let relative_height = if rd_u32(ptr + 0xf8).unwrap_or(0) != 0 {
            let Some(t) = terrain() else {
                return false;
            };
            Some(cam.pos.z - terrain_height(&t, cam.pos.x, cam.pos.y))
        } else {
            None
        };
        let mut old = 0u32;
        if VirtualProtect(
            ptr as *mut core::ffi::c_void,
            0x100,
            PAGE_READWRITE,
            &mut old,
        ) == 0
        {
            return false;
        }
        let put = |off: usize, v: Vec3| {
            core::ptr::write_unaligned((ptr + off) as *mut f32, v.x);
            core::ptr::write_unaligned((ptr + off + 4) as *mut f32, v.y);
            core::ptr::write_unaligned((ptr + off + 8) as *mut f32, v.z);
        };
        put(0x08, cam.pos);
        put(0x18, cam.dir);
        put(0x28, cam.up);
        // Native validation rolls rejected movement back to this position.
        put(0x4c, cam.pos);
        if let Some(height) = relative_height {
            core::ptr::write_unaligned((ptr + 0xf0) as *mut f32, height);
        }
        let mut ignored = 0u32;
        VirtualProtect(ptr as *mut core::ffi::c_void, 0x100, old, &mut ignored);
        controller_camera_with_ptr().is_some_and(|(current, actual)| {
            current == ptr
                && (actual.pos.x - cam.pos.x).abs() < 0.05
                && (actual.pos.y - cam.pos.y).abs() < 0.05
                && (actual.pos.z - cam.pos.z).abs() < 0.05
        })
    }
}

/// Physical-pixel scene rectangle `(x, y, w, h)`, top-left origin.
pub unsafe fn scene_rect() -> Option<[i32; 4]> {
    unsafe {
        let sc = profile::scene()?;
        let widget = crate::wtree::scene_widget()?;
        if widget < 0x10000 || !committed(widget + sc.widget_rect_off as usize, 8) {
            return None;
        }
        let r = read_rect(widget + sc.widget_rect_off as usize)?;
        let (sx, sy) = scale_xy();
        let ox = OFF_X.load(Ordering::Relaxed);
        let oy = OFF_Y.load(Ordering::Relaxed);
        Some([
            ox + scaled_span(r[0], sx),
            oy + scaled_span(r[1], sy),
            scaled_span(r[2], sx).max(1),
            scaled_span(r[3], sy).max(1),
        ])
    }
}

unsafe fn read_rect(addr: usize) -> Option<[i32; 4]> {
    unsafe {
        let mut b = [0u8; 8];
        if !rd_bytes(addr, &mut b) {
            return None;
        }
        Some([
            i16::from_le_bytes([b[0], b[1]]) as i32,
            i16::from_le_bytes([b[2], b[3]]) as i32,
            i16::from_le_bytes([b[4], b[5]]) as i32,
            i16::from_le_bytes([b[6], b[7]]) as i32,
        ])
    }
}

/// A camera basis plus the viewport mapping, precomputed once per mesh.
///
/// The overlay can contain hundreds of thousands of world vertices. Calling the
/// old `project` for each one re-normalised the camera axes and re-read the
/// config every time; this makes a frame's projection allocation-free apart from
/// the output buffer.
#[derive(Clone, Copy)]
pub struct Projection {
    pos: Vec3,
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    focal: f32,
    aspect: f32,
    rect: [i32; 4],
    flip_x: bool,
    flip_y: bool,
}

pub fn projection(cam: &Camera, rect: [i32; 4]) -> Projection {
    // The engine is X-east, Y-north, Z-up (verified live from the camera:
    // pos=(2992,929,73), dir=(-0.98,0.17,-0.12), up=(-0.12,0.02,0.99)).
    // For a right-handed look-at, right = dir x up and up2 = right x dir.
    let fwd = norm(cam.dir);
    let up = norm(cam.up);
    let right = norm(cross(fwd, up));
    let up2 = cross(right, fwd);
    let cfg = config();
    // The engine derives the aspect from the live GL viewport (FUN_0054ee90),
    // so use the scene rectangle's aspect, not the camera's +0x38 field.
    Projection {
        pos: cam.pos,
        right,
        up: up2,
        fwd,
        focal: 1.0 / (cam.fov.to_radians() * 0.5).tan(),
        aspect: (rect[2] as f32 / rect[3].max(1) as f32).max(0.01),
        rect,
        flip_x: cfg.los_flip_x,
        flip_y: cfg.los_flip_y,
    }
}

impl Projection {
    /// Project a world point to physical screen pixels. Returns `None` when the
    /// point is behind the camera. Overlay projection does not need the engine's
    /// near plane; only points behind the camera are dropped. (The live
    /// camera's +0x3c can be large.)
    #[inline]
    pub fn project(&self, p: Vec3) -> Option<(f32, f32)> {
        let dx = p.x - self.pos.x;
        let dy = p.y - self.pos.y;
        let dz = p.z - self.pos.z;
        let cx = dot(dx, dy, dz, self.right);
        let cy = dot(dx, dy, dz, self.up);
        let cz = dot(dx, dy, dz, self.fwd);
        if cz <= 0.05 {
            return None;
        }
        let mut ndc_x = (cx * self.focal) / (self.aspect * cz);
        let mut ndc_y = (cy * self.focal) / cz;
        if self.flip_x {
            ndc_x = -ndc_x;
        }
        if self.flip_y {
            ndc_y = -ndc_y;
        }
        let sx = self.rect[0] as f32 + (ndc_x * 0.5 + 0.5) * self.rect[2] as f32;
        let sy = self.rect[1] as f32 + (0.5 - ndc_y * 0.5) * self.rect[3] as f32;
        Some((sx, sy))
    }
}

/// Project a single world point. Prefer [`projection`] when projecting many.
pub fn project(cam: &Camera, rect: [i32; 4], p: Vec3) -> Option<(f32, f32)> {
    projection(cam, rect).project(p)
}

#[inline]
fn dot(ax: f32, ay: f32, az: f32, b: Vec3) -> f32 {
    ax * b.x + ay * b.y + az * b.z
}

#[inline]
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

#[inline]
fn norm(v: Vec3) -> Vec3 {
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if l <= 1e-6 {
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        }
    } else {
        Vec3 {
            x: v.x / l,
            y: v.y / l,
            z: v.z / l,
        }
    }
}

// ------------------------------------------------------------------ mapview

static MAPVIEW: AtomicUsize = AtomicUsize::new(0);
static MAPVIEW_NEXT: AtomicU32 = AtomicU32::new(0);
static MAPVIEW_HEAP_NEXT: AtomicU32 = AtomicU32::new(0);
static MAPVIEW_VT: AtomicUsize = AtomicUsize::new(0);
static MAPVIEW_LOGGED: AtomicU32 = AtomicU32::new(0);
static LAYOUT_LOGGED: AtomicU32 = AtomicU32::new(0);
static NO_LAYOUT_LOGGED: AtomicU32 = AtomicU32::new(0);
static NO_CURSOR_LOGGED: AtomicU32 = AtomicU32::new(0);

/// Vtable of `.?AVMapView@@`, resolved by RTTI name from the live image. The
/// hard-coded address is only a fallback; addresses move between builds and the
/// RTTI name does not.
unsafe fn mapview_vtable() -> usize {
    unsafe {
        let cached = MAPVIEW_VT.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        if let Some(vt) = vtable_by_name("MapView") {
            MAPVIEW_VT.store(vt, Ordering::Relaxed);
            if MAPVIEW_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!("cokelos: MapView vtable=0x{vt:x} (RTTI name)"));
            }
            return vt;
        }
        vt_mapview() as usize
    }
}

/// Validate a MapView candidate by class identity only. The old check also
/// demanded a non-empty selected-unit vector, which made every candidate fail
/// while nothing was selected - and that disabled the unit fan *and* the cursor
/// probe for the whole battle.
unsafe fn mapview_valid(p: usize) -> bool {
    unsafe {
        if p < 0x10000 || !committed(p, 0x100) {
            return false;
        }
        if vtable_of(p) != Some(mapview_vtable() as u32) {
            return false;
        }
        // A real MapView owns a model object at +0x48 whose vtable is inside the
        // module image. The selection may be empty; this only rejects random
        // memory that happens to contain the vtable value (false heap-scan hits).
        let Some(m) = rd_u32(p + 0x48) else {
            return false;
        };
        let m = m as usize;
        if m < 0x10000 || !committed(m, 0x80) {
            return false;
        }
        let (base, end) = module_range();
        matches!(vtable_of(m), Some(vt) if (vt as usize) >= base && (vt as usize) < end)
    }
}

/// Bases to try for the MapView's selected-unit vector and cursor: the view
/// itself and, if present, its model object at `+0x48`. The original code
/// validated one and read the other, so at least one was always wrong.
fn mapview_bases(mv: usize) -> [usize; 2] {
    let mut bases = [mv, 0];
    unsafe {
        if let Some(m) = rd_u32(mv + 0x48) {
            let m = m as usize;
            if m > 0x10000 && committed(m, 0x100) {
                bases[1] = m;
            }
        }
    }
    bases
}

unsafe fn unit_vector(addr: usize) -> Option<(usize, usize)> {
    unsafe {
        let b = rd_u32(addr)? as usize;
        let e = rd_u32(addr + 4)? as usize;
        if b >= 0x10000 && e > b && e - b <= 0x4000 && (e - b).is_multiple_of(4) {
            Some((b, e))
        } else {
            None
        }
    }
}

unsafe fn find_ptr_with_vtable(root: usize, range: usize, vt: u32) -> Option<usize> {
    unsafe {
        if root < 0x10000 {
            return None;
        }
        let mut off = 0usize;
        while off < range {
            let Some(v) = rd_u32(root + off) else {
                off += 4;
                continue;
            };
            let p = v as usize;
            if p > 0x10000 && committed(p, 0x10) && vtable_of(p) == Some(vt) {
                return Some(p);
            }
            off += 4;
        }
        None
    }
}

unsafe fn heap_find_vtable(vt: u32, budget_ms: u32) -> Option<usize> {
    unsafe {
        let t0 = GetTickCount();
        let mut addr = 0x10000usize;
        let mut buf = vec![0u8; 256 * 1024];
        while addr < 0x7fff_0000 {
            if GetTickCount().wrapping_sub(t0) > budget_ms {
                break;
            }
            let mut m: MemBasicInfo = core::mem::zeroed();
            if VirtualQuery(
                addr as *const c_void,
                &mut m,
                core::mem::size_of::<MemBasicInfo>(),
            ) == 0
            {
                break;
            }
            let region = m.region_size.max(0x1000);
            let next = addr.saturating_add(region);
            let readable = m.state == MEM_COMMIT
                && m.ty == MEM_PRIVATE
                && m.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
                && m.protect & 0xff != PAGE_EXECUTE_READWRITE;
            if readable && region <= 256 * 1024 * 1024 {
                let mut p = addr;
                while p < next {
                    let len = (next - p).min(buf.len());
                    let len = len & !3;
                    if len >= 4 && rd_bytes(p, &mut buf[..len]) {
                        let mut i = 0usize;
                        while i + 4 <= len {
                            let v =
                                u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
                            if v == vt {
                                let cand = p + i;
                                if mapview_valid(cand) {
                                    return Some(cand);
                                }
                            }
                            i += 4;
                        }
                    }
                    p = p.saturating_add(len.max(4));
                }
            }
            addr = next;
        }
        None
    }
}

/// The live `MapView` instance, cached and re-validated.
pub unsafe fn mapview() -> Option<usize> {
    unsafe {
        profile::los()?;
        let cached = MAPVIEW.load(Ordering::Relaxed);
        if cached != 0 && mapview_valid(cached) {
            return Some(cached);
        }
        if cached != 0 {
            MAPVIEW.store(0, Ordering::Relaxed);
        } else if GetTickCount().wrapping_sub(MAPVIEW_NEXT.load(Ordering::Relaxed)) > 0x8000_0000 {
            return None;
        }
        let vt = mapview_vtable() as u32;
        let world = world().unwrap_or(0);
        let scenario = los_scenario_ptr().and_then(|a| rd_u32(a)).unwrap_or(0) as usize;
        let scene = crate::wtree::scene_widget().unwrap_or(0);
        for (root, range) in [
            (world, 0x8000usize),
            (scenario, 0x8000),
            (scene, 0x2000),
            (
                los_app_ptr().and_then(|a| rd_u32(a)).unwrap_or(0) as usize,
                0x20000,
            ),
        ] {
            if root < 0x10000 {
                continue;
            }
            if let Some(p) = find_ptr_with_vtable(root, range, vt)
                && mapview_valid(p)
            {
                MAPVIEW.store(p, Ordering::Relaxed);
                log(&format!(
                    "cokelos: MapView=0x{p:x} (pointer scan) class={}",
                    class_name_of_vtable(vt as usize).unwrap_or_else(|| "?".into())
                ));
                return Some(p);
            }
        }
        // The heap scan walks every private region; it is the fallback of last
        // resort and must not run every couple of seconds (that was visible as
        // frame hitching). Only while a world exists, and at most every 10 s.
        if world != 0
            && GetTickCount().wrapping_sub(MAPVIEW_HEAP_NEXT.load(Ordering::Relaxed)) < 0x8000_0000
        {
            MAPVIEW_HEAP_NEXT.store(GetTickCount().wrapping_add(10_000), Ordering::Relaxed);
            if let Some(p) = heap_find_vtable(vt, 600) {
                MAPVIEW.store(p, Ordering::Relaxed);
                log(&format!("cokelos: MapView=0x{p:x} (heap scan)"));
                return Some(p);
            }
        }
        if MAPVIEW_LOGGED.swap(1, Ordering::Relaxed) == 0 {
            log(&format!(
                "cokelos: MapView not found (vt=0x{vt:x}, world=0x{world:x}, scene=0x{scene:x})"
            ));
        }
        MAPVIEW_NEXT.store(GetTickCount().wrapping_add(2000), Ordering::Relaxed);
        None
    }
}

// -------------------------------------------------------------- unit access

static UNIT_POS_OFF: AtomicI32 = AtomicI32::new(-1);
static UNIT_CACHE: AtomicUsize = AtomicUsize::new(0);
static RAY_CACHE: Mutex<Option<(usize, Vec3, u32)>> = Mutex::new(None);

/// First selected unit pointer. The vector lives at `MapView+0x6c` on the build
/// the reconnaissance used and at `[MapView+0x48]+0x6c` (the model) on others,
/// so both layouts are tried.
pub unsafe fn selected_unit() -> Option<usize> {
    unsafe {
        let mv = mapview()?;
        for base in mapview_bases(mv) {
            if base == 0 {
                continue;
            }
            let Some((begin, end)) = unit_vector(base + 0x6c) else {
                continue;
            };
            if LAYOUT_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!(
                    "cokelos: selected-unit vector base={} (+0x6c) entries={}",
                    if base == mv { "view" } else { "model" },
                    (end - begin) / 4
                ));
            }
            let n = ((end - begin) / 4).min(64);
            for i in 0..n {
                let Some(u) = rd_u32(begin + i * 4) else {
                    continue;
                };
                let u = u as usize;
                if u < 0x10000 || !committed(u, 0x100) {
                    continue;
                }
                if let Some(vt) = vtable_of(u)
                    && let Some(name) = class_name_of_vtable(vt as usize)
                    && (name.contains("Unit")
                        || name.contains("Vehicle")
                        || name.contains("Soldier")
                        || name.contains("Actor")
                        || vt == vt_unit())
                {
                    if UNIT_CACHE.swap(u, Ordering::Relaxed) != u {
                        log(&format!(
                            "cokelos: selected unit=0x{u:x} class={name} vt=0x{vt:x}"
                        ));
                    }
                    return Some(u);
                }
            }
        }
        if NO_LAYOUT_LOGGED.swap(1, Ordering::Relaxed) == 0 {
            let mut parts = String::new();
            for base in mapview_bases(mv) {
                if base == 0 {
                    continue;
                }
                parts.push_str(&format!(
                    " 0x{base:x}: b=0x{:x} e=0x{:x}",
                    rd_u32(base + 0x6c).unwrap_or(0),
                    rd_u32(base + 0x70).unwrap_or(0)
                ));
            }
            log(&format!("cokelos: no unit vector at +0x6c;{parts}"));
        }
        None
    }
}

/// `x`/`north` are horizontal map coordinates, `height` is the engine's Z-up
/// value. Returns how far `height` is from the ground there.
fn plausible_pos(t: &Terrain, x: f32, north: f32, height: f32) -> Option<f32> {
    if !(x.is_finite() && north.is_finite() && height.is_finite()) {
        return None;
    }
    if x < -32.0 || north < -32.0 || x > t.world_w + 32.0 || north > t.world_h + 32.0 {
        return None;
    }
    if height < -200.0 || height > 2000.0 {
        return None;
    }
    let h = unsafe { terrain_height(t, x, north) };
    let d = (height - h).abs();
    if d <= 4.0 { Some(d) } else { None }
}

/// Intersect the camera ray through a screen pixel with the terrain. Used as a
/// fallback for the selected unit: the marker renderer caches the unit's local
/// screen position at `+0x00/+0x04`, so a visible unit can be placed on the
/// ground without knowing its world-position field.
pub unsafe fn ray_terrain(cam: &Camera, rect: [i32; 4], sx: f32, sy: f32) -> Option<Vec3> {
    unsafe {
        let t = terrain()?;
        let u = (sx - rect[0] as f32) / rect[2].max(1) as f32;
        let v = (sy - rect[1] as f32) / rect[3].max(1) as f32;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let ndc_x = u * 2.0 - 1.0;
        let ndc_y = 1.0 - v * 2.0;
        let fwd = norm(cam.dir);
        let up = norm(cam.up);
        let right = norm(cross(fwd, up));
        let up2 = cross(right, fwd);
        let aspect = rect[2] as f32 / rect[3].max(1) as f32;
        let th = (cam.fov.to_radians() * 0.5).tan();
        let dir = norm(Vec3 {
            x: fwd.x + right.x * th * aspect * ndc_x + up2.x * th * ndc_y,
            y: fwd.y + right.y * th * aspect * ndc_x + up2.y * th * ndc_y,
            z: fwd.z + right.z * th * aspect * ndc_x + up2.z * th * ndc_y,
        });
        let max_d = 5000.0f32;
        let mut prev = 0.0f32;
        let mut d = 2.0f32;
        while d <= max_d {
            let p = Vec3 {
                x: cam.pos.x + dir.x * d,
                y: cam.pos.y + dir.y * d,
                z: cam.pos.z + dir.z * d,
            };
            let h = terrain_height(&t, p.x, p.y);
            if p.z <= h {
                // Refine the crossing between `prev` and `d`.
                let mut lo = prev;
                let mut hi = d;
                for _ in 0..12 {
                    let mid = (lo + hi) * 0.5;
                    let q = Vec3 {
                        x: cam.pos.x + dir.x * mid,
                        y: cam.pos.y + dir.y * mid,
                        z: cam.pos.z + dir.z * mid,
                    };
                    if q.z <= terrain_height(&t, q.x, q.y) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                let px = cam.pos.x + dir.x * hi;
                let py = cam.pos.y + dir.y * hi;
                // The heightfield clamps at the border, so a ray that leaves the map
                // "hits" the edge. Reject that: it parked unit fans and the cursor
                // fan on the world edge.
                if px < 0.0 || py < 0.0 || px > t.world_w || py > t.world_h {
                    return None;
                }
                return Some(Vec3 {
                    x: px,
                    y: py,
                    z: terrain_height(&t, px, py),
                });
            }
            prev = d;
            d += 2.0;
        }
        None
    }
}

/// Best-effort world position of a unit object.
///
/// The engine caches screen position/depth in the unit's first fields; the
/// world position is not exported, so this scans the object for a float triple
/// `(x, y, z)` whose horizontal components are inside the map and whose `y`
/// agrees with the terrain height. The winning offset is cached for the
/// session (unit classes share a layout), and re-scanned when it stops making
/// sense.
pub unsafe fn unit_pos(unit: usize) -> Option<Vec3> {
    unsafe {
        let t = terrain()?;
        // Fast, screen-based path: the marker renderer caches the unit's local
        // screen position at +0x00/+0x04. Projecting that pixel onto the terrain is
        // more reliable than guessing a world-position field, but it only works
        // while the unit is on screen. Cached briefly so the ray march does not run
        // on every scan.
        if let Ok(g) = RAY_CACHE.lock()
            && let Some((u, p, ts)) = *g
            && u == unit
            && GetTickCount().wrapping_sub(ts) < 750
        {
            return Some(p);
        }
        if let (Some(cam), Some(rect)) = (camera(), scene_rect()) {
            let sx = rd_u32(unit).unwrap_or(0) as i32;
            let sy = rd_u32(unit + 4).unwrap_or(0) as i32;
            if sx > 0 && sy > 0 && sx < 0x4000 && sy < 0x4000 {
                // The marker code compares these against the scene widget's width,
                // so they are local to the scene rect. Try local first, then treat
                // them as already-global pixels.
                let local = ray_terrain(
                    &cam,
                    rect,
                    rect[0] as f32 + sx as f32,
                    rect[1] as f32 + sy as f32,
                );
                let global = if local.is_none() {
                    ray_terrain(&cam, rect, sx as f32, sy as f32)
                } else {
                    None
                };
                if let Some(p) = local.or(global) {
                    if let Ok(mut g) = RAY_CACHE.lock() {
                        *g = Some((unit, p, GetTickCount()));
                    }
                    static LOGGED: AtomicU32 = AtomicU32::new(0);
                    if LOGGED.swap(1, Ordering::Relaxed) == 0 {
                        log(&format!(
                            "cokelos: unit pos via screen ray ({:.1},{:.1},{:.1}) from local ({sx},{sy})",
                            p.x, p.y, p.z
                        ));
                    }
                    return Some(p);
                }
            }
        }
        let cached = UNIT_POS_OFF.load(Ordering::Relaxed);
        if cached >= 0 {
            let o = cached as usize;
            if let (Some(a), Some(b), Some(c)) =
                (rd_f32(unit + o), rd_f32(unit + o + 4), rd_f32(unit + o + 8))
            {
                // Engine order is (x, north, height). The alternate (x, height,
                // north) layout is accepted for classes that store it that way.
                if plausible_pos(&t, a, b, c).is_some() {
                    return Some(Vec3 { x: a, y: b, z: c });
                }
                if plausible_pos(&t, a, c, b).is_some() {
                    return Some(Vec3 { x: a, y: c, z: b });
                }
            }
            UNIT_POS_OFF.store(-1, Ordering::Relaxed);
        }
        let mut best: Option<(f32, usize, Vec3)> = None;
        for o in (0x08..0x100).step_by(4) {
            let (Some(a), Some(b), Some(c)) =
                (rd_f32(unit + o), rd_f32(unit + o + 4), rd_f32(unit + o + 8))
            else {
                continue;
            };
            // Try (x, north, height) and (x, height, north); accept both world and
            // 8-unit cell coordinates for the horizontal pair.
            for &(x, north, height) in &[(a, b, c), (a, c, b)] {
                for scale in [1.0f32, 8.0] {
                    let (px, pn) = (x * scale, north * scale);
                    if let Some(d) = plausible_pos(&t, px, pn, height)
                        && best.as_ref().map(|b| d < b.0).unwrap_or(true)
                    {
                        best = Some((
                            d,
                            o,
                            Vec3 {
                                x: px,
                                y: pn,
                                z: height,
                            },
                        ));
                    }
                }
            }
        }
        if let Some((d, o, p)) = best {
            UNIT_POS_OFF.store(o as i32, Ordering::Relaxed);
            log(&format!(
                "cokelos: unit pos offset=0x{o:x} ({:.1},{:.1},{:.1}) terrain_delta={:.2}",
                p.x, p.y, p.z, d
            ));
            return Some(p);
        }
        None
    }
}

/// World point under the mouse. The client pixel is ray-cast through the live
/// camera onto the terrain, so it cannot drift with whatever cached cursor
/// target the engine keeps. (The engine's MapView/Marker code only stores unit
/// *screen* positions plus a separate cursor view; the cached world fields are
/// not the live mouse point.)
pub unsafe fn cursor_ground() -> Option<Vec3> {
    unsafe {
        let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return None;
        }
        let real = REAL_GET_CURSOR_POS.load(Ordering::Relaxed);
        if real == 0 {
            return None;
        }
        let f: unsafe extern "system" fn(*mut Point) -> i32 = core::mem::transmute(real);
        let mut p = Point { x: 0, y: 0 };
        if f(&mut p) == 0 {
            return None;
        }
        let mut origin = Point { x: 0, y: 0 };
        if ClientToScreen(hwnd, &mut origin) == 0 {
            return None;
        }
        let (cx, cy) = ((p.x - origin.x) as f32, (p.y - origin.y) as f32);
        let cam = camera()?;
        let pw = PHYS_W.load(Ordering::Relaxed);
        let ph = PHYS_H.load(Ordering::Relaxed);
        if pw <= 0 || ph <= 0 {
            return None;
        }
        let hit = ray_terrain(&cam, [0, 0, pw, ph], cx, cy)?;
        // The heightfield is clamped at the map border, so a ray that leaves the map
        // (cursor above the horizon, past the edge) still "hits" the border. Reject
        // those, otherwise the fan parks on the world edge.
        let t = terrain()?;
        if hit.x < 0.0 || hit.y < 0.0 || hit.x > t.world_w || hit.y > t.world_h {
            return None;
        }
        Some(hit)
    }
}

/// Cursor world position (metres) from the MapView at `+0xe0/+0xe4`, either on
/// the view or on its model object.
pub unsafe fn cursor_world() -> Option<Vec3> {
    unsafe {
        let mv = mapview()?;
        let t = terrain()?;
        for base in mapview_bases(mv) {
            if base == 0 {
                continue;
            }
            let (Some(x), Some(north)) = (rd_f32(base + 0xe0), rd_f32(base + 0xe4)) else {
                continue;
            };
            if !x.is_finite() || !north.is_finite() || x < -32.0 || north < -32.0 {
                continue;
            }
            if x > t.world_w + 32.0 || north > t.world_h + 32.0 {
                continue;
            }
            return Some(Vec3 {
                x,
                y: north,
                z: terrain_height(&t, x, north),
            });
        }
        if NO_CURSOR_LOGGED.swap(1, Ordering::Relaxed) == 0 {
            let mut parts = String::new();
            for base in mapview_bases(mv) {
                if base == 0 {
                    continue;
                }
                parts.push_str(&format!(
                    " 0x{base:x}: x={} n={}",
                    rd_f32(base + 0xe0).unwrap_or(f32::NAN),
                    rd_f32(base + 0xe4).unwrap_or(f32::NAN)
                ));
            }
            log(&format!(
                "cokelos: cursor world invalid (map {:.0}x{:.0});{parts}",
                t.world_w, t.world_h
            ));
        }
        None
    }
}

/// Debug helper used by the mod's dump key.
pub unsafe fn debug_dump() {
    unsafe {
        let t = terrain();
        // Map-object field dump plus candidate arrays for the tile/feature grid
        // (needed for water and woods occlusion). Pointers whose committed region
        // is the size of a grid_w*grid_h byte/int array are the interesting ones.
        if let Some(map) = t {
            let mut hex = String::new();
            for o in (0..0x140).step_by(4) {
                hex.push_str(&format!(
                    "{o:03x}:{:08x} ",
                    rd_u32(map.map + o).unwrap_or(0)
                ));
            }
            log(&format!("cokelos probe: map {hex}"));
            let want = (map.grid_w as usize) * (map.grid_h as usize);
            for o in (0..0x140).step_by(4) {
                let Some(v) = rd_u32(map.map + o) else {
                    continue;
                };
                let p = v as usize;
                if p < 0x10000 {
                    continue;
                }
                let mut m: MemBasicInfo = core::mem::zeroed();
                if VirtualQuery(
                    p as *const c_void,
                    &mut m,
                    core::mem::size_of::<MemBasicInfo>(),
                ) == 0
                    || m.state != MEM_COMMIT
                {
                    continue;
                }
                let rs = m.region_size;
                if rs >= want && rs <= want * 8 + 0x10000 {
                    log(&format!(
                        "cokelos probe: candidate map+0x{o:x} -> 0x{p:x} region=0x{rs:x} protect=0x{:x} (grid bytes {want})",
                        m.protect
                    ));
                }
            }
        }
        let mv = mapview();
        let cam = camera();
        let rect = scene_rect();
        log(&format!(
            "cokelos probe: terrain={} mapview={} camera={} scene={:?}",
            t.map(|t| format!(
                "0x{:x} grid={}x{} world={:.0}x{:.0} heights=0x{:x}",
                t.map, t.grid_w, t.grid_h, t.world_w, t.world_h, t.height_ptr
            ))
            .unwrap_or_else(|| "none".into()),
            mv.map(|p| format!("0x{p:x}"))
                .unwrap_or_else(|| "none".into()),
            cam.map(|c| format!(
                "pos=({:.1},{:.1},{:.1}) dir=({:.2},{:.2},{:.2}) fov={:.1} aspect={:.3}",
                c.pos.x, c.pos.y, c.pos.z, c.dir.x, c.dir.y, c.dir.z, c.fov, c.aspect
            ))
            .unwrap_or_else(|| "none".into()),
            rect
        ));
        if let Some(u) = selected_unit() {
            let mut hex = String::new();
            for o in (0..0x80).step_by(4) {
                let v = rd_u32(u + o).unwrap_or(0);
                hex.push_str(&format!("{o:02x}:{v:08x} "));
            }
            log(&format!("cokelos probe: unit=0x{u:x} {hex}"));
            log(&format!("cokelos probe: unit pos={:?}", unit_pos(u)));
        }
        if let Some(c) = cursor_world() {
            log(&format!(
                "cokelos probe: cursor=({:.1},{:.1},{:.1})",
                c.x, c.y, c.z
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        // X east, Y north, Z up: looking north with up = +Z.
        Camera {
            pos: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            dir: Vec3 {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            },
            up: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            fov: 90.0,
            aspect: 1.0,
            near: 0.1,
            far: 1000.0,
        }
    }

    #[test]
    fn projection_centre_and_axes() {
        let r = [0, 0, 100, 100];
        let c = project(
            &cam(),
            r,
            Vec3 {
                x: 0.0,
                y: 10.0,
                z: 0.0,
            },
        )
        .unwrap();
        assert!((c.0 - 50.0).abs() < 0.1 && (c.1 - 50.0).abs() < 0.1);
        let right = project(
            &cam(),
            r,
            Vec3 {
                x: 1.0,
                y: 10.0,
                z: 0.0,
            },
        )
        .unwrap();
        assert!(right.0 > 50.0 && (right.1 - 50.0).abs() < 0.1);
        let up = project(
            &cam(),
            r,
            Vec3 {
                x: 0.0,
                y: 10.0,
                z: 1.0,
            },
        )
        .unwrap();
        assert!((up.0 - 50.0).abs() < 0.1 && up.1 < 50.0);
        assert!(
            project(
                &cam(),
                r,
                Vec3 {
                    x: 0.0,
                    y: -1.0,
                    z: 0.0
                }
            )
            .is_none()
        );
    }
}
