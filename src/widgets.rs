//! Engine-layout repair, executed from the watcher thread.
//!
//! Two repairs share one heap walk, both discovered at runtime (no absolute
//! addresses):
//!
//! **Widget re-center** (`menu_recenter`): the engine's widget constructor
//! `FUN_007990a0` stores a vtable at `+0x00`, the parent at `+0x04`, an
//! *absolute* rect as four i16s at `+0x10..+0x17` (children are centered or
//! clamped into the parent at construction), and a children vector
//! `{begin, end, capacity}` at `+0x3c/+0x40/+0x44`; the constructor also
//! registers the new widget in the parent's vector. Screens built before a canvas change keep geometry
//! centered for that canvas (`2*x + w == canvas_w`, odd halves truncated).
//! Candidates are matched against every canvas the engine has used, validated
//! (vtable inside the main module image, parent-backlink membership, sane
//! children span) and re-centered for the current canvas, shifting the whole
//! subtree. This fixes drawing AND mouse hit testing.
//!
//! **Scene-rect repair** (`scene_rect_fix`): the engine draws the battle scene
//! through a render-target object whose rect (u32 x/y/w/h at `+0x28..+0x37`)
//! is copied *once* from the scene widget (i16 rect at `+0x10..+0x17`, linked
//! at `+0x54`) when the object is created. After a canvas change the engine
//! has been observed asking for the scene at `(0,0,w,h-inset)` (F11 grow) and
//! at `(0,98,cw,ch-inset)` (mid-battle resize, y from an un-RE'd convention),
//! and BOTH the draw viewport and the click-to-unit picking derive from that
//! rect - so the fix must repair the engine's rects, not the drawn viewport.
//! The y convention of the rect that feeds the viewport is not known a
//! priori, so the repair works by *feedback*: write a candidate y, observe
//! the next scene viewport the engine asks for, derive the slope (+1
//! identity or -1 bottom-up flip), and apply the exact correction. Two
//! observations determine the mapping for any linear convention.
//!
//! Safety: the walk never dereferences engine memory directly. Regions are
//! enumerated with VirtualQuery and read through `ReadProcessMemory` in
//! 64 KB chunks, which fails cleanly instead of faulting when the engine
//! frees or decommits memory mid-walk. Validation reads on candidates use the
//! same fault-safe read. Writes go only to addresses re-validated as writable
//! immediately before the write.

use crate::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

// ------------------------------------------------------------- scheduling

/// Set by `patch_engine_canvas` when the engine's canvas actually changed.
static RECENTER_PENDING: AtomicU32 = AtomicU32::new(0);
/// Set by the `glViewport` hook when the engine asks for a misplaced scene.
static SCENE_FIX_PENDING: AtomicU32 = AtomicU32::new(0);
/// Earliest tick the next scan may start (lets the engine settle).
static NOT_BEFORE: AtomicU32 = AtomicU32::new(0);
static LAST_SCAN_END: AtomicU32 = AtomicU32::new(0);
/// 0 = idle; otherwise the next address to scan.
static SCAN_CURSOR: AtomicU64 = AtomicU64::new(0);
static SCAN_ACTIVE: AtomicU32 = AtomicU32::new(0);
/// Tick the current walk started (for total-duration reporting).
static SCAN_T0: AtomicU32 = AtomicU32::new(0);
static SCAN_LOG: AtomicU32 = AtomicU32::new(0);
/// Why the current walk was started (1 = scene fix, 2 = recenter/other).
static SCAN_FOR: AtomicU32 = AtomicU32::new(0);
/// Census accumulated over the whole walk. Per-tick counters would only cover
/// the final slice and misreport the walk as starved.
static CENSUS_PRIV: AtomicU64 = AtomicU64::new(0);
static CENSUS_IMG: AtomicU64 = AtomicU64::new(0);
static CENSUS_MAP: AtomicU64 = AtomicU64::new(0);
static CENSUS_PRIV_N: AtomicU64 = AtomicU64::new(0);
static CENSUS_IMG_N: AtomicU64 = AtomicU64::new(0);
static CENSUS_MAP_N: AtomicU64 = AtomicU64::new(0);
static CENSUS_WALKED: AtomicU64 = AtomicU64::new(0);
static CENSUS_READ: AtomicU64 = AtomicU64::new(0);
static CENSUS_VT: AtomicU64 = AtomicU64::new(0);
static CENSUS_RECT: AtomicU64 = AtomicU64::new(0);
static CENSUS_CENTERED: AtomicU64 = AtomicU64::new(0);
static CENSUS_PARENT_REJ: AtomicU64 = AtomicU64::new(0);
static CENSUS_CHILD_REJ: AtomicU64 = AtomicU64::new(0);
static CENSUS_SCENE: AtomicU64 = AtomicU64::new(0);
static CENSUS_FULLSPAN: AtomicU64 = AtomicU64::new(0);
static CENSUS_DELTA0: AtomicU64 = AtomicU64::new(0);

/// Called from `patch_engine_canvas` after the engine's canvas changed.
pub fn schedule_widget_recenter() {
    if RECENTER_PENDING.swap(1, Ordering::Relaxed) == 0 {
        log("widget recenter: scheduled");
    }
    // A new canvas is a new layout key: forget scene-fix give-ups and any
    // probe armed for the old canvas' rects.
    SC_EXHAUSTED_WH.store(0, Ordering::Relaxed);
    SC_EMPTY_SCANS.store(0, Ordering::Relaxed);
    PROBE_WH.store(0, Ordering::Relaxed);
    NOT_BEFORE.store(unsafe { GetTickCount() } + 600, Ordering::Relaxed);
}

/// Called from the `glViewport` hook (via `scene_vp_observed`) when the engine
/// asks for a misplaced scene. Runs every frame while the scene is misplaced,
/// so the settle delay is only armed on the *first* request - re-arming it
/// per frame would postpone the scan forever.
fn schedule_scene_fix() {
    if SCENE_FIX_PENDING.swap(1, Ordering::Relaxed) == 0 {
        log("scene rect fix: scheduled");
        NOT_BEFORE.store(unsafe { GetTickCount() } + 600, Ordering::Relaxed);
    }
}

/// True while a heap walk is in progress (the watcher polls fast then, since
/// F11 and the window geometry are handled on that same thread).
pub fn scan_in_progress() -> bool {
    SCAN_ACTIVE.load(Ordering::Relaxed) != 0
}

// ---------------------------------------------------------------- scanning

const SLICE_MS: u32 = 8;
const CHUNK: usize = 65536;
const MAX_TREE: usize = 512;
const MAX_TREE_DEPTH: usize = 10;
const MAX_CHILDREN: usize = 4096;
const MAX_FIXES: usize = 8;

#[inline]
fn in_image(v: u32, base: usize, end: usize) -> bool {
    (v as usize) >= base && (v as usize) < end
}

/// Fault-safe read through ReadProcessMemory: returns None instead of
/// raising an exception when the address is unmapped or unreadable.
#[inline]
unsafe fn rpm_u32(addr: usize) -> Option<u32> {
    unsafe {
        let mut out = 0u32;
        let mut got = 0usize;
        if ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            (&mut out as *mut u32) as *mut c_void,
            4,
            &mut got,
        ) != 0
            && got == 4
        {
            Some(out)
        } else {
            None
        }
    }
}

/// Cheap "is this pointer a readable heap address" check via VirtualQuery,
/// with a small cache of recently seen regions (heap pointers cluster).
struct RegionCache {
    entries: [(usize, usize); 16],
    n: usize,
}

impl RegionCache {
    fn new() -> Self {
        RegionCache {
            entries: [(0, 0); 16],
            n: 0,
        }
    }
    fn hit(&self, addr: usize) -> bool {
        self.entries[..self.n]
            .iter()
            .any(|&(s, e)| addr >= s && addr < e)
    }
    fn remember(&mut self, start: usize, size: usize) {
        if self.n < self.entries.len() {
            self.entries[self.n] = (start, start + size);
            self.n += 1;
        }
    }
}

unsafe fn region_of(addr: usize) -> Option<(usize, usize)> {
    unsafe {
        if addr < 0x10000 {
            return None;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(
            addr as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) != core::mem::size_of::<MemBasicInfo>()
        {
            return None;
        }
        if m.state != MEM_COMMIT
            || m.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            || addr.saturating_sub(m.base_address as usize) >= m.region_size
        {
            return None;
        }
        Some((m.base_address as usize, m.region_size))
    }
}

/// Is the region containing `addr` writable (for the apply-time writes)?
unsafe fn writable(addr: usize) -> bool {
    unsafe {
        if addr < 0x10000 {
            return false;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(
            addr as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) != core::mem::size_of::<MemBasicInfo>()
            || m.state != MEM_COMMIT
            || addr.saturating_sub(m.base_address as usize) >= m.region_size
        {
            return false;
        }
        matches!(
            m.protect & 0xff,
            PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        ) && m.protect & PAGE_GUARD == 0
    }
}

/// Validate `addr` as a widget: vtable inside the main module image and a
/// children vector that is either empty or a sane, readable pointer pair.
/// All reads are fault-safe (RPM).
unsafe fn widget_children(
    addr: usize,
    base: usize,
    end: usize,
    cache: &mut RegionCache,
) -> Option<(usize, usize)> {
    unsafe {
        if !in_image(rpm_u32(addr)?, base, end) {
            return None;
        }
        let begin = rpm_u32(addr + 0x3c)? as usize;
        let endp = rpm_u32(addr + 0x40)? as usize;
        if begin > endp || endp - begin > MAX_CHILDREN * 4 || begin & 3 != 0 {
            return None;
        }
        if begin != endp {
            if cache.hit(begin) && cache.hit(endp - 4) {
                return Some((begin, endp));
            }
            match (region_of(begin), region_of(endp - 4)) {
                (Some((s1, l1)), Some((s2, l2))) => {
                    cache.remember(s1, l1);
                    cache.remember(s2, l2);
                }
                _ => return None,
            }
        }
        Some((begin, endp))
    }
}

/// Is `addr` a member of its parent's child vector (the constructor
/// registers every child)? Fault-safe reads.
unsafe fn member_of_parent(parent: usize, addr: usize, cache: &mut RegionCache) -> bool {
    unsafe {
        if !(cache.hit(parent) || region_of(parent).is_some()) {
            return false;
        }
        let begin = match rpm_u32(parent + 0x3c) {
            Some(v) => v as usize,
            None => return false,
        };
        let endp = match rpm_u32(parent + 0x40) {
            Some(v) => v as usize,
            None => return false,
        };
        if begin > endp || endp - begin > MAX_CHILDREN * 4 || begin & 3 != 0 {
            return false;
        }
        let mut q = begin;
        while q + 4 <= endp {
            match rpm_u32(q) {
                Some(v) if v as usize == addr => return true,
                Some(_) => q += 4,
                None => return false,
            }
        }
        false
    }
}

/// Shift a widget subtree by `(dx, dy)` (rects are absolute, so children move
/// with their parent). Reads are fault-safe; writes are gated on a fresh
/// writable check per node.
unsafe fn shift_tree(
    root: usize,
    dx: i32,
    dy: i32,
    base: usize,
    end: usize,
    cache: &mut RegionCache,
) -> usize {
    unsafe {
        let mut moved = 0usize;
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((addr, depth)) = stack.pop() {
            if moved >= MAX_TREE || depth > MAX_TREE_DEPTH {
                break;
            }
            let vt = match rpm_u32(addr) {
                Some(v) => v,
                None => continue,
            };
            if !in_image(vt, base, end) {
                continue;
            }
            let packed = match rpm_u32(addr + 0x10) {
                Some(v) => v,
                None => continue,
            };
            let x = (packed & 0xffff) as i32;
            let y = (packed >> 16) as i32;
            let nx = (x + dx).max(0) as u32;
            let ny = (y + dy).max(0) as u32;
            if ((nx != packed & 0xffff) || (ny != packed >> 16)) && writable(addr + 0x10) {
                core::ptr::write_unaligned((addr + 0x10) as *mut u32, (nx & 0xffff) | (ny << 16));
                moved += 1;
            }
            if let Some((begin, endp)) = widget_children(addr, base, end, cache) {
                let mut p = begin;
                while p + 4 <= endp {
                    if let Some(child) = rpm_u32(p) {
                        let child = child as usize;
                        if child > 0x10000 && (cache.hit(child) || region_of(child).is_some()) {
                            stack.push((child, depth + 1));
                        }
                    }
                    p += 4;
                }
            }
        }
        moved
    }
}

// ---------------------------------------------------------- scene probing

/// Candidate rect source for the scene fix, packed:
/// `addr << 32 | kind << 16 | y_at_scan`. kind 1 = render-target object
/// (u32 rect at +0x28..+0x37, widget backlink at +0x54), kind 2 = widget
/// (i16 rect at +0x10..+0x17).
static SC_CAND: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static SC_CAND_N: AtomicU32 = AtomicU32::new(0);
/// The widget backlink of a kind-1 candidate (0 if none / kind 2), so a
/// successful fix can be propagated to the linked object.
static SC_CAND_PAIR: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Scene-shaped viewport the engine asked for last, packed like
/// `pack_rect` (x, y, w, h as u16).
static LAST_SCENE_VP: AtomicU64 = AtomicU64::new(u64::MAX);
/// The (w, h) pair the probe is working on, packed `(w << 32) | h`. 0 = idle.
static PROBE_WH: AtomicU64 = AtomicU64::new(0);
static PROBE_IDX: AtomicU32 = AtomicU32::new(0);
static PROBE_TRIES: AtomicU32 = AtomicU32::new(0);
/// Observation points: what we wrote (candidate y) vs what the engine asked.
static PROBE_W1: AtomicI32 = AtomicI32::new(0);
static PROBE_V1: AtomicI32 = AtomicI32::new(0);
static PROBE_W2: AtomicI32 = AtomicI32::new(0);
static PROBE_V2: AtomicI32 = AtomicI32::new(0);
/// Debounce: the engine asks for the scene viewport several times per frame
/// (blur pass + main pass, plus repeats), and a write from within one of
/// those calls only affects the frame rendered after it. Processing at most
/// one observation per 20 ms keeps the state machine in step with the render
/// loop and prevents same-frame duplicates from advancing it wrongly.
static PROBE_LAST_OBS_TICK: AtomicU32 = AtomicU32::new(0);
/// 0 = awaiting the observation of the initial write, 1 = awaiting the
/// observation of the slope-corrected write.
static PROBE_STAGE: AtomicU32 = AtomicU32::new(0);
/// (w,h) pairs we gave up on (per canvas epoch; cleared on canvas change).
static SC_EXHAUSTED_WH: AtomicU64 = AtomicU64::new(0);
static SC_EMPTY_SCANS: AtomicU32 = AtomicU32::new(0);

fn scene_target_inset() -> i32 {
    let cfg = config().scene_inset;
    if cfg > 0 { cfg } else { scene_inset() }
}

#[inline]
fn pack_vp(x: i32, y: i32, w: i32, h: i32) -> u64 {
    ((x as u16 as u64) << 48)
        | ((y as u16 as u64) << 32)
        | ((w as u16 as u64) << 16)
        | (h as u16 as u64)
}

/// Called from the `glViewport` hook for every battle-scene-shaped viewport
/// `(0, y, w, h)` with `(w, h) == (canvas_w, canvas_h - inset)` for a known
/// canvas. Drives the feedback controller that straightens the scene's y.
pub fn scene_vp_observed(x: i32, y: i32, w: i32, h: i32) {
    if x != 0 || w <= 0 || h <= 0 {
        return;
    }
    record_scene_vp(x, y, w, h);
    LAST_SCENE_VP.store(pack_vp(x, y, w, h), Ordering::Relaxed);

    let inset = scene_target_inset();
    let wh = ((w as u64) << 32) | (h as u32 as u64);
    if y == inset {
        // Healthy scene: if the probe is armed for this shape, it succeeded.
        if PROBE_WH.load(Ordering::Relaxed) == wh {
            let i = PROBE_IDX.load(Ordering::Relaxed) as usize;
            let cand = SC_CAND[i.min(7)].load(Ordering::Relaxed);
            let addr = (cand >> 32) as u32 as usize;
            log(&format!(
                "scene rect fix: verified y={y} for ({w},{h}) via 0x{addr:x}"
            ));
            disarm_probe(wh);
        }
        return;
    }

    // Misplaced scene. If the probe is working on this shape, feed it.
    if PROBE_WH.load(Ordering::Relaxed) == wh {
        probe_feedback(y, w, h);
        return;
    }
    // A probe armed for a different shape is stale (the engine moved on to a
    // new canvas); disarm it so this shape can be scanned and probed.
    let armed = PROBE_WH.load(Ordering::Relaxed);
    if armed != 0 && armed != wh {
        log("scene rect fix: dropping stale probe for an old canvas shape");
        disarm_probe(armed);
    }
    if SC_EXHAUSTED_WH.load(Ordering::Relaxed) == wh {
        return;
    }
    // No probe yet: ask the watcher for a scan that collects candidates.
    if config().scene_rect_fix {
        schedule_scene_fix();
    }
}

fn disarm_probe(wh: u64) {
    PROBE_WH.store(0, Ordering::Relaxed);
    PROBE_STAGE.store(0, Ordering::Relaxed);
    PROBE_TRIES.store(0, Ordering::Relaxed);
    let _ = wh;
}

/// Read a candidate's current y (kind-aware), fault-safe.
unsafe fn scene_read_y(addr: usize, kind: u32) -> Option<i32> {
    unsafe {
        if kind == 1 {
            rpm_u32(addr + 0x2c).map(|v| v as i32)
        } else {
            rpm_u32(addr + 0x10).map(|v| (v >> 16) as u16 as i32)
        }
    }
}

/// Write a candidate's y (kind-aware). The write target is re-validated as
/// writable immediately before writing. Returns false when the target is not
/// writable, in which case the probe moves to the next candidate.
unsafe fn scene_write(addr: usize, kind: u32, y: i32) -> bool {
    unsafe {
        if !writable(addr) {
            return false;
        }
        if kind == 1 {
            core::ptr::write_unaligned((addr + 0x2c) as *mut u32, y as u32);
        } else {
            let cur = match rpm_u32(addr + 0x10) {
                Some(v) => v,
                None => return false,
            };
            core::ptr::write_unaligned(
                (addr + 0x10) as *mut u32,
                (cur & 0xffff) | ((y as u32) << 16),
            );
        }
        true
    }
}

/// Arm the probe on candidate `idx`: record the candidate's current y as the
/// first observation point and write the target inset as the first probe
/// value. Runs on the watcher thread (scan completion) or the engine thread
/// (candidate advance); both only touch engine-owned rect fields.
unsafe fn arm_candidate(idx: usize, obs_y: i32, w: i32, h: i32, target: i32) -> bool {
    unsafe {
        let cand = SC_CAND[idx].load(Ordering::Relaxed);
        if cand == 0 {
            return false;
        }
        let addr = (cand >> 32) as u32 as usize;
        let kind = (cand >> 16) as u16 as u32;
        let y0 = match scene_read_y(addr, kind) {
            Some(v) => v,
            None => return false,
        };
        if !scene_write(addr, kind, target) {
            return false;
        }
        PROBE_W1.store(y0, Ordering::Relaxed);
        PROBE_V1.store(obs_y, Ordering::Relaxed);
        PROBE_W2.store(target, Ordering::Relaxed);
        PROBE_V2.store(i32::MIN, Ordering::Relaxed);
        PROBE_STAGE.store(0, Ordering::Relaxed);
        PROBE_WH.store(((w as u64) << 32) | (h as u32 as u64), Ordering::Relaxed);
        PROBE_IDX.store(idx as u32, Ordering::Relaxed);
        PROBE_TRIES.fetch_add(1, Ordering::Relaxed);
        log(&format!(
            "scene rect fix: probe 0x{addr:x} kind{kind} y {y0}->{target} (expect ({w},{h}) y={target}; was y={obs_y})"
        ));
        true
    }
}

/// Step used to measure the y convention: the second probe writes
/// `target + STEP`, so the sign of `(vp_after - vp_before)` is the sign of
/// the mapping's slope (both plausible conventions have |slope| = 1).
const PROBE_STEP: i32 = 8;

/// Observation of a misplaced scene vp while the probe is armed.
///
/// The mapping from the rect's y to the viewport's y is not known a priori
/// (identity, or a bottom-up flip through some canvas height), and the rect
/// may even change between the scan and the write. So the probe establishes
/// the mapping from *controlled* writes only, each followed by an
/// observation:
///
///   stage 0: wrote `target`                 -> observe v2
///   stage 1: wrote `target + PROBE_STEP`    -> observe v3 (slope sign)
///   stage 2: wrote the exact correction     -> observe v4 (verify)
///
/// Adjacent write/observe pairs are always consistent, so the derived slope
/// cannot be corrupted by the engine changing the rect mid-scan.
fn probe_feedback(obs: i32, w: i32, h: i32) {
    // The engine asks for the scene viewport more than once per frame; only
    // the first observation per ~20 ms is a genuine post-write reading.
    let now = unsafe { GetTickCount() };
    if now.wrapping_sub(PROBE_LAST_OBS_TICK.load(Ordering::Relaxed)) < 20 {
        return;
    }
    PROBE_LAST_OBS_TICK.store(now, Ordering::Relaxed);

    let target = scene_target_inset();
    let stage = PROBE_STAGE.load(Ordering::Relaxed);
    let last_w = PROBE_W2.load(Ordering::Relaxed);
    let prev_v = PROBE_V2.load(Ordering::Relaxed);
    let v1 = PROBE_V1.load(Ordering::Relaxed);

    if obs == target {
        probe_success(obs, w, h);
        return;
    }

    match stage {
        0 => {
            if obs == v1 {
                // The write had no effect: this object does not feed the vp.
                log(&format!(
                    "scene rect fix: candidate y={last_w} had no effect (vp y still {obs}); trying next source"
                ));
                advance_candidate(obs, w, h, target);
                return;
            }
            // Second controlled point to measure the slope sign.
            unsafe {
                let idx = PROBE_IDX.load(Ordering::Relaxed) as usize;
                let cand = SC_CAND[idx].load(Ordering::Relaxed);
                let addr = (cand >> 32) as u32 as usize;
                let kind = (cand >> 16) as u16 as u32;
                let b = target + PROBE_STEP;
                if !scene_write(addr, kind, b) {
                    advance_candidate(obs, w, h, target);
                    return;
                }
                PROBE_W2.store(b, Ordering::Relaxed);
                PROBE_V2.store(obs, Ordering::Relaxed);
                PROBE_STAGE.store(1, Ordering::Relaxed);
                log(&format!(
                    "scene rect fix: measuring slope: y {last_w}->{b}, vp y={obs}"
                ));
            }
        }
        1 => {
            if obs == prev_v {
                log("scene rect fix: probe write again had no effect; trying next source");
                advance_candidate(obs, w, h, target);
                return;
            }
            // slope sign from the +PROBE_STEP step; |slope| is 1 for the known
            // conventions, so the exact correction is one Newton step.
            let sign = if obs > prev_v { 1 } else { -1 };
            let y_final = last_w + (target - obs) * sign;
            log(&format!(
                "scene rect fix: slope {sign:+}, correcting y {last_w}->{y_final} (vp y {obs}, want {target})"
            ));
            unsafe {
                let idx = PROBE_IDX.load(Ordering::Relaxed) as usize;
                let cand = SC_CAND[idx].load(Ordering::Relaxed);
                let addr = (cand >> 32) as u32 as usize;
                let kind = (cand >> 16) as u16 as u32;
                if !scene_write(addr, kind, y_final) {
                    advance_candidate(obs, w, h, target);
                    return;
                }
                PROBE_W2.store(y_final, Ordering::Relaxed);
                PROBE_V2.store(obs, Ordering::Relaxed);
                PROBE_STAGE.store(2, Ordering::Relaxed);
            }
        }
        _ => {
            log(&format!(
                "scene rect fix: correction miss for ({w},{h}): wrote y={last_w}, vp y={obs}, want {target}"
            ));
            advance_candidate(obs, w, h, target);
        }
    }
}

/// The viewport is at the inset now: propagate the final rect y to the linked
/// object (RT and its widget carry the same rect) so drawing AND picking
/// agree, then disarm.
fn probe_success(obs: i32, w: i32, h: i32) {
    let idx = PROBE_IDX.load(Ordering::Relaxed) as usize;
    let cand = SC_CAND[idx].load(Ordering::Relaxed);
    let addr = (cand >> 32) as u32 as usize;
    let kind = (cand >> 16) as u16 as u32;
    let pair = SC_CAND_PAIR[idx].load(Ordering::Relaxed);
    let written = PROBE_W2.load(Ordering::Relaxed);
    unsafe {
        if pair > 0xffff {
            let pair_addr = (pair >> 16) as usize;
            let pair_kind = (pair & 0xffff) as u32;
            if pair_kind != kind && writable(pair_addr) {
                let _ = scene_write(pair_addr, pair_kind, written);
            }
        }
    }
    log(&format!(
        "scene rect fix: y={obs} reached for ({w},{h}); source 0x{addr:x} kind{kind} y={written}, synced 0x{pair:x}"
    ));
    disarm_probe(((w as u64) << 32) | (h as u32 as u64));
}

fn advance_candidate(obs: i32, w: i32, h: i32, target: i32) {
    if PROBE_TRIES.load(Ordering::Relaxed) >= 8 {
        give_up(obs, w, h);
        return;
    }
    let n = SC_CAND_N.load(Ordering::Relaxed) as usize;
    let mut idx = PROBE_IDX.load(Ordering::Relaxed) as usize + 1;
    while idx < n {
        unsafe {
            if arm_candidate(idx, obs, w, h, target) {
                return;
            }
        }
        idx += 1;
    }
    give_up(obs, w, h);
}

fn give_up(obs: i32, w: i32, h: i32) {
    let mut list = String::new();
    let n = SC_CAND_N.load(Ordering::Relaxed) as usize;
    for i in 0..n.min(SC_CAND.len()) {
        let c = SC_CAND[i].load(Ordering::Relaxed);
        if c != 0 {
            list.push_str(&format!(" 0x{:x}/k{}", (c >> 32) as u32, (c >> 16) as u16));
        }
    }
    log(&format!(
        "scene rect fix: giving up on ({w},{h}) at vp y={obs}; candidates:{list}"
    ));
    SC_EXHAUSTED_WH.store(((w as u64) << 32) | (h as u32 as u64), Ordering::Relaxed);
    disarm_probe(((w as u64) << 32) | (h as u32 as u64));
}

// ------------------------------------------------------------------- pass

struct Counters {
    regions_committed: usize,
    priv_regions: usize,
    img_regions: usize,
    map_regions: usize,
    priv_bytes: usize,
    img_bytes: usize,
    map_bytes: usize,
    bytes_walked: usize,
    bytes_read: usize,
    vt_hits: usize,
    parent_rejects: usize,
    children_rejects: usize,
    rect_hits: usize,
    centered: usize,
    fullspan_skips: usize,
    delta0_skips: usize,
    scene_hits: usize,
}

/// One watcher tick's worth of work. Resumes an active scan or starts a new
/// one when a repair was requested and the settle delay has passed.
pub unsafe fn widget_recenter_tick() {
    unsafe {
        // Repair requested and not running?
        if SCAN_ACTIVE.load(Ordering::Relaxed) == 0 {
            let recenter = RECENTER_PENDING.swap(0, Ordering::Relaxed) != 0;
            let scene = SCENE_FIX_PENDING.swap(0, Ordering::Relaxed) != 0;
            if !recenter && !scene {
                return;
            }
            if recenter && !scene && !config().menu_recenter {
                return;
            }
            let now = GetTickCount();
            if now.wrapping_sub(NOT_BEFORE.load(Ordering::Relaxed)) > 0x8000_0000
                || now.wrapping_sub(LAST_SCAN_END.load(Ordering::Relaxed)) < 1500
            {
                if recenter {
                    RECENTER_PENDING.store(1, Ordering::Relaxed);
                }
                if scene {
                    SCENE_FIX_PENDING.store(1, Ordering::Relaxed);
                }
                return;
            }
            SCAN_CURSOR.store(0x10000, Ordering::Relaxed);
            SCAN_ACTIVE.store(1, Ordering::Relaxed);
            SCAN_T0.store(now, Ordering::Relaxed);
            SCAN_FOR.store(if scene { 1 } else { 2 }, Ordering::Relaxed);
            CENSUS_PRIV.store(0, Ordering::Relaxed);
            CENSUS_IMG.store(0, Ordering::Relaxed);
            CENSUS_MAP.store(0, Ordering::Relaxed);
            CENSUS_PRIV_N.store(0, Ordering::Relaxed);
            CENSUS_IMG_N.store(0, Ordering::Relaxed);
            CENSUS_MAP_N.store(0, Ordering::Relaxed);
            CENSUS_WALKED.store(0, Ordering::Relaxed);
            CENSUS_READ.store(0, Ordering::Relaxed);
            CENSUS_VT.store(0, Ordering::Relaxed);
            CENSUS_RECT.store(0, Ordering::Relaxed);
            CENSUS_CENTERED.store(0, Ordering::Relaxed);
            CENSUS_PARENT_REJ.store(0, Ordering::Relaxed);
            CENSUS_CHILD_REJ.store(0, Ordering::Relaxed);
            CENSUS_SCENE.store(0, Ordering::Relaxed);
            CENSUS_FULLSPAN.store(0, Ordering::Relaxed);
            CENSUS_DELTA0.store(0, Ordering::Relaxed);
            // Fresh walk: clear stale collections from a previous one.
            RC_JOB_N.store(0, Ordering::Relaxed);
            SC_CAND_N.store(0, Ordering::Relaxed);
            if SCAN_LOG.load(Ordering::Relaxed) < 2 {
                // One-time sanity probe of the MemBasicInfo decoding.
                let mut m: MemBasicInfo = core::mem::zeroed();
                VirtualQuery(
                    0x10000 as *const c_void,
                    &mut m,
                    core::mem::size_of::<MemBasicInfo>(),
                );
                log(&format!(
                    "scan vq@0x10000: base=0x{:x} size=0x{:x} state=0x{:x} protect=0x{:x} type=0x{:x}",
                    m.base_address as usize, m.region_size, m.state, m.protect, m.ty
                ));
            }
        }

        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            SCAN_ACTIVE.store(0, Ordering::Relaxed);
            return;
        }
        let end = base + module_image_size(base);
        if end <= base {
            SCAN_ACTIVE.store(0, Ordering::Relaxed);
            return;
        }
        let (lw, lh) = logical_size();
        if lw <= 0 || lh <= 0 {
            SCAN_ACTIVE.store(0, Ordering::Relaxed);
            return;
        }

        let mut c = Counters {
            regions_committed: 0,
            priv_regions: 0,
            img_regions: 0,
            map_regions: 0,
            priv_bytes: 0,
            img_bytes: 0,
            map_bytes: 0,
            bytes_walked: 0,
            bytes_read: 0,
            vt_hits: 0,
            parent_rejects: 0,
            children_rejects: 0,
            rect_hits: 0,
            centered: 0,
            fullspan_skips: 0,
            delta0_skips: 0,
            scene_hits: 0,
        };
        let ms_t0 = GetTickCount();
        let for_scene = SCAN_FOR.load(Ordering::Relaxed) == 1;
        let done = scan_slice(base, end, lw, lh, &mut c, ms_t0);

        if done {
            apply_jobs(base, end, for_scene);
            SCAN_ACTIVE.store(0, Ordering::Relaxed);
            SCAN_CURSOR.store(0, Ordering::Relaxed);
            LAST_SCAN_END.store(GetTickCount(), Ordering::Relaxed);
            let ms = GetTickCount().wrapping_sub(SCAN_T0.load(Ordering::Relaxed));
            if SCAN_LOG.fetch_add(1, Ordering::Relaxed) < 64 {
                let gp = CENSUS_PRIV.load(Ordering::Relaxed);
                let gi = CENSUS_IMG.load(Ordering::Relaxed);
                let gm = CENSUS_MAP.load(Ordering::Relaxed);
                log(&format!(
                    "widget recenter: scan done in {ms} ms: committed {} MB (priv {} MB/{}r, img {} MB/{}r, map {} MB/{}r), walked {} MB read {} MB, vt-hits {}, rejects p/c/r {}/{}/{}, centered {}, fullspan {} delta0 {}, scene-cand {}",
                    (gp + gi + gm) / 1048576,
                    gp / 1048576,
                    CENSUS_PRIV_N.load(Ordering::Relaxed),
                    gi / 1048576,
                    CENSUS_IMG_N.load(Ordering::Relaxed),
                    gm / 1048576,
                    CENSUS_MAP_N.load(Ordering::Relaxed),
                    CENSUS_WALKED.load(Ordering::Relaxed) / 1048576,
                    CENSUS_READ.load(Ordering::Relaxed) / 1048576,
                    CENSUS_VT.load(Ordering::Relaxed),
                    CENSUS_PARENT_REJ.load(Ordering::Relaxed),
                    CENSUS_CHILD_REJ.load(Ordering::Relaxed),
                    CENSUS_RECT.load(Ordering::Relaxed),
                    CENSUS_CENTERED.load(Ordering::Relaxed),
                    CENSUS_FULLSPAN.load(Ordering::Relaxed),
                    CENSUS_DELTA0.load(Ordering::Relaxed),
                    CENSUS_SCENE.load(Ordering::Relaxed)
                ));
            }
            // A scene scan that found no candidates: count it, give up after a
            // few so the (broken) frame does not trigger endless re-scans.
            if for_scene && SC_CAND_N.load(Ordering::Relaxed) == 0 {
                let n = SC_EMPTY_SCANS.fetch_add(1, Ordering::Relaxed) + 1;
                if n >= 3
                    && let Some(wh) = broken_scene_wh()
                {
                    log("scene rect fix: no candidates found in 3 scans; giving up");
                    SC_EXHAUSTED_WH.store(wh, Ordering::Relaxed);
                }
            }
        }
    }
}

/// The (w,h) of the last misplaced scene vp, if it is still misplaced.
fn broken_scene_wh() -> Option<u64> {
    let v = LAST_SCENE_VP.load(Ordering::Relaxed);
    if v == u64::MAX {
        return None;
    }
    let y = (v >> 32) as u16 as i32;
    let w = (v >> 16) as u16 as i32;
    let h = v as u16 as i32;
    if y == scene_target_inset() {
        return None;
    }
    Some(((w as u64) << 32) | (h as u32 as u64))
}

/// Read one 64 KB chunk through ReadProcessMemory. Fails cleanly (false)
/// when any page in the chunk is inaccessible.
unsafe fn read_chunk(addr: usize, len: usize, buf: *mut u8) -> bool {
    unsafe {
        let mut got = 0usize;
        ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            buf as *mut c_void,
            len,
            &mut got,
        ) != 0
            && got == len
    }
}

/// Allocate the chunk buffer once (leaked intentionally, 64 KB).
unsafe fn chunk_buf() -> *mut u8 {
    unsafe {
        static PTR: AtomicU64 = AtomicU64::new(0);
        let p = PTR.load(Ordering::Relaxed);
        if p != 0 {
            return p as *mut u8;
        }
        let layout = core::alloc::Layout::from_size_align(CHUNK, 4096).unwrap();
        let p = std::alloc::alloc(layout);
        if p.is_null() {
            // Unreachable in practice; skip the walk rather than deref null.
            return core::ptr::null_mut();
        }
        PTR.store(p as usize as u64, Ordering::Relaxed);
        p
    }
}

#[inline]
unsafe fn buf_u32(buf: *const u8, i: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(buf.add(i) as *const u32) }
}

/// Scan a time slice starting at SCAN_CURSOR. Returns true when the walk is
/// complete (cursor reset).
unsafe fn scan_slice(base: usize, end: usize, lw: i32, lh: i32, c: &mut Counters, t0: u32) -> bool {
    unsafe {
        let buf = chunk_buf();
        if buf.is_null() {
            return true;
        }
        let mut addr = SCAN_CURSOR.load(Ordering::Relaxed) as usize;
        if addr < 0x10000 {
            addr = 0x10000;
        }
        let mut cache = RegionCache::new();
        let olds = canvas_history();
        let scene_whs: Vec<(i32, i32)> = {
            let n = SCENE_VP_HIST_N.load(Ordering::Relaxed) as usize;
            let mut out = Vec::new();
            for i in 0..n.min(SCENE_VP_HIST.len()) {
                let v = SCENE_VP_HIST[i].load(Ordering::Relaxed);
                if v != u64::MAX {
                    let wh = ((v >> 16) as u16 as i32, v as u16 as i32);
                    if !out.contains(&wh) {
                        out.push(wh);
                    }
                }
            }
            out
        };
        let do_scene = config().scene_rect_fix && !scene_whs.is_empty();
        let do_recenter = config().menu_recenter;

        'outer: loop {
            // The region loop must honor the time slice too: with a fragmented
            // address space the VirtualQuery enumeration alone can take seconds,
            // and this thread also polls F11 and the window geometry.
            if GetTickCount().wrapping_sub(t0) > SLICE_MS {
                SCAN_CURSOR.store(addr as u64, Ordering::Relaxed);
                return false;
            }
            let mut m: MemBasicInfo = core::mem::zeroed();
            if VirtualQuery(
                addr as *const c_void,
                &mut m,
                core::mem::size_of::<MemBasicInfo>(),
            ) != core::mem::size_of::<MemBasicInfo>()
            {
                SCAN_CURSOR.store(0, Ordering::Relaxed);
                return true;
            }
            let rbase = m.base_address as usize;
            let rsize = m.region_size.max(1);
            if rbase.saturating_add(rsize) <= addr {
                SCAN_CURSOR.store(0, Ordering::Relaxed);
                return true;
            }
            if rbase >= 0xFFFF_0000 {
                SCAN_CURSOR.store(0, Ordering::Relaxed);
                return true;
            }
            if m.state == MEM_COMMIT {
                c.regions_committed += 1;
                match m.ty {
                    MEM_PRIVATE => {
                        c.priv_regions += 1;
                        c.priv_bytes += rsize;
                        CENSUS_PRIV.fetch_add(rsize as u64, Ordering::Relaxed);
                        CENSUS_PRIV_N.fetch_add(1, Ordering::Relaxed);
                    }
                    0x0100_0000 => {
                        c.img_regions += 1;
                        c.img_bytes += rsize;
                        CENSUS_IMG.fetch_add(rsize as u64, Ordering::Relaxed);
                        CENSUS_IMG_N.fetch_add(1, Ordering::Relaxed);
                    }
                    0x0004_0000 => {
                        c.map_regions += 1;
                        c.map_bytes += rsize;
                        CENSUS_MAP.fetch_add(rsize as u64, Ordering::Relaxed);
                        CENSUS_MAP_N.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            addr = rbase.saturating_add(rsize);
            // Walk committed private memory only (widget/RT objects live in the
            // engine heap); everything else is only counted in the census.
            if m.state != MEM_COMMIT || m.ty != MEM_PRIVATE {
                continue;
            }
            if m.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0 {
                continue;
            }
            if !matches!(
                m.protect & 0xff,
                PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
            ) {
                continue;
            }

            // Walk this region in chunks read via RPM. Resume support: if the
            // cursor points into this region, start there.
            let cur = SCAN_CURSOR.load(Ordering::Relaxed) as usize;
            let mut off = if cur > rbase && cur < rbase + rsize {
                cur - rbase
            } else {
                0
            };
            off &= !7;
            c.bytes_walked += rsize - off;
            CENSUS_WALKED.fetch_add((rsize - off) as u64, Ordering::Relaxed);
            while off < rsize {
                if GetTickCount().wrapping_sub(t0) > SLICE_MS {
                    SCAN_CURSOR.store((rbase + off) as u64, Ordering::Relaxed);
                    return false;
                }
                let len = (rsize - off).min(CHUNK);
                if !read_chunk(rbase + off, len, buf) {
                    // Region shrank/changed under us; skip the rest of it.
                    break;
                }
                c.bytes_read += len;
                CENSUS_READ.fetch_add(len as u64, Ordering::Relaxed);
                let a0 = rbase + off;
                let lim = len.saturating_sub(0x58);
                let mut i = 0usize;
                while i <= lim {
                    // Fast reject: the first field of a widget/RT is its vtable.
                    if !in_image(buf_u32(buf, i), base, end) {
                        i += 8;
                        continue;
                    }
                    c.vt_hits += 1;
                    CENSUS_VT.fetch_add(1, Ordering::Relaxed);
                    let p = a0 + i;
                    let pxy = buf_u32(buf, i + 0x10);
                    let pwh = buf_u32(buf, i + 0x14);

                    // ---- scene-rect candidates (rect check is buffer-only).
                    if do_scene {
                        // kind 1: render-target object, u32 rect at +0x28.
                        let rx = buf_u32(buf, i + 0x28);
                        let ry = buf_u32(buf, i + 0x2c);
                        let rw = buf_u32(buf, i + 0x30);
                        let rh = buf_u32(buf, i + 0x34);
                        if rx == 0 && rw > 0 && rw <= 0x8000 && rh > 0 && rh <= 0x8000 {
                            for &(sw, sh) in scene_whs.iter() {
                                if rw == sw as u32 && rh == sh as u32 {
                                    // Validated: vtable in image (already), rect
                                    // shape, and a widget backlink that reads.
                                    let wptr = buf_u32(buf, i + 0x54) as usize;
                                    let wvt = if wptr > 0x10000 { rpm_u32(wptr) } else { None };
                                    if wvt.is_some()
                                        && in_image(wvt.unwrap(), base, end)
                                        && (cache.hit(wptr) || region_of(wptr).is_some())
                                    {
                                        // RT and its linked widget as separate
                                        // candidates: the vp may be fed from
                                        // either; pair = (addr << 16 | kind).
                                        push_scene_cand(p, 1, ry as i32, (wptr as u64) << 16 | 2);
                                        c.scene_hits += 1;
                                        CENSUS_SCENE.fetch_add(1, Ordering::Relaxed);
                                        let wxy = match rpm_u32(wptr + 0x10) {
                                            Some(v) => v,
                                            None => break,
                                        };
                                        let wy = (wxy >> 16) as u16 as i32;
                                        push_scene_cand(wptr, 2, wy, (p as u64) << 16 | 1);
                                        c.scene_hits += 1;
                                        CENSUS_SCENE.fetch_add(1, Ordering::Relaxed);
                                        break;
                                    }
                                }
                            }
                        }
                        // kind 2: widget-shaped i16 rect at +0x10..+0x17.
                        let wx = (pxy & 0xffff) as i32;
                        let wy = (pxy >> 16) as i32;
                        let ww = (pwh & 0xffff) as i32;
                        let wh = (pwh >> 16) as i32;
                        if wx == 0 && ww > 0 && ww <= 0x8000 && wh > 0 && wh <= 0x8000 {
                            for &(sw, sh) in scene_whs.iter() {
                                if ww == sw && wh == sh {
                                    push_scene_cand(p, 2, wy, 0);
                                    c.scene_hits += 1;
                                    CENSUS_SCENE.fetch_add(1, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                    }

                    if !do_recenter {
                        i += 8;
                        continue;
                    }

                    // ---- recenter candidates: the (cheap) centered-rect check
                    // runs on buffer data; the expensive validation (parent
                    // membership, children vector) only for real matches.
                    let packed_xy = pxy;
                    let packed_wh = pwh;
                    let x = (packed_xy & 0xffff) as i32;
                    let y = (packed_xy >> 16) as i32;
                    let w = (packed_wh & 0xffff) as i32;
                    let h = (packed_wh >> 16) as i32;
                    if w <= 0 || h <= 0 {
                        i += 8;
                        continue;
                    }
                    let mut matched_old: Option<(i32, i32)> = None;
                    for &(ow, oh) in olds.iter() {
                        if ow <= 0 || oh <= 0 || (ow == lw && oh == lh) {
                            continue;
                        }
                        if (2 * x + w == ow || 2 * x + w == ow - 1)
                            && (2 * y + h == oh || 2 * y + h == oh - 1)
                        {
                            matched_old = Some((ow, oh));
                            break;
                        }
                    }
                    let Some((ow, oh)) = matched_old else {
                        i += 8;
                        continue;
                    };
                    c.rect_hits += 1;
                    CENSUS_RECT.fetch_add(1, Ordering::Relaxed);
                    if w == ow && h == oh {
                        // Full-span widgets are stretch targets, not centers.
                        c.fullspan_skips += 1;
                        CENSUS_FULLSPAN.fetch_add(1, Ordering::Relaxed);
                        i += 8;
                        continue;
                    }
                    let nx = (lw - w) / 2;
                    let ny = (lh - h) / 2;
                    if nx == x && ny == y {
                        c.delta0_skips += 1;
                        CENSUS_DELTA0.fetch_add(1, Ordering::Relaxed);
                        i += 8;
                        continue;
                    }
                    // Validate: parent membership + sane children vector.
                    let parent = buf_u32(buf, i + 4) as usize;
                    if parent != 0 && !member_of_parent(parent, p, &mut cache) {
                        c.parent_rejects += 1;
                        CENSUS_PARENT_REJ.fetch_add(1, Ordering::Relaxed);
                        i += 8;
                        continue;
                    }
                    if widget_children(p, base, end, &mut cache).is_none() {
                        c.children_rejects += 1;
                        CENSUS_CHILD_REJ.fetch_add(1, Ordering::Relaxed);
                        i += 8;
                        continue;
                    }
                    if RC_JOB_N.load(Ordering::Relaxed) < MAX_FIXES as u32 {
                        push_job(
                            &RC_JOBS,
                            &RC_JOB_N,
                            ((p as u64) << 32)
                                | (((nx - x) as u16 as u64) << 16)
                                | ((ny - y) as u16 as u64),
                        );
                        c.centered += 1;
                        CENSUS_CENTERED.fetch_add(1, Ordering::Relaxed);
                    }
                    i += 8;
                }
                off += len;
            }
            continue 'outer;
        }
    }
}

fn push_scene_cand(addr: usize, kind: u32, y: i32, pair: u64) {
    let n = SC_CAND_N.load(Ordering::Relaxed) as usize;
    if n >= SC_CAND.len() {
        return;
    }
    // Skip duplicates (same address already collected).
    for i in 0..n {
        if (SC_CAND[i].load(Ordering::Relaxed) >> 32) as u32 as usize == addr {
            return;
        }
    }
    SC_CAND[n].store(
        ((addr as u64) << 32) | ((kind as u64) << 16) | (y as u16 as u64),
        Ordering::Relaxed,
    );
    SC_CAND_PAIR[n].store(pair, Ordering::Relaxed);
    SC_CAND_N.store(n as u32 + 1, Ordering::Relaxed);
}

// ------------------------------------------------------- recenter jobs

/// Re-center jobs collected during the walk, applied after it completes (the
/// watcher thread is the only writer, so plain atomics suffice).
/// Packed: `addr << 32 | (dx as u16) << 16 | (dy as u16)`
/// (deltas fit i16: canvases are < 16384 px).
static RC_JOBS: [AtomicU64; MAX_FIXES] = [const { AtomicU64::new(0) }; MAX_FIXES];
static RC_JOB_N: AtomicU32 = AtomicU32::new(0);
static RC_FIX_LOG: AtomicU32 = AtomicU32::new(0);

#[inline]
fn push_job(slots: &[AtomicU64; MAX_FIXES], counter: &AtomicU32, v: u64) {
    let n = counter.load(Ordering::Relaxed) as usize;
    if n < MAX_FIXES {
        slots[n].store(v, Ordering::Relaxed);
        counter.store(n as u32 + 1, Ordering::Relaxed);
    }
}

/// Apply the fixes collected during the walk, called once after the final
/// slice. Every write is re-validated first; both repairs are idempotent.
unsafe fn apply_jobs(base: usize, end: usize, scene_scan: bool) {
    unsafe {
        let mut cache = RegionCache::new();

        // ---- widget re-centering.
        let n = RC_JOB_N.swap(0, Ordering::Relaxed).min(MAX_FIXES as u32) as usize;
        for i in 0..n {
            let j = RC_JOBS[i].swap(0, Ordering::Relaxed);
            if j == 0 {
                continue;
            }
            let root = (j >> 32) as u32 as usize;
            let dx = (j >> 16) as u16 as i16 as i32;
            let dy = j as u16 as i16 as i32;
            if dx == 0 && dy == 0 {
                continue;
            }
            let vt = match rpm_u32(root) {
                Some(v) => v,
                None => continue,
            };
            if !(in_image(vt, base, end) && (cache.hit(root) || region_of(root).is_some())) {
                continue;
            }
            let moved = shift_tree(root, dx, dy, base, end, &mut cache);
            if RC_FIX_LOG.fetch_add(1, Ordering::Relaxed) < 32 {
                log(&format!(
                    "widget recenter: widget 0x{root:x} moved by ({dx},{dy}), {moved} rects shifted"
                ));
            }
        }

        // ---- scene-rect repair: arm the feedback probe on the first candidate
        // if the scene is currently misplaced.
        if scene_scan {
            let n = SC_CAND_N.load(Ordering::Relaxed) as usize;
            if n > 0
                && PROBE_WH.load(Ordering::Relaxed) == 0
                && let Some(wh) = broken_scene_wh()
            {
                let w = (wh >> 32) as i32;
                let h = wh as u32 as i32;
                let obs = ((LAST_SCENE_VP.load(Ordering::Relaxed) >> 32) as u16) as i32;
                let target = scene_target_inset();
                for i in 0..n {
                    if arm_candidate(i, obs, w, h, target) {
                        break;
                    }
                }
            }
        }
    }
}
