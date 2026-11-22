use crate::*;
use coke_host::{
    CokeRangePoint, CokeRangeResult, CokeTerrainTiles, RANGE_BLOCKED, RANGE_CANCEL,
    RANGE_CAP_HOOK_INSTALLED, RANGE_CAP_PROFILED, RANGE_CAP_TERRAIN_LOS, RANGE_CAP_TERRAIN_PAINT,
    RANGE_CLEAR, RANGE_ENABLE_KEYS, RANGE_FROM_CURSOR, RANGE_PAINT_TERRAIN, RANGE_SET_ORIGIN,
    RANGE_TOGGLE_TERRAIN_LOS, RANGE_WAITING,
};
use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

mod terrain_overlay;

const SCENE_TIMEOUT_MS: u32 = 650;
const QUERY_INTERVAL_MS: u32 = 100;
const OBSERVER_POSTURE: i32 = 2;
const NATIVE_OBSERVER_HEIGHT: f32 = 1.6;
const SOURCE_HEIGHTS: [f32; 4] = [0.25, 1.0, 2.0, 3.5];
const TARGET_POSTURE: i32 = 4;

#[derive(Clone, Copy)]
struct Native {
    flush: usize,
    trace: usize,
    ribbon: usize,
    ribbon_pool_off: usize,
    point_vtable: usize,
    owner_slot: usize,
    hud_draw: usize,
    label_draw: usize,
    label_width: usize,
    label_background: usize,
}

static NATIVE: OnceLock<Native> = OnceLock::new();
static DEPTH_CONTEXTS: OnceLock<Mutex<HashMap<usize, DepthReadback>>> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
static KEYS: AtomicBool = AtomicBool::new(false);
const INSTALL_UNSTARTED: u32 = 0;
const INSTALL_PENDING: u32 = 1;
const INSTALL_READY: u32 = 2;
const INSTALL_FAILED: u32 = 3;
static INSTALL_STATE: AtomicU32 = AtomicU32::new(INSTALL_UNSTARTED);
static ALLOWED_COMMAND_STATES: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy)]
struct DepthApi {
    gen_buffers: usize,
    delete_buffers: usize,
    bind_buffer: usize,
    buffer_data: usize,
    map_buffer: usize,
    unmap_buffer: usize,
    fence_sync: usize,
    client_wait_sync: usize,
    delete_sync: usize,
}

#[derive(Clone, Copy)]
struct DepthSample {
    eye: CokeRangePoint,
    ray: CokeRangePoint,
    projection: [f32; 16],
    depth_range: [f32; 2],
    pixel_x: i32,
    pixel_y: i32,
    client_y: i32,
    issued_at: u32,
    generation: u32,
}

#[derive(Clone, Copy)]
struct DepthHit {
    point: Option<CokeRangePoint>,
    sample: DepthSample,
}

#[derive(Clone, Copy)]
struct Aim {
    point: CokeRangePoint,
    pixel: Point,
    eye: CokeRangePoint,
}

struct DepthReadback {
    api: Option<DepthApi>,
    buffers: [u32; 2],
    fences: [usize; 2],
    samples: [Option<DepthSample>; 2],
    next: usize,
    last_hit: Option<DepthHit>,
    disabled: bool,
}

impl DepthReadback {
    fn new() -> Self {
        Self {
            api: load_depth_api(),
            buffers: [0; 2],
            fences: [0; 2],
            samples: [None; 2],
            next: 0,
            last_hit: None,
            disabled: false,
        }
    }
}
/// Delete resources while the context is still current. On subsequent binds
/// the context can resolve its own functions and create fresh readback slots.
pub(crate) unsafe fn release_depth_context(context: Hglrc) {
    if let Some(contexts) = DEPTH_CONTEXTS.get()
        && let Some(mut state) = contexts.lock().unwrap().remove(&(context as usize))
        && let Some(api) = state.api
    {
        unsafe { state.retire(api) };
    }
}

impl DepthReadback {
    unsafe fn retire(&mut self, api: DepthApi) {
        let delete_sync: unsafe extern "system" fn(*const c_void) =
            unsafe { core::mem::transmute(api.delete_sync) };
        let delete_buffers: unsafe extern "system" fn(i32, *const u32) =
            unsafe { core::mem::transmute(api.delete_buffers) };
        unsafe {
            for &fence in &self.fences {
                if fence != 0 {
                    delete_sync(fence as *const c_void);
                }
            }
            delete_buffers(self.buffers.len() as i32, self.buffers.as_ptr());
        }
        self.buffers = [0; 2];
        self.fences = [0; 2];
        self.samples = [None; 2];
        self.last_hit = None;
        self.disabled = true;
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    scene: None,
    generation: 0,
    seen_at: 0,
    can_start: false,
    unit_selected: false,
    source_height: SOURCE_HEIGHTS[0],
    pending: None,
    origin: None,
    result: None,
    last_trace: None,
    terrain_overlay: terrain_overlay::Overlay::new(),
});

#[derive(Clone, Copy, PartialEq, Eq)]
struct Scene {
    owner: usize,
    world: usize,
    map: usize,
}

#[derive(Clone, Copy)]
enum Origin {
    Cursor { since: u32, pixel: Point },
    Point(CokeRangePoint),
}

struct State {
    scene: Option<Scene>,
    generation: u32,
    seen_at: u32,
    can_start: bool,
    unit_selected: bool,
    source_height: f32,
    pending: Option<Origin>,
    origin: Option<CokeRangePoint>,
    result: Option<CokeRangeResult>,
    last_trace: Option<(u32, CokeRangeResult)>,
    terrain_overlay: terrain_overlay::Overlay,
}

impl State {
    fn active(&self) -> bool {
        self.pending.is_some() || self.origin.is_some()
    }

    fn recent(&self, now: u32) -> bool {
        self.scene.is_some() && now.wrapping_sub(self.seen_at) <= SCENE_TIMEOUT_MS
    }

    fn cancel(&mut self) {
        self.pending = None;
        self.origin = None;
        self.result = None;
        self.last_trace = None;
    }

    fn start(&mut self, origin: Origin) {
        self.cancel();
        self.pending = Some(origin);
    }
}

fn finite(p: CokeRangePoint) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
}

fn next_source_height(current: f32) -> f32 {
    SOURCE_HEIGHTS
        .iter()
        .copied()
        .find(|height| *height > current + 0.01)
        .unwrap_or(SOURCE_HEIGHTS[0])
}

fn in_terrain(p: CokeRangePoint, t: &Terrain) -> bool {
    finite(p) && p.x >= 0.0 && p.y >= 0.0 && p.x < t.world_w && p.y < t.world_h
}

fn focused() -> bool {
    unsafe {
        let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
        !hwnd.is_null() && GetForegroundWindow() == hwnd
    }
}

pub unsafe extern "system" fn control(action: u32, point: *const CokeRangePoint) -> i32 {
    if action == RANGE_ENABLE_KEYS {
        KEYS.store(true, Ordering::Release);
        REQUESTED.store(true, Ordering::Release);
        return 1;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return 0;
    };
    if action == RANGE_TOGGLE_TERRAIN_LOS {
        return i32::from(toggle_terrain_los(&mut state, unsafe { GetTickCount() }));
    }
    if action == RANGE_PAINT_TERRAIN {
        if !terrain_overlay::available() {
            return 0;
        }
        if unsafe { mod_span(point.cast::<CokeTerrainTiles>(), 1, false) }.is_none() {
            return 0;
        }
        let paint = unsafe { point.cast::<CokeTerrainTiles>().read() };
        if paint.count as usize > 65_536 || !paint.world_w.is_finite() || !paint.world_h.is_finite()
        {
            return 0;
        }
        let Some(scene) = state
            .scene
            .filter(|_| state.recent(unsafe { GetTickCount() }))
        else {
            return 0;
        };
        if paint.map as usize != scene.map {
            return 0;
        }
        let Some(terrain) = (unsafe { range_terrain() }) else {
            return 0;
        };
        if terrain.map != scene.map
            || (terrain.world_w - paint.world_w).abs() > 0.01
            || (terrain.world_h - paint.world_h).abs() > 0.01
        {
            return 0;
        }
        let tiles = if paint.count == 0 {
            Vec::new()
        } else {
            if unsafe { mod_span(paint.tiles, paint.count as usize, false) }.is_none() {
                return 0;
            }
            let src = unsafe { core::slice::from_raw_parts(paint.tiles, paint.count as usize) };
            if src.iter().any(|t| {
                ![t.x0, t.y0, t.x1, t.y1].iter().all(|v| v.is_finite())
                    || t.x0 < 0.0
                    || t.y0 < 0.0
                    || t.x1 <= t.x0
                    || t.y1 <= t.y0
                    || t.x1 > paint.world_w
                    || t.y1 > paint.world_h
                    || t.visible > 1
            }) {
                return 0;
            }
            src.to_vec()
        };
        state
            .terrain_overlay
            .set_tiles(scene.map, paint.world_w, paint.world_h, tiles);
        return 1;
    }
    if action == coke_host::RANGE_SET_SOURCE_HEIGHT {
        if unsafe { mod_span(point, 1, false) }.is_none() {
            return 0;
        }
        let height = unsafe { point.read() }.z;
        if !height.is_finite() || !(SOURCE_HEIGHTS[0]..=6.0).contains(&height) {
            return 0;
        }
        state.source_height = height;
        state.last_trace = None;
        return 1;
    }
    if action == RANGE_CANCEL {
        state.cancel();
        return 1;
    }
    if action != RANGE_FROM_CURSOR && action != RANGE_SET_ORIGIN {
        return 0;
    }
    REQUESTED.store(true, Ordering::Release);
    if !state.recent(unsafe { GetTickCount() }) || !state.can_start || !focused() {
        return 0;
    }
    let origin = if action == RANGE_FROM_CURSOR {
        let Some(pixel) = client_cursor() else {
            return 0;
        };
        Origin::Cursor {
            since: unsafe { GetTickCount() },
            pixel,
        }
    } else {
        if unsafe { mod_span(point, 1, false) }.is_none() {
            return 0;
        }
        let point = unsafe { point.read() };
        if !finite(point) {
            return 0;
        }
        Origin::Point(point)
    };
    state.start(origin);
    1
}

pub unsafe extern "system" fn read(out: *mut CokeRangeResult) -> i32 {
    if !focused() || unsafe { mod_span(out, 1, true) }.is_none() {
        return 0;
    }
    let Ok(state) = STATE.try_lock() else {
        return 0;
    };
    if !state.recent(unsafe { GetTickCount() }) || !state.active() {
        return 0;
    }
    let result = state.result.unwrap_or(CokeRangeResult {
        status: RANGE_WAITING,
        ..CokeRangeResult::default()
    });
    unsafe { out.write(result) };
    1
}

pub(crate) unsafe fn draw_terrain_status(width: i32, height: i32) {
    use terrain_overlay::HudStatus;

    struct StatusImage {
        status: HudStatus,
        pixels: Vec<u8>,
        width: i32,
        height: i32,
    }
    static IMAGE: Mutex<Option<StatusImage>> = Mutex::new(None);

    if !terrain_overlay::available() {
        return;
    }
    let status = STATE.try_lock().ok().and_then(|state| {
        state
            .recent(unsafe { GetTickCount() })
            .then(|| state.terrain_overlay.hud_status())
            .flatten()
    });
    let Some(status) = status else { return };
    let Ok(mut image) = IMAGE.try_lock() else {
        return;
    };
    if image.as_ref().is_none_or(|cached| cached.status != status) {
        let label = match status {
            HudStatus::Picking => "LOS: picking surface".to_owned(),
            HudStatus::Scanning(percent) => format!("LOS: scanning {percent}%"),
            HudStatus::Ready(s) => {
                format!(
                    "LOS: {}/{} visible, {} tiles",
                    s.visible, s.sampled, s.tiles
                )
            }
        };
        let (pixels, width, height) = rasterise_text(&label, 0x00ff_ffff, 2);
        *image = Some(StatusImage {
            status,
            pixels,
            width,
            height,
        });
    }
    if let Some(cached) = image.as_ref() {
        unsafe {
            draw_rgba_overlay(
                &cached.pixels,
                cached.width,
                cached.height,
                8,
                30,
                width,
                height,
            )
        };
    }
}

/// Capabilities of the matched build's native preview adapter.
pub unsafe extern "system" fn capabilities() -> u32 {
    let Some(p) = profile::profile() else {
        return 0;
    };
    let configured = p.range.is_some()
        && (p.los.is_some() || p.terrain.is_some())
        && p.scene.as_ref().is_some_and(|scene| scene.owner_ptr != 0);
    if !configured {
        return 0;
    }
    RANGE_CAP_PROFILED
        | if INSTALL_STATE.load(Ordering::Acquire) == INSTALL_READY {
            RANGE_CAP_HOOK_INSTALLED
        } else {
            0
        }
        | if terrain_overlay::available() {
            RANGE_CAP_TERRAIN_PAINT | RANGE_CAP_TERRAIN_LOS
        } else {
            0
        }
}

fn toggle_terrain_los(state: &mut State, now: u32) -> bool {
    if !terrain_overlay::available() || !focused() {
        return false;
    }
    if state.terrain_overlay.stop_scan() {
        return true;
    }
    if !state.recent(now) || !state.can_start {
        return false;
    }
    let Some(pixel) = client_cursor() else {
        return false;
    };
    state.terrain_overlay.start_scan(pixel, now);
    true
}

pub fn handle_message(msg: u32, value: usize, flags: isize) -> bool {
    if !KEYS.load(Ordering::Acquire) || !matches!(msg, WM_KEYDOWN | WM_CHAR | WM_RBUTTONDOWN) {
        return false;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return false;
    };
    if terrain_overlay::available()
        && msg == WM_KEYDOWN
        && value == b'T' as usize
        && state.recent(unsafe { GetTickCount() })
        && unsafe { GetKeyState(0x11) < 0 && GetKeyState(0x10) < 0 && GetKeyState(0x12) >= 0 }
    {
        if flags & (1 << 30) == 0 {
            toggle_terrain_los(&mut state, unsafe { GetTickCount() });
        }
        return true;
    }
    if terrain_overlay::available()
        && msg == WM_KEYDOWN
        && value == b'Y' as usize
        && state.recent(unsafe { GetTickCount() })
        && unsafe { GetKeyState(0x11) < 0 && GetKeyState(0x10) < 0 && GetKeyState(0x12) >= 0 }
    {
        if flags & (1 << 30) == 0
            && let Some(cursor) = client_cursor()
        {
            state.terrain_overlay.request(cursor);
        }
        return true;
    }
    if !state.recent(unsafe { GetTickCount() }) || !state.can_start {
        return false;
    }
    if state.active() && (msg == WM_RBUTTONDOWN || value == 27) {
        state.cancel();
        return msg == WM_CHAR;
    }
    if msg != WM_CHAR {
        return false;
    }
    if matches!(value, 0x48 | 0x68) {
        if !state.active() {
            return false;
        }
        if unsafe { GetKeyState(0x11) < 0 || GetKeyState(0x12) < 0 } {
            return false;
        }
        if flags & (1 << 30) == 0 {
            state.source_height = next_source_height(state.source_height);
            state.last_trace = None;
        }
        return true;
    }
    if !matches!(value, 0x54 | 0x74) {
        return false;
    }
    if unsafe { GetKeyState(0x11) < 0 || GetKeyState(0x12) < 0 } {
        return false;
    }
    if !state.active() && state.unit_selected && unsafe { GetKeyState(0x10) >= 0 } {
        return false;
    }
    if flags & (1 << 30) == 0 {
        if state.active() {
            state.cancel();
        } else if let Some(pixel) = client_cursor() {
            state.start(Origin::Cursor {
                since: unsafe { GetTickCount() },
                pixel,
            });
        }
    }
    true
}

fn rel_call(site: usize, target: usize) -> [u8; 5] {
    let offset = (target as u32).wrapping_sub((site as u32).wrapping_add(5));
    let mut call = [0xe8; 5];
    call[1..].copy_from_slice(&offset.to_le_bytes());
    call
}

/// Install on the main GL thread after the initial matched-profile frame.
/// The verified call site belongs to this thread's battle render function.
#[derive(Clone)]
struct CallPatch {
    site: usize,
    original: Vec<u8>,
    replacement: Vec<u8>,
}

impl CallPatch {
    fn relative(site: usize, original: usize, replacement: usize) -> Self {
        Self {
            site,
            original: rel_call(site, original).to_vec(),
            replacement: rel_call(site, replacement).to_vec(),
        }
    }

    fn imported(site: usize, slot: usize, replacement: usize) -> Self {
        let mut original = vec![0xff, 0x15];
        original.extend_from_slice(&(slot as u32).to_le_bytes());
        let mut replacement = rel_call(site, replacement).to_vec();
        replacement.push(0x90);
        Self {
            site,
            original,
            replacement,
        }
    }

    unsafe fn matches(&self) -> bool {
        let mut bytes = [0; 6];
        let actual = &mut bytes[..self.original.len()];
        (unsafe { rd_bytes(self.site, actual) }) && actual == self.original
    }
}

pub unsafe fn install_from_swap() {
    if !REQUESTED.load(Ordering::Acquire)
        || INSTALL_STATE.load(Ordering::Acquire) != INSTALL_UNSTARTED
    {
        return;
    }
    let Some(p) = profile::profile() else { return };
    let Some(r) = p.range else {
        INSTALL_STATE.store(INSTALL_FAILED, Ordering::Release);
        log("cokerange: unsupported profile (no range adapter)");
        return;
    };
    let hwnd = MAIN_WINDOW.load(Ordering::Acquire) as Hwnd;
    if unsafe {
        !current_window(hwnd)
            || GetWindowThreadProcessId(hwnd, core::ptr::null_mut()) != GetCurrentThreadId()
    } {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log(
                "cokerange: installation deferred until the game window's presentation thread swaps",
            );
        }
        return;
    }
    if INSTALL_STATE
        .compare_exchange(
            INSTALL_UNSTARTED,
            INSTALL_PENDING,
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    match unsafe { install(r, p.scene.as_ref().map_or(0, |s| s.owner_ptr)) } {
        Ok(()) => {
            INSTALL_STATE.store(INSTALL_READY, Ordering::Release);
            log("cokerange: native battle ribbon hook installed");
        }
        Err(reason) => {
            INSTALL_STATE.store(INSTALL_FAILED, Ordering::Release);
            log(&format!("cokerange: installation failed: {reason}"));
        }
    }
}

unsafe fn install(r: profile::RangePreview, owner_rva: u32) -> Result<(), &'static str> {
    const LAYOUT: &str = "unsupported call-site layout";
    let addr = |rva| profile::profile_addr(rva).ok_or(LAYOUT);
    ALLOWED_COMMAND_STATES.store(r.command_states, Ordering::Relaxed);
    let base = profile::module_base();
    let (code_rva, code_size) = unsafe { module_text_range(base) }.ok_or(LAYOUT)?;
    let in_code = |rva: u32, bytes: usize| {
        (rva as usize)
            .checked_sub(code_rva)
            .is_some_and(|offset| offset <= code_size.saturating_sub(bytes))
    };
    if !in_code(r.draw_call, 5)
        || !in_code(r.draw_flush, 1)
        || !in_code(r.trace, 1)
        || !in_code(r.ribbon, 1)
        || !in_code(r.hud_call, 5)
        || !in_code(r.hud_draw, 1)
        || !in_code(r.label_draw, 1)
        || !in_code(r.label_width, 1)
        || !in_code(r.label_background, 1)
    {
        return Err(LAYOUT);
    }
    let site = addr(r.draw_call)?;
    let hud_site = addr(r.hud_call)?;
    let native = Native {
        flush: addr(r.draw_flush)?,
        trace: addr(r.trace)?,
        ribbon: addr(r.ribbon)?,
        ribbon_pool_off: r.ribbon_pool_off as usize,
        point_vtable: addr(r.point_vtable)?,
        owner_slot: addr(owner_rva)?,
        hud_draw: addr(r.hud_draw)?,
        label_draw: addr(r.label_draw)?,
        label_width: addr(r.label_width)?,
        label_background: addr(r.label_background)?,
    };
    let mut sites = vec![
        CallPatch::relative(site, native.flush, battle_ribbons as *const () as usize),
        CallPatch::relative(hud_site, native.hud_draw, battle_hud as *const () as usize),
    ];
    let mut terrain_hooks = false;
    if config().range_terrain_overlay_test {
        if let Some(hooks) = terrain_overlay::hooks()
            .filter(|hooks| hooks.iter().all(|patch| unsafe { patch.matches() }))
        {
            sites.extend(hooks);
            terrain_hooks = true;
        } else {
            log("cokerange: terrain overlay unavailable on this executable build");
        }
    }
    if !sites.iter().all(|patch| unsafe { patch.matches() }) {
        return Err(LAYOUT);
    }
    sites.sort_unstable_by_key(|patch| patch.site);
    if sites
        .windows(2)
        .any(|p| p[0].site + p[0].original.len() > p[1].site)
    {
        return Err(LAYOUT);
    }
    let mut info = SystemInfo::default();
    unsafe { GetSystemInfo(&mut info) };
    let page_size = info.page_size as usize;
    if !page_size.is_power_of_two() {
        return Err(LAYOUT);
    }
    let mut pages = Vec::new();
    for patch in &sites {
        for address in [
            patch.site,
            patch
                .site
                .checked_add(patch.original.len() - 1)
                .ok_or(LAYOUT)?,
        ] {
            pages.push(address & !(page_size - 1));
        }
    }
    pages.sort_unstable();
    pages.dedup();

    // Both sites execute in the battle draw phase on the window's render
    // thread. Install between frames here, before calling the real swap.
    let mut protected = Vec::new();
    let result = (|| unsafe {
        for page in pages {
            let mut old = 0;
            if VirtualProtect(
                page as *mut c_void,
                page_size,
                PAGE_EXECUTE_READWRITE,
                &mut old,
            ) == 0
            {
                return Err("cannot make a call-site page writable");
            }
            protected.push((page, old));
        }
        if !sites.iter().all(|patch| patch.matches()) {
            return Err(LAYOUT);
        }
        NATIVE
            .set(native)
            .map_err(|_| "dispatch was already initialized")?;
        let write = |rollback| {
            for patch in &sites {
                let bytes = if rollback {
                    &patch.original
                } else {
                    &patch.replacement
                };
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), patch.site as *mut u8, bytes.len());
            }
        };
        let flush = || {
            let mut success = true;
            for patch in &sites {
                success &= FlushInstructionCache(
                    GetCurrentProcess(),
                    patch.site as *const c_void,
                    patch.original.len(),
                ) != 0;
            }
            success
        };
        write(false);
        if !flush() {
            write(true);
            if !flush() {
                log("cokerange: instruction cache flush failed during rollback");
            }
            return Err("cannot flush patched instructions");
        }
        Ok(())
    })();
    let mut restored = true;
    for (page, old) in protected {
        let mut unused = 0;
        restored &=
            unsafe { VirtualProtect(page as *mut c_void, page_size, old, &mut unused) } != 0;
    }
    if !restored {
        return Err("cannot restore call-site page protection");
    }
    if terrain_hooks && result.is_ok() {
        terrain_overlay::installed();
    }
    result
}

unsafe extern "C" fn battle_ribbons() {
    let Some(native) = NATIVE.get().copied() else {
        return;
    };
    unsafe {
        if INSTALL_STATE.load(Ordering::Acquire) == INSTALL_READY {
            update(native);
        }
        let flush: unsafe extern "C" fn() = core::mem::transmute(native.flush);
        flush();
    }
}

unsafe extern "thiscall" fn battle_hud(view: usize) {
    let Some(native) = NATIVE.get().copied() else {
        return;
    };
    let original: unsafe extern "thiscall" fn(usize) =
        unsafe { core::mem::transmute(native.hud_draw) };
    unsafe { original(view) };
    let mut result = CokeRangeResult::default();
    if INSTALL_STATE.load(Ordering::Acquire) != INSTALL_READY || unsafe { read(&mut result) } == 0 {
        return;
    }
    unsafe { draw_label(native, view, result) };
}

#[repr(C)]
struct NativeText {
    storage: [u8; 16],
    len: u32,
    capacity: u32,
}

unsafe fn draw_label(native: Native, view: usize, result: CokeRangeResult) {
    let Some(world) = (unsafe { range_world() }) else {
        return;
    };
    let Some(font) = (unsafe { rd_u32(world + 0x40) }).filter(|&p| p != 0) else {
        return;
    };
    let Some(font_height) =
        (unsafe { rd_u32(font as usize + 4) }).filter(|&h| (4..=64).contains(&h))
    else {
        return;
    };
    let Some(extent) = (unsafe { rd_u32(view + 0x14) }) else {
        return;
    };
    let width = (extent & 0xffff) as i32;
    let height = (extent >> 16) as i32;
    if width < 130 || height < 128 {
        return;
    }
    let get = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
    if get == 0 {
        return;
    }
    let get: unsafe extern "system" fn(u32, *mut i32) = unsafe { core::mem::transmute(get) };
    let mut viewport = [0i32; 4];
    unsafe { get(0x0ba2, viewport.as_mut_ptr()) };
    let [left, bottom, vp_width, vp_height] = viewport;
    if vp_width <= 0 || vp_height <= 0 {
        return;
    }
    if result.status == RANGE_WAITING {
        unsafe {
            print_label(
                native,
                font,
                width / 2,
                height - 36,
                font_height,
                "Aim at surface",
            )
        };
        return;
    }
    let from_bottom = PHYS_H.load(Ordering::Relaxed) - result.cursor_y - bottom;
    let x = ((result.cursor_x - left) * width / vp_width).clamp(65, width - 65);
    let y = (from_bottom * height / vp_height - 46).clamp(12, height - 24);
    let status = if result.status == RANGE_BLOCKED {
        "No Line of Sight"
    } else {
        "Ground Range"
    };
    let range = format!("Range {:.0}m", result.distance_m);
    unsafe {
        print_label(native, font, x, y + 16, font_height, status);
        print_label(native, font, x, y, font_height, &range);
    }
}

unsafe fn print_label(native: Native, font: u32, x: i32, y: i32, font_height: u32, text: &str) {
    let mut data = NativeText {
        storage: [0; 16],
        len: text.len() as u32,
        capacity: 15,
    };
    if text.len() > 15 {
        data.storage[..4].copy_from_slice(&(text.as_ptr() as u32).to_le_bytes());
        data.capacity = data.len;
    } else {
        data.storage[..text.len()].copy_from_slice(text.as_bytes());
    }
    let measure: unsafe extern "thiscall" fn(u32, *const NativeText) -> i32 =
        unsafe { core::mem::transmute(native.label_width) };
    let print: unsafe extern "thiscall" fn(u32, i32, i32, *const NativeText, i32, *const f32, f32) =
        unsafe { core::mem::transmute(native.label_draw) };
    let width = unsafe { measure(font, &data) };
    let black = [0.0f32; 3];
    let white = [1.0f32; 3];
    let bounds = [
        width + 4,
        font_height as i32 + 2,
        black.as_ptr() as i32,
        0.2f32.to_bits() as i32,
        native.label_background as i32,
    ];
    unsafe {
        asm!(
            "push dword ptr [eax + 12]",
            "push dword ptr [eax + 8]",
            "push dword ptr [eax + 4]",
            "push dword ptr [eax]",
            "call dword ptr [eax + 16]",
            "add esp, 16",
            in("eax") &bounds,
            in("ecx") x - width / 2 - 2,
            in("edx") y - 2,
            clobber_abi("C"),
        );
        print(font, x, y, &data, 0, white.as_ptr(), 1.0);
    }
}

unsafe fn cursor(
    terrain: &Terrain,
    generation: u32,
    pos: Point,
    now: u32,
    pressed_at: Option<u32>,
) -> Option<Aim> {
    let (eye, ray, projection, depth_range) = unsafe { battle_cursor_ray(pos.x, pos.y)? };
    let sample = DepthSample {
        eye,
        ray,
        projection,
        depth_range,
        pixel_x: pos.x,
        pixel_y: PHYS_H.load(Ordering::Relaxed) - pos.y - 1,
        client_y: pos.y,
        issued_at: now,
        generation,
    };
    let hit =
        unsafe { depth_surface(terrain, wglGetCurrentContext() as usize, sample, pressed_at) }?;
    Some(Aim {
        point: hit.point?,
        pixel: Point {
            x: hit.sample.pixel_x,
            y: hit.sample.client_y,
        },
        eye,
    })
}

fn client_cursor() -> Option<Point> {
    let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
    let get = REAL_GET_CURSOR_POS.load(Ordering::Relaxed);
    if hwnd.is_null() || get == 0 {
        return None;
    }
    let get: unsafe extern "system" fn(*mut Point) -> i32 = unsafe { core::mem::transmute(get) };
    let mut point = Point { x: 0, y: 0 };
    let mut client = Point { x: 0, y: 0 };
    if unsafe { get(&mut point) == 0 || ClientToScreen(hwnd, &mut client) == 0 } {
        return None;
    }
    Some(Point {
        x: point.x - client.x,
        y: point.y - client.y,
    })
}

unsafe fn battle_cursor_ray(
    x: i32,
    y: i32,
) -> Option<(CokeRangePoint, CokeRangePoint, [f32; 16], [f32; 2])> {
    let get = REAL_GL_GET_FLOATV.load(Ordering::Relaxed);
    let get_int = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
    if get == 0 || get_int == 0 {
        return None;
    }
    let get: unsafe extern "system" fn(u32, *mut f32) = unsafe { core::mem::transmute(get) };
    let get_int: unsafe extern "system" fn(u32, *mut i32) =
        unsafe { core::mem::transmute(get_int) };
    let mut viewport = [0i32; 4];
    let mut view = [0f32; 16];
    let mut projection = [0f32; 16];
    let mut depth_range = [0.0, 1.0];
    unsafe {
        get_int(0x0ba2, viewport.as_mut_ptr()); // GL_VIEWPORT
        get(0x0ba6, view.as_mut_ptr()); // GL_MODELVIEW_MATRIX
        get(0x0ba7, projection.as_mut_ptr()); // GL_PROJECTION_MATRIX
        get(0x0b70, depth_range.as_mut_ptr()); // GL_DEPTH_RANGE
    }
    let pixel_y = PHYS_H.load(Ordering::Relaxed) - y - 1;
    let [left, bottom, width, height] = viewport;
    if width <= 0
        || height <= 0
        || x < left
        || x >= left + width
        || pixel_y < bottom
        || pixel_y >= bottom + height
    {
        return None;
    }
    let ndc_x = ((x - left) as f32 + 0.5) * 2.0 / width as f32 - 1.0;
    let ndc_y = ((pixel_y - bottom) as f32 + 0.5) * 2.0 / height as f32 - 1.0;
    let (eye, ray) = unproject_ray(&view, &projection, ndc_x, ndc_y)?;
    Some((eye, ray, projection, depth_range))
}

/// Pick the nearest rendered surface asynchronously. A PBO plus a zero-timeout
/// fence check avoids stalling the battle render thread on GPU readback.
unsafe fn depth_surface(
    terrain: &Terrain,
    context: usize,
    sample: DepthSample,
    pressed_at: Option<u32>,
) -> Option<DepthHit> {
    if context == 0 {
        return None;
    }
    let get_int = REAL_GL_GET_INTEGERV.load(Ordering::Relaxed);
    let read_pixels = REAL_GL_READ_PIXELS.load(Ordering::Relaxed);
    if read_pixels == 0 || get_int == 0 {
        return None;
    }
    let get_int: unsafe extern "system" fn(u32, *mut i32) =
        unsafe { core::mem::transmute(get_int) };
    let read_pixels: unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut c_void) =
        unsafe { core::mem::transmute(read_pixels) };
    let contexts = DEPTH_CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut contexts = contexts.try_lock().ok()?;
    let readback = contexts.entry(context).or_insert_with(DepthReadback::new);
    let api = readback.api?;
    if readback.disabled {
        return None;
    }
    if readback.buffers[0] == 0 {
        let bind: unsafe extern "system" fn(u32, u32) =
            unsafe { core::mem::transmute(api.bind_buffer) };
        let generate: unsafe extern "system" fn(i32, *mut u32) =
            unsafe { core::mem::transmute(api.gen_buffers) };
        let buffer_data: unsafe extern "system" fn(u32, isize, *const c_void, u32) =
            unsafe { core::mem::transmute(api.buffer_data) };
        let mut previous = 0i32;
        unsafe { get_int(0x88ed, &mut previous) }; // GL_PIXEL_PACK_BUFFER_BINDING
        let mut buffers = [0u32; 2];
        unsafe { generate(2, buffers.as_mut_ptr()) };
        readback.buffers = buffers;
        if buffers.contains(&0) {
            unsafe { readback.retire(api) };
            return None;
        }
        for &buffer in &buffers {
            unsafe {
                bind(0x88eb, buffer); // GL_PIXEL_PACK_BUFFER
                buffer_data(0x88eb, 4, core::ptr::null(), 0x88e1); // GL_STREAM_READ
            }
        }
        unsafe { bind(0x88eb, previous as u32) };
    }

    let bind: unsafe extern "system" fn(u32, u32) =
        unsafe { core::mem::transmute(api.bind_buffer) };
    let map: unsafe extern "system" fn(u32, u32) -> *mut c_void =
        unsafe { core::mem::transmute(api.map_buffer) };
    let unmap: unsafe extern "system" fn(u32) -> u8 =
        unsafe { core::mem::transmute(api.unmap_buffer) };
    let client_wait: unsafe extern "system" fn(*const c_void, u32, u64) -> u32 =
        unsafe { core::mem::transmute(api.client_wait_sync) };
    let delete_sync: unsafe extern "system" fn(*const c_void) =
        unsafe { core::mem::transmute(api.delete_sync) };
    for slot in 0..readback.buffers.len() {
        let fence = readback.fences[slot];
        if fence == 0 {
            continue;
        }
        let status = unsafe { client_wait(fence as *const c_void, 0, 0) };
        if status == 0x911b {
            continue; // GL_TIMEOUT_EXPIRED: never wait on the render thread.
        }
        unsafe { delete_sync(fence as *const c_void) };
        readback.fences[slot] = 0;
        let pending = readback.samples[slot].take();
        if status != 0x911a && status != 0x911c {
            unsafe { readback.retire(api) };
            return None;
        }
        let Some(pending) = pending.filter(|pending| pending.generation == sample.generation)
        else {
            continue;
        };
        let mut previous = 0i32;
        unsafe { get_int(0x88ed, &mut previous) };
        unsafe { bind(0x88eb, readback.buffers[slot]) };
        let mapped = unsafe { map(0x88eb, 0x88b8) }; // GL_READ_ONLY
        let depth = if mapped.is_null() {
            None
        } else {
            let value = unsafe { mapped.cast::<f32>().read_unaligned() };
            (unsafe { unmap(0x88eb) } != 0).then_some(value)
        };
        unsafe { bind(0x88eb, previous as u32) };
        if let Some(depth) = depth.filter(|depth| depth.is_finite())
            && readback.last_hit.is_none_or(|previous| {
                previous.sample.generation != pending.generation
                    || pending.issued_at.wrapping_sub(previous.sample.issued_at) < 0x8000_0000
            })
        {
            readback.last_hit = Some(DepthHit {
                point: depth_point(terrain, pending, depth),
                sample: pending,
            });
        }
    }

    let slot = readback.next;
    if readback.fences[slot] == 0 {
        let _pack = unsafe { tight_pixel_store(true) }?;
        let fence_sync: unsafe extern "system" fn(u32, u32) -> *const c_void =
            unsafe { core::mem::transmute(api.fence_sync) };
        let mut previous = 0i32;
        unsafe { get_int(0x88ed, &mut previous) };
        unsafe { bind(0x88eb, readback.buffers[slot]) };
        let restore_read = crate::opengl::framebuffer_proc()
            .filter(|&(_, target)| target == GL_DRAW_FRAMEBUFFER)
            .and_then(|(bind_framebuffer, _)| {
                let draw = crate::opengl::gl_integer(0x8ca6) as u32; // GL_DRAW_FRAMEBUFFER_BINDING
                let read = crate::opengl::gl_integer(0x8caa) as u32; // GL_READ_FRAMEBUFFER_BINDING
                (draw != read).then(|| {
                    unsafe { bind_framebuffer(GL_READ_FRAMEBUFFER, draw) };
                    (bind_framebuffer, read)
                })
            });
        unsafe {
            read_pixels(
                sample.pixel_x,
                sample.pixel_y,
                1,
                1,
                0x1902,                // GL_DEPTH_COMPONENT
                0x1406,                // GL_FLOAT
                core::ptr::null_mut(), // byte offset zero in the bound PBO
            )
        };
        if let Some((bind_framebuffer, read)) = restore_read {
            unsafe { bind_framebuffer(GL_READ_FRAMEBUFFER, read) };
        }
        let fence = unsafe { fence_sync(0x9117, 0) }; // GL_SYNC_GPU_COMMANDS_COMPLETE
        unsafe { bind(0x88eb, previous as u32) };
        if fence.is_null() {
            unsafe { readback.retire(api) };
            return None;
        }
        readback.fences[slot] = fence as usize;
        readback.samples[slot] = Some(sample);
        readback.next = (slot + 1) % readback.buffers.len();
    }
    readback.last_hit.filter(|hit| {
        let old = hit.sample;
        old.generation == sample.generation
            && sample.issued_at.wrapping_sub(old.issued_at) <= QUERY_INTERVAL_MS
            // The point was reconstructed with the matrices saved when its
            // depth was read. Hover follows that completed sample, while
            // origin selection still requires the exact press pixel.
            && pressed_at.is_none_or(|since| {
                old.pixel_x == sample.pixel_x
                    && old.pixel_y == sample.pixel_y
                    && old.client_y == sample.client_y
                    && old.issued_at.wrapping_sub(since) < 0x8000_0000
            })
    })
}

fn same_point(a: CokeRangePoint, b: CokeRangePoint) -> bool {
    a.x == b.x && a.y == b.y && a.z == b.z
}

fn load_depth_api() -> Option<DepthApi> {
    let resolve = |core: &'static [u8], arb: &'static [u8]| {
        let pointer = gl_proc(core);
        if pointer != 0 { pointer } else { gl_proc(arb) }
    };
    let api = DepthApi {
        gen_buffers: resolve(b"glGenBuffers\0", b"glGenBuffersARB\0"),
        delete_buffers: resolve(b"glDeleteBuffers\0", b"glDeleteBuffersARB\0"),
        bind_buffer: resolve(b"glBindBuffer\0", b"glBindBufferARB\0"),
        buffer_data: resolve(b"glBufferData\0", b"glBufferDataARB\0"),
        map_buffer: resolve(b"glMapBuffer\0", b"glMapBufferARB\0"),
        unmap_buffer: resolve(b"glUnmapBuffer\0", b"glUnmapBufferARB\0"),
        fence_sync: resolve(b"glFenceSync\0", b"glFenceSync\0"),
        client_wait_sync: resolve(b"glClientWaitSync\0", b"glClientWaitSync\0"),
        delete_sync: resolve(b"glDeleteSync\0", b"glDeleteSync\0"),
    };
    (api.gen_buffers != 0
        && api.delete_buffers != 0
        && api.bind_buffer != 0
        && api.buffer_data != 0
        && api.map_buffer != 0
        && api.unmap_buffer != 0
        && api.fence_sync != 0
        && api.client_wait_sync != 0
        && api.delete_sync != 0)
        .then_some(api)
}

fn depth_point(terrain: &Terrain, sample: DepthSample, depth: f32) -> Option<CokeRangePoint> {
    let [near, far] = sample.depth_range;
    // Clear depth is 1 even when the scene uses a restricted depth range.
    if !depth.is_finite() || depth >= 1.0 || near == far {
        return None;
    }
    let normalized = (depth - near) / (far - near);
    if !(0.0..=1.0).contains(&normalized) {
        return None;
    }
    let ndc_z = normalized * 2.0 - 1.0;
    let denominator = ndc_z + sample.projection[10];
    if denominator.abs() < 1e-6 {
        return None;
    }
    // The camera-space ray has z=-1, so this is its distance parameter.
    let distance = sample.projection[14] / denominator;
    if !distance.is_finite() || distance <= 0.0 {
        return None;
    }
    let mut hit = CokeRangePoint {
        x: sample.eye.x + sample.ray.x * distance,
        y: sample.eye.y + sample.ray.y * distance,
        z: sample.eye.z + sample.ray.z * distance,
    };
    if !in_terrain(hit, terrain) {
        return None;
    }
    let ground = unsafe { terrain_height(terrain, hit.x, hit.y) };
    if !ground.is_finite() || hit.z < ground - 0.5 {
        return None;
    }
    if hit.z < ground {
        hit.z = ground;
    }
    Some(hit)
}

// The battle pass uses a rigid model-view and an OpenGL perspective frustum.
// Invert the view rotation by transposing it, preserving the native viewport.
fn unproject_ray(
    view: &[f32; 16],
    proj: &[f32; 16],
    x: f32,
    y: f32,
) -> Option<(CokeRangePoint, CokeRangePoint)> {
    if proj[11] != -1.0 || proj[0] <= 0.0 || proj[5] <= 0.0 || view[15] != 1.0 {
        return None;
    }
    let rotate_back = |x, y, z| CokeRangePoint {
        x: view[0] * x + view[1] * y + view[2] * z,
        y: view[4] * x + view[5] * y + view[6] * z,
        z: view[8] * x + view[9] * y + view[10] * z,
    };
    let origin = rotate_back(-view[12], -view[13], -view[14]);
    let direction = rotate_back((x + proj[8]) / proj[0], (y + proj[9]) / proj[5], -1.0);
    (finite(origin) && finite(direction)).then_some((origin, direction))
}

unsafe fn trace_ground(
    terrain: &Terrain,
    eye: CokeRangePoint,
    ray: CokeRangePoint,
) -> Option<CokeRangePoint> {
    let horizontal = |at: f32, scale: f32, limit: f32| {
        if scale.abs() < 1e-6 {
            if !(0.0..limit).contains(&at) {
                return None;
            }
            return Some((0.0, f32::INFINITY));
        }
        let a = -at / scale;
        let b = (limit - at) / scale;
        Some((a.min(b), a.max(b)))
    };
    let (x0, x1) = horizontal(eye.x, ray.x, terrain.world_w)?;
    let (y0, y1) = horizontal(eye.y, ray.y, terrain.world_h)?;
    let mut lo = x0.max(y0).max(0.0);
    let end = x1.min(y1).min(5000.0);
    let at = |distance: f32| CokeRangePoint {
        x: eye.x + ray.x * distance,
        y: eye.y + ray.y * distance,
        z: eye.z + ray.z * distance,
    };
    let height = |p: CokeRangePoint| unsafe { terrain_height(terrain, p.x, p.y) };
    if lo >= end || at(lo).z <= height(at(lo)) {
        return None;
    }
    while lo < end {
        let mut hi = (lo + 2.0).min(end);
        let point = at(hi);
        if point.z <= height(point) {
            for _ in 0..10 {
                let mid = (lo + hi) * 0.5;
                let point = at(mid);
                if point.z > height(point) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let mut hit = at(hi);
            hit.z = height(hit);
            return Some(hit);
        }
        lo = hi;
    }
    None
}

unsafe fn update(native: Native) {
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    let now = unsafe { GetTickCount() };
    let Some(world) = (unsafe { range_world() }) else {
        state.terrain_overlay.clear();
        state.scene = None;
        state.can_start = false;
        state.cancel();
        return;
    };
    let Some(owner) = (unsafe { rd_u32(native.owner_slot) }).filter(|&p| p != 0) else {
        state.terrain_overlay.clear();
        state.scene = None;
        state.can_start = false;
        state.cancel();
        return;
    };
    let Some(terrain) = (unsafe { range_terrain() }) else {
        state.terrain_overlay.clear();
        state.scene = None;
        state.can_start = false;
        state.cancel();
        return;
    };
    let scene = Scene {
        owner: owner as usize,
        world,
        map: terrain.map,
    };
    let command = unsafe { rd_u32(world + 0x38) };
    let command_allowed = command.is_some_and(command_state_allowed);
    let scene_changed = state.scene != Some(scene);
    let preview_expired = scene_changed || !state.recent(now);
    if preview_expired {
        state.generation = state.generation.wrapping_add(1);
    }
    if preview_expired || !focused() || !command_allowed {
        state.cancel();
    }
    state.scene = Some(scene);
    state.seen_at = now;
    if terrain_overlay::available() {
        unsafe { state.terrain_overlay.update(&terrain, scene_changed) };
    }
    state.can_start = command_allowed;
    state.unit_selected = unsafe { rd_u32(world + 0x8a0) } != Some(0);
    let generation = state.generation;
    if let Some((pixel, since)) = state.terrain_overlay.scan_pick() {
        if now.wrapping_sub(since) >= SCENE_TIMEOUT_MS {
            state.terrain_overlay.stop_scan();
        } else if let Some(aim) = unsafe { cursor(&terrain, generation, pixel, now, Some(since)) } {
            state.terrain_overlay.begin_scan(&terrain, aim.point);
        }
        return;
    }
    unsafe {
        state
            .terrain_overlay
            .scan_frame(&terrain, native, scene.owner)
    };
    if !state.active() {
        return;
    }
    state.result = None;
    let (pixel, pressed_at) = match state.pending {
        Some(Origin::Cursor { since, pixel }) => {
            if now.wrapping_sub(since) >= SCENE_TIMEOUT_MS {
                state.cancel();
                return;
            }
            (pixel, Some(since))
        }
        _ => {
            let Some(pixel) = client_cursor() else { return };
            (pixel, None)
        }
    };
    let Some(Aim {
        point: target,
        pixel,
        eye,
    }) = (unsafe { cursor(&terrain, generation, pixel, now, pressed_at) })
    else {
        return;
    };
    if let Some(origin) = state.pending.take() {
        state.origin = match origin {
            Origin::Cursor { .. } => Some(target),
            Origin::Point(point) => Some(point).filter(|&p| in_terrain(p, &terrain)),
        };
    }
    let Some(origin) = state.origin else { return };
    let Some(mut result) = (unsafe {
        sample(
            native,
            scene.owner,
            origin,
            target,
            state.source_height,
            now,
            &mut state,
        )
    }) else {
        return;
    };
    result.cursor_x = pixel.x;
    result.cursor_y = pixel.y;
    state.result = Some(result);
    if result.distance_m >= 0.25 {
        unsafe { draw(native, scene.owner, result, now, state.source_height, eye) };
    }
}

fn command_state_allowed(command: u32) -> bool {
    command < u32::BITS && ALLOWED_COMMAND_STATES.load(Ordering::Relaxed) & (1 << command) != 0
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativePoint {
    vtable: usize,
    point: CokeRangePoint,
}

fn between(a: CokeRangePoint, b: CokeRangePoint, t: f32) -> CokeRangePoint {
    CokeRangePoint {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        z: a.z + (b.z - a.z) * t,
    }
}

unsafe fn sample(
    native: Native,
    owner: usize,
    origin: CokeRangePoint,
    target: CokeRangePoint,
    source_height: f32,
    now: u32,
    state: &mut State,
) -> Option<CokeRangeResult> {
    if let Some((at, result)) = state.last_trace
        && now.wrapping_sub(at) < QUERY_INTERVAL_MS
        && same_point(result.target, target)
    {
        return Some(CokeRangeResult {
            status: result.status,
            origin,
            target,
            distance_m: (target.x - origin.x).hypot(target.y - origin.y),
            visible_fraction: result.visible_fraction,
            ..CokeRangeResult::default()
        });
    }
    let context = unsafe { trace_context(owner) }?;
    let distance = (target.x - origin.x).hypot(target.y - origin.y);
    // Retain the native stance offset and add only the requested height delta.
    // The public result continues to report the ground origin for ABI stability.
    let trace_origin = CokeRangePoint {
        z: origin.z + (source_height - NATIVE_OBSERVER_HEIGHT),
        ..origin
    };
    let mut visible_fraction = 1.0;
    let mut status = RANGE_CLEAR;
    if distance >= 0.25 && !unsafe { trace(native, context, trace_origin, target) } {
        status = RANGE_BLOCKED;
        let mut blocked = 1.0;
        visible_fraction = 0.0;
        // Match the native preview's roughly three-metre refinement bound.
        while (blocked - visible_fraction) * distance > 3.0 {
            let middle = (visible_fraction + blocked) * 0.5;
            if unsafe {
                trace(
                    native,
                    context,
                    trace_origin,
                    between(trace_origin, target, middle),
                )
            } {
                visible_fraction = middle;
            } else {
                blocked = middle;
            }
        }
    }
    let result = CokeRangeResult {
        status,
        origin,
        target,
        distance_m: distance,
        visible_fraction,
        ..CokeRangeResult::default()
    };
    state.last_trace = Some((now, result));
    Some(result)
}

unsafe fn trace_context(owner: usize) -> Option<usize> {
    let context = unsafe { rd_u32(owner + 0x1c)? } as usize;
    unsafe { committed(context, 0x54) }.then_some(context)
}

unsafe fn trace(native: Native, context: usize, a: CokeRangePoint, b: CokeRangePoint) -> bool {
    type Trace = unsafe extern "thiscall" fn(
        usize,
        *const NativePoint,
        i32,
        usize,
        *const NativePoint,
        i32,
        usize,
        i32,
        usize,
        usize,
        usize,
    ) -> u8;
    let run: Trace = unsafe { core::mem::transmute(native.trace) };
    let a = NativePoint {
        vtable: native.point_vtable,
        point: a,
    };
    let b = NativePoint {
        vtable: native.point_vtable,
        point: b,
    };
    unsafe {
        run(
            context,
            &a,
            OBSERVER_POSTURE,
            0,
            &b,
            TARGET_POSTURE,
            0,
            0,
            0,
            0,
            0,
        ) != 0
    }
}

fn ribbon_width(cfg: &Config, eye: CokeRangePoint, a: CokeRangePoint, b: CokeRangePoint) -> f32 {
    let base = cfg.range_line_width;
    let scale_distance = cfg.range_line_scale_distance;
    if scale_distance == 0.0 {
        return base;
    }
    let distance = |p: CokeRangePoint| (p.x - eye.x).hypot(p.y - eye.y).hypot(p.z - eye.z);
    // The native ribbon accepts one width. Size it for the farther endpoint
    // and use the same width across the visible and blocked portions.
    let scale = (distance(a).max(distance(b)) / scale_distance).max(1.0);
    (base * scale).min(cfg.range_line_max_width.max(base))
}

unsafe fn draw(
    native: Native,
    owner: usize,
    result: CokeRangeResult,
    now: u32,
    source_height: f32,
    eye: CokeRangePoint,
) {
    if unsafe { rd_u32(owner + native.ribbon_pool_off) }.is_none_or(|p| p == 0) {
        return;
    }
    let a = CokeRangePoint {
        z: result.origin.z + source_height,
        ..result.origin
    };
    let b = result.target;
    let width = ribbon_width(config(), eye, a, b);
    let alpha = |period: u32, base| {
        let t = now % period;
        base + 0.2 * (t.min(period - t) as f32 / (period / 2) as f32)
    };
    unsafe {
        if result.status == RANGE_CLEAR {
            ribbon(native, owner, 5, alpha(700, 0.5), a, b, width);
        } else {
            let middle = between(a, b, result.visible_fraction);
            if result.visible_fraction > 0.0 {
                ribbon(native, owner, 14, alpha(700, 0.5), a, middle, width);
            }
            ribbon(native, owner, 4, alpha(300, 0.7), b, middle, width);
        }
    }
}

#[repr(C)]
struct RibbonArgs {
    colour: u32,
    from_height: i32,
    to_height: i32,
    opacity: f32,
    reserved: u32,
    from: NativePoint,
    to: NativePoint,
}

unsafe fn ribbon(
    native: Native,
    owner: usize,
    colour: u32,
    opacity: f32,
    a: CokeRangePoint,
    b: CokeRangePoint,
    width: f32,
) {
    let args = RibbonArgs {
        colour,
        from_height: -1,
        to_height: -1,
        opacity,
        reserved: 0,
        from: NativePoint {
            vtable: native.point_vtable,
            point: a,
        },
        to: NativePoint {
            vtable: native.point_vtable,
            point: b,
        },
    };
    const { assert!(core::mem::size_of::<RibbonArgs>() == 0x34) };
    // This engine routine passes ribbon width in XMM2 and cleans up its
    // thirteen stack words. ECX carries the same owner as native Target.
    unsafe {
        asm!(
            "push dword ptr [edx + 48]",
            "push dword ptr [edx + 44]",
            "push dword ptr [edx + 40]",
            "push dword ptr [edx + 36]",
            "push dword ptr [edx + 32]",
            "push dword ptr [edx + 28]",
            "push dword ptr [edx + 24]",
            "push dword ptr [edx + 20]",
            "push dword ptr [edx + 16]",
            "push dword ptr [edx + 12]",
            "push dword ptr [edx + 8]",
            "push dword ptr [edx + 4]",
            "push dword ptr [edx]",
            "call eax",
            in("eax") native.ribbon,
            in("ecx") owner,
            in("edx") &args,
            in("xmm2") width,
            clobber_abi("C"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_width_tracks_the_farther_endpoint_and_respects_the_cap() {
        let mut cfg = Config {
            range_line_width: 1.0,
            range_line_max_width: 3.0,
            ..Config::default()
        };
        let eye = CokeRangePoint {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let near = CokeRangePoint { z: 100.0, ..eye };
        let far = CokeRangePoint {
            x: 120.0,
            y: 160.0,
            ..eye
        };
        assert_eq!(ribbon_width(&cfg, eye, near, eye), 1.0);
        assert_eq!(ribbon_width(&cfg, eye, near, far), 2.0);
        assert_eq!(ribbon_width(&cfg, eye, far, near), 2.0);
        assert_eq!(
            ribbon_width(&cfg, eye, CokeRangePoint { z: 1000.0, ..eye }, near),
            3.0
        );
        cfg.range_line_scale_distance = 0.0;
        assert_eq!(ribbon_width(&cfg, eye, near, far), 1.0);
    }

    #[test]
    fn projects_through_a_translated_battle_camera() {
        let view = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., -10., -20., -30., 1.,
        ];
        let proj = [
            2., 0., 0., 0., 0., 4., 0., 0., 0., 0., -1., -1., 0., 0., -1., 0.,
        ];
        let (eye, ray) = unproject_ray(&view, &proj, 1., -1.).unwrap();
        assert_eq!([eye.x, eye.y, eye.z], [10., 20., 30.]);
        assert_eq!([ray.x, ray.y, ray.z], [0.5, -0.25, -1.]);
    }

    #[test]
    fn relative_call_wraps_across_the_full_x86_address_space() {
        assert_eq!(
            rel_call(0x400000, 0xf0000000),
            [0xe8, 0xfb, 0xff, 0xbf, 0xef]
        );
    }
}
