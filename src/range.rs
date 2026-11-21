use crate::*;
use coke_host::{
    CokeRangePoint, CokeRangeResult, RANGE_BLOCKED, RANGE_CANCEL, RANGE_CLEAR, RANGE_ENABLE_KEYS,
    RANGE_FROM_CURSOR, RANGE_SET_ORIGIN, RANGE_WAITING,
};
use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

const SCENE_TIMEOUT_MS: u32 = 650;
const QUERY_INTERVAL_MS: u32 = 100;
const CURSOR_SAMPLE_INTERVAL_MS: u32 = 16;
const OBSERVER_POSTURE: i32 = 2;
const NATIVE_OBSERVER_HEIGHT: f32 = 1.6;
const SOURCE_HEIGHTS: [f32; 4] = [0.25, 1.0, 2.0, 3.5];
const TARGET_POSTURE: i32 = 4;

#[derive(Clone, Copy)]
struct Native {
    flush: usize,
    trace: usize,
    ribbon: usize,
    point_vtable: usize,
    owner_slot: usize,
    hud_draw: usize,
    label_draw: usize,
    label_width: usize,
    label_background: usize,
}

static NATIVE: OnceLock<Native> = OnceLock::new();
static DEPTH_API: OnceLock<Option<DepthApi>> = OnceLock::new();
static DEPTH_READBACK: OnceLock<Mutex<DepthReadback>> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
static KEYS: AtomicBool = AtomicBool::new(false);
static INSTALL_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct DepthApi {
    gen_buffers: usize,
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
}

struct DepthReadback {
    context: usize,
    buffers: [u32; 2],
    fences: [usize; 2],
    samples: [Option<DepthSample>; 2],
    next: usize,
    last_hit: Option<CokeRangePoint>,
    disabled: bool,
}

impl DepthReadback {
    const fn new() -> Self {
        Self {
            context: 0,
            buffers: [0; 2],
            fences: [0; 2],
            samples: [None; 2],
            next: 0,
            last_hit: None,
            disabled: false,
        }
    }
}
static STATE: Mutex<State> = Mutex::new(State {
    scene: None,
    seen_at: 0,
    can_start: false,
    unit_selected: false,
    source_height: SOURCE_HEIGHTS[0],
    cursor_sample: None,
    pending: None,
    origin: None,
    result: None,
    last_trace: None,
});

#[derive(Clone, Copy, PartialEq, Eq)]
struct Scene {
    owner: usize,
    world: usize,
    map: usize,
}

#[derive(Clone, Copy)]
enum Origin {
    Cursor,
    Point(CokeRangePoint),
}

struct State {
    scene: Option<Scene>,
    seen_at: u32,
    can_start: bool,
    unit_selected: bool,
    source_height: f32,
    cursor_sample: Option<(u32, CokeRangePoint)>,
    pending: Option<Origin>,
    origin: Option<CokeRangePoint>,
    result: Option<CokeRangeResult>,
    last_trace: Option<(u32, CokeRangeResult)>,
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
        self.cursor_sample = None;
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

fn cursor_sample_interval() -> u32 {
    if DEPTH_API.get().is_some_and(Option::is_none) {
        return QUERY_INTERVAL_MS;
    }
    if let Some(readback) = DEPTH_READBACK.get()
        && let Ok(state) = readback.try_lock()
        && state.disabled
    {
        return QUERY_INTERVAL_MS;
    }
    CURSOR_SAMPLE_INTERVAL_MS
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
    if action == coke_host::RANGE_SET_SOURCE_HEIGHT {
        if point.is_null() || !point.is_aligned() {
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
        Origin::Cursor
    } else {
        if point.is_null() || !point.is_aligned() {
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
    if out.is_null() || !out.is_aligned() || !focused() {
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

pub fn handle_message(msg: u32, value: usize, flags: isize) -> bool {
    if !KEYS.load(Ordering::Acquire) || (msg != WM_CHAR && msg != WM_RBUTTONDOWN) {
        return false;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return false;
    };
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
        } else {
            state.start(Origin::Cursor);
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
pub unsafe fn install_from_swap() {
    if !REQUESTED.load(Ordering::Acquire)
        || NATIVE.get().is_some()
        || INSTALL_FAILED.load(Ordering::Relaxed)
    {
        return;
    }
    let Some(p) = profile::profile() else { return };
    let Some(r) = p.range else {
        INSTALL_FAILED.store(true, Ordering::Relaxed);
        log("cokerange: no native range adapter for this profile");
        return;
    };
    let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;
    if hwnd.is_null()
        || unsafe { GetWindowThreadProcessId(hwnd, core::ptr::null_mut()) != GetCurrentThreadId() }
    {
        return;
    }
    if unsafe { install(r, p.scene.as_ref().map_or(0, |s| s.owner_ptr)) }.is_none() {
        INSTALL_FAILED.store(true, Ordering::Relaxed);
        log("cokerange: native range hook unavailable (code or page validation failed)");
    }
}

unsafe fn install(r: profile::RangePreview, owner_rva: u32) -> Option<()> {
    let base = profile::module_base();
    let (code_rva, code_size) = unsafe { module_text_range(base)? };
    let in_code = |rva: u32, bytes: usize| {
        (rva as usize) >= code_rva && (rva as usize) + bytes <= code_rva + code_size
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
        return None;
    }
    let site = profile::profile_addr(r.draw_call)?;
    let hud_site = profile::profile_addr(r.hud_call)?;
    let native = Native {
        flush: profile::profile_addr(r.draw_flush)?,
        trace: profile::profile_addr(r.trace)?,
        ribbon: profile::profile_addr(r.ribbon)?,
        point_vtable: profile::profile_addr(r.point_vtable)?,
        owner_slot: profile::profile_addr(owner_rva)?,
        hud_draw: profile::profile_addr(r.hud_draw)?,
        label_draw: profile::profile_addr(r.label_draw)?,
        label_width: profile::profile_addr(r.label_width)?,
        label_background: profile::profile_addr(r.label_background)?,
    };
    let sites = [
        (site, native.flush, battle_ribbons as *const () as usize),
        (hud_site, native.hud_draw, battle_hud as *const () as usize),
    ];
    for (site, old, _) in sites {
        let mut bytes = [0; 5];
        if !unsafe { rd_bytes(site, &mut bytes) } || bytes != rel_call(site, old) {
            return None;
        }
    }
    // Both sites are in the same page on the verified build. Protect them
    // together so their installation succeeds or fails as a pair.
    let first = site.min(hud_site);
    let end = site.max(hud_site).checked_add(5)?;
    if sites[0].0.abs_diff(sites[1].0) < 5 || first / 4096 != (end - 1) / 4096 {
        return None;
    }
    let span = end - first;
    let code = first as *mut c_void;
    let mut protection = 0;
    unsafe {
        if VirtualProtect(code, span, PAGE_EXECUTE_READWRITE, &mut protection) == 0 {
            return None;
        }
        let mut unused = 0;
        if NATIVE.set(native).is_err() {
            VirtualProtect(code, span, protection, &mut unused);
            return None;
        }
        for (site, _, replacement) in sites {
            let bytes = rel_call(site, replacement);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), site as *mut u8, bytes.len());
        }
        if FlushInstructionCache(GetCurrentProcess(), code, span) == 0 {
            log("cokerange: could not flush the patched instruction cache");
        }
        if VirtualProtect(code, span, protection, &mut unused) == 0 {
            log("cokerange: could not restore battle code page protection");
        }
    }
    log("cokerange: native battle ribbon hook installed");
    Some(())
}

unsafe extern "C" fn battle_ribbons() {
    let Some(native) = NATIVE.get().copied() else {
        return;
    };
    unsafe {
        update(native);
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
    if unsafe { read(&mut result) } == 0 {
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
    let Some(world) = (unsafe { world() }) else {
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
                "Aim at ground",
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
    state: &mut State,
    now: u32,
) -> Option<(CokeRangePoint, Point, Option<CokeRangePoint>)> {
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
    let pos = Point {
        x: point.x - client.x,
        y: point.y - client.y,
    };
    let (eye, ray, projection) = unsafe { battle_cursor_ray(pos.x, pos.y)? };
    let hit = if let Some((at, hit)) = state.cursor_sample
        && now.wrapping_sub(at) < cursor_sample_interval()
    {
        hit
    } else {
        let hit = unsafe { depth_surface(terrain, pos.x, pos.y, eye, ray, &projection) }
            .or_else(|| unsafe { trace_ground(terrain, eye, ray) })?;
        state.cursor_sample = Some((now, hit));
        hit
    };
    let ground_origin = if matches!(state.pending, Some(Origin::Cursor)) {
        unsafe { trace_ground(terrain, eye, ray) }.filter(|point| in_terrain(*point, terrain))
    } else {
        None
    };
    Some((hit, pos, ground_origin))
}

unsafe fn battle_cursor_ray(x: i32, y: i32) -> Option<(CokeRangePoint, CokeRangePoint, [f32; 16])> {
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
    unsafe {
        get_int(0x0ba2, viewport.as_mut_ptr()); // GL_VIEWPORT
        get(0x0ba6, view.as_mut_ptr()); // GL_MODELVIEW_MATRIX
        get(0x0ba7, projection.as_mut_ptr()); // GL_PROJECTION_MATRIX
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
    let ndc_x = (x - left) as f32 * 2.0 / width as f32 - 1.0;
    let ndc_y = (pixel_y - bottom) as f32 * 2.0 / height as f32 - 1.0;
    let (eye, ray) = unproject_ray(&view, &projection, ndc_x, ndc_y)?;
    Some((eye, ray, projection))
}

/// Pick the nearest rendered surface asynchronously. A PBO plus a zero-timeout
/// fence check avoids stalling the battle render thread on GPU readback.
unsafe fn depth_surface(
    terrain: &Terrain,
    x: i32,
    y: i32,
    eye: CokeRangePoint,
    ray: CokeRangePoint,
    projection: &[f32; 16],
) -> Option<CokeRangePoint> {
    let api = DEPTH_API
        .get_or_init(|| unsafe { load_depth_api() })
        .as_ref()
        .copied()?;
    let context_fn = REAL_WGL_GET_CURRENT_CONTEXT.load(Ordering::Relaxed);
    let context = if context_fn == 0 {
        0
    } else {
        let f: unsafe extern "system" fn() -> Hglrc = unsafe { core::mem::transmute(context_fn) };
        unsafe { f() as usize }
    };
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
    let pixel_y = PHYS_H.load(Ordering::Relaxed) - y - 1;
    let sample = DepthSample {
        eye,
        ray,
        projection: *projection,
    };
    let lock = DEPTH_READBACK.get_or_init(|| Mutex::new(DepthReadback::new()));
    let Ok(mut readback) = lock.try_lock() else {
        return None;
    };
    if readback.disabled {
        return None;
    }
    if readback.context != context {
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
        if buffers.contains(&0) {
            readback.disabled = true;
            return None;
        }
        for &buffer in &buffers {
            unsafe {
                bind(0x88eb, buffer); // GL_PIXEL_PACK_BUFFER
                buffer_data(0x88eb, 4, core::ptr::null(), 0x88e1); // GL_STREAM_READ
            }
        }
        unsafe { bind(0x88eb, previous as u32) };
        readback.context = context;
        readback.buffers = buffers;
        readback.fences = [0; 2];
        readback.samples = [None; 2];
        readback.next = 0;
        readback.last_hit = None;
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
        let sample = readback.samples[slot].take();
        if status != 0x911a && status != 0x911c {
            continue; // GL_WAIT_FAILED or an unknown status.
        }
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
        if let Some(sample) = sample {
            readback.last_hit = depth.and_then(|depth| depth_point(terrain, sample, depth));
        }
    }

    let slot = readback.next;
    if readback.fences[slot] == 0 {
        let fence_sync: unsafe extern "system" fn(u32, u32) -> *const c_void =
            unsafe { core::mem::transmute(api.fence_sync) };
        let mut previous = 0i32;
        unsafe { get_int(0x88ed, &mut previous) };
        unsafe { bind(0x88eb, readback.buffers[slot]) };
        unsafe {
            read_pixels(
                x,
                pixel_y,
                1,
                1,
                0x1902,                // GL_DEPTH_COMPONENT
                0x1406,                // GL_FLOAT
                core::ptr::null_mut(), // byte offset zero in the bound PBO
            )
        };
        let fence = unsafe { fence_sync(0x9117, 0) }; // GL_SYNC_GPU_COMMANDS_COMPLETE
        unsafe { bind(0x88eb, previous as u32) };
        if fence.is_null() {
            readback.disabled = true;
            return None;
        }
        readback.fences[slot] = fence as usize;
        readback.samples[slot] = Some(sample);
        readback.next = (slot + 1) % readback.buffers.len();
    }
    readback.last_hit
}

unsafe fn load_depth_api() -> Option<DepthApi> {
    let get_proc = REAL_WGL_GET_PROC_ADDRESS.load(Ordering::Relaxed);
    if get_proc == 0 {
        return None;
    }
    let get_proc: unsafe extern "system" fn(*const i8) -> *mut c_void =
        unsafe { core::mem::transmute(get_proc) };
    let resolve = |core: &'static [u8], arb: &'static [u8]| {
        let pointer = unsafe { get_proc(core.as_ptr().cast()) } as usize;
        if pointer > 3 {
            pointer
        } else {
            let pointer = unsafe { get_proc(arb.as_ptr().cast()) } as usize;
            (pointer > 3).then_some(pointer).unwrap_or(0)
        }
    };
    let api = DepthApi {
        gen_buffers: resolve(b"glGenBuffers\0", b"glGenBuffersARB\0"),
        bind_buffer: resolve(b"glBindBuffer\0", b"glBindBufferARB\0"),
        buffer_data: resolve(b"glBufferData\0", b"glBufferDataARB\0"),
        map_buffer: resolve(b"glMapBuffer\0", b"glMapBufferARB\0"),
        unmap_buffer: resolve(b"glUnmapBuffer\0", b"glUnmapBufferARB\0"),
        fence_sync: resolve(b"glFenceSync\0", b"glFenceSync\0"),
        client_wait_sync: resolve(b"glClientWaitSync\0", b"glClientWaitSync\0"),
        delete_sync: resolve(b"glDeleteSync\0", b"glDeleteSync\0"),
    };
    (api.gen_buffers != 0
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
    if !depth.is_finite() || !(0.0..1.0).contains(&depth) {
        return None;
    }
    let ndc_z = depth * 2.0 - 1.0;
    let denominator = ndc_z + sample.projection[10];
    if denominator.abs() < 1e-6 {
        return None;
    }
    // The camera-space ray has z=-1, so this is its distance parameter.
    let distance = sample.projection[14] / denominator;
    if !distance.is_finite() || distance <= 0.0 || distance > 5000.0 {
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
    let Some(world) = (unsafe { world() }) else {
        return;
    };
    let Some(owner) = (unsafe { rd_u32(native.owner_slot) }).filter(|&p| p != 0) else {
        return;
    };
    let Some(terrain) = (unsafe { terrain() }) else {
        return;
    };
    let scene = Scene {
        owner: owner as usize,
        world,
        map: terrain.map,
    };
    let command = unsafe { rd_u32(world + 0x38) };
    if state.scene != Some(scene) || !state.recent(now) || !focused() || command != Some(0) {
        state.cancel();
    }
    state.scene = Some(scene);
    state.seen_at = now;
    state.can_start = command == Some(0);
    state.unit_selected = unsafe { rd_u32(world + 0x8a0) } != Some(0);
    if !state.active() {
        return;
    }
    state.result = None;
    let Some((target, pixel, ground_origin)) =
        (unsafe { cursor(&terrain, &mut state, now) }).filter(|(p, _, _)| in_terrain(*p, &terrain))
    else {
        return;
    };
    if let Some(origin) = state.pending.take() {
        state.origin = match origin {
            Origin::Cursor => ground_origin.or(Some(target)),
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
        unsafe { draw(native, scene.owner, result, now, state.source_height) };
    }
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
    let context = unsafe { rd_u32(owner + 0x1c)? } as usize;
    if !unsafe { committed(context, 0x54) } {
        return None;
    }
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

unsafe fn draw(
    native: Native,
    owner: usize,
    result: CokeRangeResult,
    now: u32,
    source_height: f32,
) {
    if unsafe { rd_u32(owner + 0xc99c) }.is_none_or(|p| p == 0) {
        return;
    }
    let a = CokeRangePoint {
        z: result.origin.z + source_height,
        ..result.origin
    };
    let b = result.target;
    let alpha = |period: u32, base| {
        let t = now % period;
        base + 0.2 * (t.min(period - t) as f32 / (period / 2) as f32)
    };
    unsafe {
        if result.status == RANGE_CLEAR {
            ribbon(native, owner, 5, alpha(700, 0.5), a, b);
        } else {
            let middle = between(a, b, result.visible_fraction);
            if result.visible_fraction > 0.0 {
                ribbon(native, owner, 14, alpha(700, 0.5), a, middle);
            }
            ribbon(native, owner, 4, alpha(300, 0.7), b, middle);
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
            in("xmm2") 0.65f32,
            clobber_abi("C"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
