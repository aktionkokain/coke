use super::{
    CallPatch, Native, Point, STATE, Terrain, battle_cursor_ray, trace, trace_context, trace_ground,
};
use crate::{
    REAL_GL_COLOR_3F, REAL_GL_DISABLE, REAL_GL_POP_ATTRIB, REAL_GL_PUSH_ATTRIB, log, profile,
    rd_bytes, rd_u32,
};
use coke_host::{CokeRangePoint, CokeTerrainTile};
use std::cell::Cell;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const HALF_WIDTH: f32 = 12.0;
const TERRAIN_DRAW: u32 = 0x0032_6ad0;
const TERRAIN_DRAW_SITES: [u32; 2] = [0x0039_a84f, 0x0039_a961];
const COLOR_BUFFER_OFFSET: usize = 0x0011_6f44;
const DISTANT_LIST_CALL: u32 = 0x0032_783c;
const GL_CALL_LIST_SLOT: u32 = 0x005d_53bc;
const DISTANT_CHUNKS_END: usize = 0x0011_6fc0;
const COLOR_WIDTH_OFFSET: usize = 0x0c;
const COLOR_HEIGHT_OFFSET: usize = 0x10;
const CHUNK_COUNT_OFFSET: usize = 0x24;
const CHUNKS_OFFSET: usize = 0x94;
const CHUNK_COLOR_CACHE_OFFSET: usize = 0x6c;
const MAX_SCAN_CELLS: usize = 65_536;
const CHUNK_METRES: f32 = 16.0;
const DETAILED_RADIUS: f32 = 192.0;
const SCAN_FRAME_BUDGET: Duration = Duration::from_millis(2);

const BLUE: [f32; 3] = [0.40, 0.45, 1.0];
const RED: [f32; 3] = [1.0, 0.27, 0.22];
const GREEN: [f32; 3] = [0.28, 1.0, 0.28];
static INSTALLED: AtomicBool = AtomicBool::new(false);

struct DistantGl {
    call_list: unsafe extern "system" fn(u32),
    color: unsafe extern "system" fn(f32, f32, f32),
    push: unsafe extern "system" fn(u32),
    pop: unsafe extern "system" fn(),
    disable: unsafe extern "system" fn(u32),
}
static DISTANT_GL: OnceLock<DistantGl> = OnceLock::new();

thread_local! {
    static DRAW_PAINT: Cell<*const Paint> = const { Cell::new(core::ptr::null()) };
    static DISTANT_END: Cell<usize> = const { Cell::new(0) };
}

pub(super) struct Overlay {
    requested: Option<Point>,
    patch: Option<Patch>,
    map: usize,
    world_w: f32,
    world_h: f32,
    tiles: Vec<CokeTerrainTile>,
    revision: u32,
    painted: Option<Arc<Paint>>,
    prepared: Option<(usize, u32)>,
    scan_pick: Option<(Point, u32)>,
    scan: Option<Scan>,
    scan_active: bool,
    summary: Option<ScanSummary>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HudStatus {
    Picking,
    Scanning(usize),
    Ready(ScanSummary),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ScanSummary {
    pub(super) visible: usize,
    pub(super) sampled: usize,
    pub(super) tiles: usize,
}

struct Scan {
    origin: CokeRangePoint,
    columns: usize,
    rows: usize,
    step: f32,
    next: usize,
    started: Instant,
    tiles: Vec<CokeTerrainTile>,
    summary: ScanSummary,
}

struct Patch {
    blue: bool,
    map: usize,
    x: f32,
    y: f32,
    world_w: f32,
    world_h: f32,
}

impl Overlay {
    pub(super) const fn new() -> Self {
        Self {
            requested: None,
            patch: None,
            map: 0,
            world_w: 0.0,
            world_h: 0.0,
            tiles: Vec::new(),
            revision: 0,
            painted: None,
            prepared: None,
            scan_pick: None,
            scan: None,
            scan_active: false,
            summary: None,
        }
    }

    pub(super) fn request(&mut self, cursor: Point) {
        self.requested = Some(cursor);
    }

    pub(super) fn clear(&mut self) {
        self.requested = None;
        self.patch = None;
        self.tiles.clear();
        self.scan_pick = None;
        self.scan = None;
        self.scan_active = false;
        self.summary = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn start_scan(&mut self, pixel: Point, now: u32) {
        self.scan_active = true;
        self.scan_pick = Some((pixel, now));
    }

    pub(super) fn stop_scan(&mut self) -> bool {
        let active = self.scan_active;
        if active {
            self.clear();
        }
        active
    }

    pub(super) fn scan_pick(&self) -> Option<(Point, u32)> {
        self.scan_pick
    }

    pub(super) fn hud_status(&self) -> Option<HudStatus> {
        if self.scan_pick.is_some() {
            Some(HudStatus::Picking)
        } else if let Some(scan) = self.scan.as_ref() {
            Some(HudStatus::Scanning(
                scan.next * 100 / (scan.columns * scan.rows),
            ))
        } else {
            self.summary.map(HudStatus::Ready)
        }
    }

    pub(super) fn begin_scan(&mut self, terrain: &Terrain, origin: CokeRangePoint) {
        self.scan_pick = None;
        let mut step = CHUNK_METRES;
        loop {
            let columns = (terrain.world_w / step).ceil() as usize;
            let rows = (terrain.world_h / step).ceil() as usize;
            if columns.saturating_mul(rows) <= MAX_SCAN_CELLS {
                self.scan = Some(Scan {
                    origin,
                    columns,
                    rows,
                    step,
                    next: 0,
                    started: Instant::now(),
                    tiles: Vec::new(),
                    summary: ScanSummary::default(),
                });
                log(&format!(
                    "cokerange: scanning native LOS at {:.2}, {:.2}, {:.2} ({columns}x{rows})",
                    origin.x, origin.y, origin.z
                ));
                return;
            }
            step *= 2.0;
        }
    }

    pub(super) unsafe fn scan_frame(&mut self, terrain: &Terrain, native: Native, owner: usize) {
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        let Some(context) = (unsafe { trace_context(owner) }) else {
            return;
        };
        let start = Instant::now();
        let height_ptr = terrain.height_ptr as *const f32;
        let height_count = terrain.grid_w as usize * terrain.grid_h as usize;
        let direct_heights =
            unsafe { crate::coke::mod_span(height_ptr, height_count, false) }.is_some();
        let cells = scan.columns * scan.rows;
        // Use the live engine context on the battle render thread. Yield
        // between batches so a large map does not stall a single frame.
        while scan.next < cells {
            let row = scan.next / scan.columns;
            let column = scan.next % scan.columns;
            let x0 = column as f32 * scan.step;
            let y0 = row as f32 * scan.step;
            let x1 = (x0 + scan.step).min(terrain.world_w);
            let y1 = (y0 + scan.step).min(terrain.world_h);
            let xm = (x0 + x1) * 0.5;
            let ym = (y0 + y1) * 0.5;
            let detailed = scan.step == CHUNK_METRES
                && (xm - scan.origin.x).powi(2) + (ym - scan.origin.y).powi(2)
                    <= DETAILED_RADIUS * DETAILED_RADIUS;
            let children = [
                (x0, y0, xm, ym),
                (xm, y0, x1, ym),
                (x0, ym, xm, y1),
                (xm, ym, x1, y1),
            ];
            let area = [(x0, y0, x1, y1)];
            for &(x0, y0, x1, y1) in if detailed { &children[..] } else { &area[..] } {
                let x = (x0 + x1) * 0.5;
                let y = (y0 + y1) * 0.5;
                let target = CokeRangePoint {
                    x,
                    y,
                    z: if direct_heights {
                        crate::los::interpolate_terrain_height(terrain, x, y, |index| unsafe {
                            height_ptr.add(index).read()
                        })
                    } else {
                        unsafe { super::terrain_height(terrain, x, y) }
                    },
                };
                let visible = u32::from(unsafe { trace(native, context, scan.origin, target) });
                scan.summary.sampled += 1;
                scan.summary.visible += visible as usize;
                if let Some(last) = scan.tiles.last_mut()
                    && last.visible == visible
                    && last.y0 == y0
                    && last.y1 == y1
                    && last.x1 == x0
                {
                    last.x1 = x1;
                } else {
                    scan.tiles.push(CokeTerrainTile {
                        x0,
                        y0,
                        x1,
                        y1,
                        visible,
                    });
                }
            }
            scan.next += 1;
            if scan.next.is_multiple_of(4) && start.elapsed() >= SCAN_FRAME_BUDGET {
                break;
            }
        }
        if scan.next == cells {
            let tiles = core::mem::take(&mut scan.tiles);
            let summary = ScanSummary {
                tiles: tiles.len(),
                ..scan.summary
            };
            log(&format!(
                "cokerange: LOS scan finished in {} ms",
                scan.started.elapsed().as_millis()
            ));
            self.scan = None;
            self.set_tiles(terrain.map, terrain.world_w, terrain.world_h, tiles);
            self.summary = Some(summary);
        }
    }

    pub(super) fn set_tiles(
        &mut self,
        map: usize,
        world_w: f32,
        world_h: f32,
        tiles: Vec<CokeTerrainTile>,
    ) {
        self.patch = None;
        self.map = map;
        self.world_w = world_w;
        self.world_h = world_h;
        self.tiles = tiles;
        self.summary = None;
        self.revision = self.revision.wrapping_add(1);
        log(&format!(
            "cokerange: LOS terrain paint tiles={}",
            self.tiles.len()
        ));
    }

    pub(super) unsafe fn update(&mut self, terrain: &Terrain, scene_changed: bool) {
        if scene_changed {
            if self
                .painted
                .as_ref()
                .is_some_and(|paint| paint.map != terrain.map)
            {
                self.painted = None;
                self.prepared = None;
            }
            self.clear();
            self.map = 0;
            self.world_w = 0.0;
            self.world_h = 0.0;
        }
        let Some(cursor) = self.requested.take() else {
            return;
        };
        if self.patch.as_ref().is_some_and(|p| p.blue) {
            self.patch = None;
            self.revision = self.revision.wrapping_add(1);
            log("cokerange: terrain overlay cleared");
        } else if let Some((eye, ray, _, _)) = unsafe { battle_cursor_ray(cursor.x, cursor.y) }
            && let Some(hit) = unsafe { trace_ground(terrain, eye, ray) }
        {
            let blue = self.patch.is_some();
            self.patch = Some(Patch {
                blue,
                map: terrain.map,
                x: hit.x,
                y: hit.y,
                world_w: terrain.world_w,
                world_h: terrain.world_h,
            });
            self.revision = self.revision.wrapping_add(1);
            log(&format!(
                "cokerange: terrain overlay {} at {:.2}, {:.2}",
                if blue { "blue" } else { "red" },
                hit.x,
                hit.y
            ));
        }
    }

    unsafe fn prepare_paint(&mut self, map: usize) -> Option<Arc<Paint>> {
        let patch = self.patch.as_ref().filter(|patch| patch.map == map);
        let tiles = self.map == map && !self.tiles.is_empty();
        if !tiles && patch.is_none() && self.painted.as_ref().is_none_or(|paint| paint.map != map) {
            return None;
        }
        if self.prepared == Some((map, self.revision)) {
            return self.painted.clone();
        }

        let old_map = self.painted.take().map(|paint| paint.map);
        self.prepared = Some((map, self.revision));
        self.painted = if tiles {
            unsafe { Paint::new(map, self.world_w, self.world_h, &self.tiles) }
        } else if let Some(patch) = patch {
            let tile = CokeTerrainTile {
                x0: (patch.x - HALF_WIDTH).max(0.0),
                y0: (patch.y - HALF_WIDTH).max(0.0),
                x1: (patch.x + HALF_WIDTH).min(patch.world_w),
                y1: (patch.y + HALF_WIDTH).min(patch.world_h),
                visible: if patch.blue { 2 } else { 0 },
            };
            unsafe { Paint::new(map, patch.world_w, patch.world_h, &[tile]) }
        } else {
            None
        }
        .map(Arc::new);
        if old_map == Some(map) || self.painted.is_some() {
            unsafe { invalidate_chunk_colors(map) };
        }
        self.painted.clone()
    }
}

pub(super) fn hooks() -> Option<Vec<CallPatch>> {
    let p = profile::profile()?;
    if !p.exe.eq_ignore_ascii_case("CM Black Sea.exe")
        || p.image_size != 0x00a5_f000
        || p.pe_timestamp != 0x63d8_2c90
        || p.text_fnv1a != Some(0xf7b8_0d9a)
    {
        return None;
    }
    let draw = profile::profile_addr(TERRAIN_DRAW)?;
    let list_slot = profile::profile_addr(GL_CALL_LIST_SLOT)?;
    let call_list = unsafe { rd_u32(list_slot) }? as usize;
    let color = REAL_GL_COLOR_3F.load(Ordering::Relaxed);
    let push = REAL_GL_PUSH_ATTRIB.load(Ordering::Relaxed);
    let pop = REAL_GL_POP_ATTRIB.load(Ordering::Relaxed);
    let disable = REAL_GL_DISABLE.load(Ordering::Relaxed);
    if [call_list, color, push, pop, disable].contains(&0) {
        return None;
    }
    DISTANT_GL
        .set(unsafe {
            DistantGl {
                call_list: core::mem::transmute::<usize, unsafe extern "system" fn(u32)>(call_list),
                color: core::mem::transmute::<usize, unsafe extern "system" fn(f32, f32, f32)>(
                    color,
                ),
                push: core::mem::transmute::<usize, unsafe extern "system" fn(u32)>(push),
                pop: core::mem::transmute::<usize, unsafe extern "system" fn()>(pop),
                disable: core::mem::transmute::<usize, unsafe extern "system" fn(u32)>(disable),
            }
        })
        .ok()?;
    let mut hooks = Vec::new();
    for site in TERRAIN_DRAW_SITES {
        hooks.push(CallPatch::relative(
            profile::profile_addr(site)?,
            draw,
            draw_terrain as *const () as usize,
        ));
    }
    hooks.push(CallPatch::imported(
        profile::profile_addr(DISTANT_LIST_CALL)?,
        list_slot,
        distant_list as *const () as usize,
    ));
    Some(hooks)
}

pub(super) fn installed() {
    INSTALLED.store(true, Ordering::Release);
    log("cokerange: native terrain overlay hook installed");
}

pub(super) fn available() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

unsafe extern "thiscall" fn draw_terrain(map: usize) {
    let Some(address) = profile::profile_addr(TERRAIN_DRAW) else {
        return;
    };
    let original: unsafe extern "thiscall" fn(usize) = unsafe { core::mem::transmute(address) };
    let paint = STATE
        .lock()
        .ok()
        .and_then(|mut state| unsafe { state.terrain_overlay.prepare_paint(map) });
    let Some(paint) = paint else {
        unsafe { original(map) };
        return;
    };

    // The terrain draw consumes the color grid. Leave the game-owned grid
    // in place for the deployment writer and all other engine code.
    let slot = (map + COLOR_BUFFER_OFFSET) as *mut usize;
    let previous_grid = unsafe { slot.replace(paint.colors.as_ptr() as usize) };
    let previous_paint = DRAW_PAINT.with(|state| state.replace(Arc::as_ptr(&paint)));
    unsafe { original(map) };
    if DISTANT_END.with(|state| state.replace(0)) != 0
        && let Some(gl) = DISTANT_GL.get()
    {
        unsafe { (gl.pop)() };
    }
    DRAW_PAINT.with(|state| state.set(previous_paint));
    unsafe { slot.write(previous_grid) };
}

// At the verified glCallList site, EAX holds the current terrain chunk and
// ESI points to its entry in the distant chunk vector, in both cache branches.
#[unsafe(naked)]
unsafe extern "system" fn distant_list() {
    core::arch::naked_asm!(
        "push esi",
        "push eax",
        "push dword ptr [esp + 12]",
        "call {draw}",
        "ret 4",
        draw = sym draw_distant_chunk,
    );
}

unsafe extern "system" fn draw_distant_chunk(list: u32, chunk: *const u32, entry: usize) {
    let Some(gl) = DISTANT_GL.get() else {
        return;
    };
    let Some(paint) = (unsafe { DRAW_PAINT.with(|state| state.get()).as_ref() }) else {
        unsafe { (gl.call_list)(list) };
        return;
    };
    let end = DISTANT_END.with(|state| {
        let end = state.get();
        if end != 0 {
            return end;
        }
        let end = unsafe { rd_u32(paint.map + DISTANT_CHUNKS_END) }.unwrap_or(0) as usize;
        if end > entry {
            // Keep the terrain texture, fog and depth test. The paint itself
            // supplies the vertex color in place of this pass's lighting.
            unsafe {
                (gl.push)(0x41); // GL_CURRENT_BIT | GL_LIGHTING_BIT
                (gl.disable)(0x0b50); // GL_LIGHTING
            }
            state.set(end);
        }
        end
    });
    if end > entry {
        let x = unsafe { chunk.add(0x44 / 4).read() } as usize;
        let y = unsafe { chunk.add(0x48 / 4).read() } as usize;
        let [r, g, b] = paint
            .color_at(x.saturating_add(8), y.saturating_add(8))
            .unwrap_or([1.0; 3]);
        unsafe { (gl.color)(r, g, b) };
    }
    unsafe { (gl.call_list)(list) };
    if entry + 4 == end {
        unsafe { (gl.pop)() };
        DISTANT_END.with(|state| state.set(0));
    }
}

struct Paint {
    map: usize,
    width: usize,
    height: usize,
    colors: Vec<[f32; 3]>,
}

impl Paint {
    fn color_at(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        (x < self.width && y < self.height).then(|| self.colors[y * self.width + x])
    }

    unsafe fn new(
        map: usize,
        world_w: f32,
        world_h: f32,
        tiles: &[CokeTerrainTile],
    ) -> Option<Self> {
        let width = unsafe { rd_u32(map + COLOR_WIDTH_OFFSET) }? as usize;
        let height = unsafe { rd_u32(map + COLOR_HEIGHT_OFFSET) }? as usize;
        let near = unsafe { rd_u32(map + COLOR_BUFFER_OFFSET) }? as usize;
        let count = width.checked_mul(height)?;
        if width < 2 || height < 2 || count > 8_388_608 || world_w <= 0.0 || world_h <= 0.0 {
            return None;
        }

        let mut colors = Vec::new();
        colors.try_reserve_exact(count).ok()?;
        colors.resize(count, [1.0; 3]);
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(colors.as_mut_ptr().cast::<u8>(), count * 12)
        };
        if !unsafe { rd_bytes(near, bytes) } {
            return None;
        }
        for tile in tiles {
            let columns = grid_span(tile.x0, tile.x1, world_w, width);
            let rows = grid_span(tile.y0, tile.y1, world_h, height);
            let color = match tile.visible {
                1 => GREEN,
                2 => BLUE,
                _ => RED,
            };
            for y in rows {
                colors[y * width + columns.start..y * width + columns.end].fill(color);
            }
        }
        Some(Self {
            map,
            width,
            height,
            colors,
        })
    }
}

fn grid_span(low: f32, high: f32, world: f32, samples: usize) -> core::ops::Range<usize> {
    let last = (samples - 1) as f32;
    let scale = last / world;
    let from = (low * scale).ceil().clamp(0.0, last) as usize;
    let end = if high >= world {
        samples
    } else {
        (high * scale).ceil().clamp(0.0, last) as usize
    };
    from..end.max(from)
}

unsafe fn invalidate_chunk_colors(map: usize) {
    let Some(count) = (unsafe { rd_u32(map + CHUNK_COUNT_OFFSET) }).filter(|&n| n <= 65_536) else {
        return;
    };
    let Some(chunks) = (unsafe { rd_u32(map + CHUNKS_OFFSET) }).filter(|&p| p >= 0x10000) else {
        return;
    };
    let chunks = chunks as *const u32;
    let count = count as usize;
    if unsafe { crate::coke::mod_span(chunks, count, false) }.is_none() {
        return;
    }
    for &chunk in unsafe { core::slice::from_raw_parts(chunks, count) } {
        let Some(addr) = (chunk as usize).checked_add(CHUNK_COLOR_CACHE_OFFSET) else {
            continue;
        };
        if unsafe { rd_u32(addr) }.is_some_and(|cache| cache != 0) {
            unsafe { (addr as *mut u32).write(0) };
        }
    }
}
