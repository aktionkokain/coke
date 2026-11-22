//! Low-cost GL/frame counters exposed to `coke/` profiler mods.
//!
//! The proxy already intercepts a few GL calls; this module adds counters for
//! the calls that dominate the engine's frame (display lists, texture binds,
//! matrix stack traffic, draw submission and texture upload) plus frame/swap
//! timing. Counting is always on and costs a single atomic increment. Per-call
//! QPC timing is opt-in (`coke_prof_set_timing(1)`) because 10k `glCallList`
//! calls per frame would otherwise distort the measurement.
//!
//! Frame time is measured in two parts: `swap_ns` is time spent inside the
//! driver's present, and `cpu_frame_ns` is everything between the previous
//! present returning and this frame reaching `wglSwapBuffers` (engine work plus
//! the proxy's overlays). That excludes the present/vsync wait, so it is the
//! cost of producing a frame rather than the wall-clock frame interval.
//!
//! The data is published as a versioned C struct so a mod can read it without
//! linking to the proxy. `coke_prof_version` returns the layout version; a mod
//! must reject a version it does not know.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use coke_host::{COKE_PROF_VERSION, CokeProfStats};

// ---- counters ------------------------------------------------------------

static FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAME_NS: AtomicU64 = AtomicU64::new(0);
static LAST_FRAME_NS: AtomicU64 = AtomicU64::new(0);

static SWAP_CALLS: AtomicU64 = AtomicU64::new(0);
static SWAP_NS: AtomicU64 = AtomicU64::new(0);
static LAST_SWAP_NS: AtomicU64 = AtomicU64::new(0);

static CALL_LIST: AtomicU64 = AtomicU64::new(0);
static CALL_LIST_NS: AtomicU64 = AtomicU64::new(0);
static BIND_TEXTURE: AtomicU64 = AtomicU64::new(0);
static BIND_TEXTURE_NS: AtomicU64 = AtomicU64::new(0);

static BEGIN_END: AtomicU64 = AtomicU64::new(0);
static PUSH_POP: AtomicU64 = AtomicU64::new(0);
static DRAW_ARRAYS: AtomicU64 = AtomicU64::new(0);
static DRAW_ELEMENTS: AtomicU64 = AtomicU64::new(0);
static VERTICES: AtomicU64 = AtomicU64::new(0);

static TEX_UPLOAD: AtomicU64 = AtomicU64::new(0);
static TEX_UPLOAD_BYTES: AtomicU64 = AtomicU64::new(0);
static READ_PIXELS: AtomicU64 = AtomicU64::new(0);
static READ_PIXELS_NS: AtomicU64 = AtomicU64::new(0);
static COPY_TEX: AtomicU64 = AtomicU64::new(0);

static LISTS_NEW: AtomicU64 = AtomicU64::new(0);
static LISTS_CALLED: AtomicU64 = AtomicU64::new(0);
static LISTS_DELETED: AtomicU64 = AtomicU64::new(0);

static CPU_FRAMES: AtomicU64 = AtomicU64::new(0);
static CPU_FRAME_NS: AtomicU64 = AtomicU64::new(0);
static LAST_CPU_FRAME_NS: AtomicU64 = AtomicU64::new(0);

/// End of the previous `wglSwapBuffers` call (after the real swap returned).
static LAST_SWAP_END: AtomicU64 = AtomicU64::new(0);

/// 0 = counters only, 1 = also time the expensive calls with QPC.
static TIMING: AtomicU32 = AtomicU32::new(0);

#[inline]
pub fn timing() -> bool {
    TIMING.load(Ordering::Relaxed) != 0
}

/// Convert QPC ticks to nanoseconds without overflowing on short intervals.
#[inline]
pub fn ticks_to_ns(ticks: u64) -> u64 {
    let freq = unsafe { qpc_freq() };
    if freq == 0 {
        return 0;
    }
    // QPC deltas are small (a frame or one GL call). Split the multiply so a
    // 64-bit tick count cannot overflow on 32-bit Windows.
    let secs = ticks / freq;
    let rem = ticks % freq;
    secs.saturating_mul(1_000_000_000)
        .saturating_add(rem.saturating_mul(1_000_000_000) / freq)
}

#[inline]
pub unsafe fn now() -> u64 {
    unsafe { qpc() }
}

pub fn reset() {
    for a in [
        &FRAMES,
        &FRAME_NS,
        &LAST_FRAME_NS,
        &SWAP_CALLS,
        &SWAP_NS,
        &LAST_SWAP_NS,
        &CALL_LIST,
        &CALL_LIST_NS,
        &BIND_TEXTURE,
        &BIND_TEXTURE_NS,
        &BEGIN_END,
        &PUSH_POP,
        &DRAW_ARRAYS,
        &DRAW_ELEMENTS,
        &VERTICES,
        &TEX_UPLOAD,
        &TEX_UPLOAD_BYTES,
        &READ_PIXELS,
        &READ_PIXELS_NS,
        &COPY_TEX,
        &LISTS_NEW,
        &LISTS_CALLED,
        &LISTS_DELETED,
        &CPU_FRAMES,
        &CPU_FRAME_NS,
        &LAST_CPU_FRAME_NS,
    ] {
        a.store(0, Ordering::Relaxed);
    }
}

pub fn snapshot(out: &mut CokeProfStats) {
    out.version = COKE_PROF_VERSION;
    out.size = core::mem::size_of::<CokeProfStats>() as u32;
    out.frames = FRAMES.load(Ordering::Relaxed);
    out.frame_ns = FRAME_NS.load(Ordering::Relaxed);
    out.last_frame_ns = LAST_FRAME_NS.load(Ordering::Relaxed);
    out.swap_calls = SWAP_CALLS.load(Ordering::Relaxed);
    out.swap_ns = SWAP_NS.load(Ordering::Relaxed);
    out.last_swap_ns = LAST_SWAP_NS.load(Ordering::Relaxed);
    out.call_list = CALL_LIST.load(Ordering::Relaxed);
    out.call_list_ns = CALL_LIST_NS.load(Ordering::Relaxed);
    out.bind_texture = BIND_TEXTURE.load(Ordering::Relaxed);
    out.bind_texture_ns = BIND_TEXTURE_NS.load(Ordering::Relaxed);
    out.begin_end = BEGIN_END.load(Ordering::Relaxed);
    out.push_pop = PUSH_POP.load(Ordering::Relaxed);
    out.draw_arrays = DRAW_ARRAYS.load(Ordering::Relaxed);
    out.draw_elements = DRAW_ELEMENTS.load(Ordering::Relaxed);
    out.vertices = VERTICES.load(Ordering::Relaxed);
    out.tex_upload = TEX_UPLOAD.load(Ordering::Relaxed);
    out.tex_upload_bytes = TEX_UPLOAD_BYTES.load(Ordering::Relaxed);
    out.read_pixels = READ_PIXELS.load(Ordering::Relaxed);
    out.read_pixels_ns = READ_PIXELS_NS.load(Ordering::Relaxed);
    out.copy_tex = COPY_TEX.load(Ordering::Relaxed);
    out.lists_new = LISTS_NEW.load(Ordering::Relaxed);
    out.lists_called = LISTS_CALLED.load(Ordering::Relaxed);
    out.lists_deleted = LISTS_DELETED.load(Ordering::Relaxed);
    out.cpu_frames = CPU_FRAMES.load(Ordering::Relaxed);
    out.cpu_frame_ns = CPU_FRAME_NS.load(Ordering::Relaxed);
    out.last_cpu_frame_ns = LAST_CPU_FRAME_NS.load(Ordering::Relaxed);
}

// ---- instrumentation helpers ------------------------------------------

#[inline]
pub fn call_list() {
    CALL_LIST.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn call_list_ns(ns: u64) {
    CALL_LIST_NS.fetch_add(ns, Ordering::Relaxed);
}

#[inline]
pub fn bind_texture() {
    BIND_TEXTURE.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bind_texture_ns(ns: u64) {
    BIND_TEXTURE_NS.fetch_add(ns, Ordering::Relaxed);
}

#[inline]
pub fn begin_end() {
    BEGIN_END.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn push_pop() {
    PUSH_POP.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn draw_arrays(vertices: u32) {
    DRAW_ARRAYS.fetch_add(1, Ordering::Relaxed);
    VERTICES.fetch_add(vertices as u64, Ordering::Relaxed);
}

#[inline]
pub fn draw_elements(vertices: u32) {
    DRAW_ELEMENTS.fetch_add(1, Ordering::Relaxed);
    VERTICES.fetch_add(vertices as u64, Ordering::Relaxed);
}

#[inline]
pub fn tex_upload(bytes: u64) {
    TEX_UPLOAD.fetch_add(1, Ordering::Relaxed);
    TEX_UPLOAD_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

#[inline]
pub fn read_pixels() {
    READ_PIXELS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn read_pixels_ns(ns: u64) {
    READ_PIXELS_NS.fetch_add(ns, Ordering::Relaxed);
}

#[inline]
pub fn copy_tex() {
    COPY_TEX.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn list_new() {
    LISTS_NEW.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn list_called() {
    LISTS_CALLED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn list_deleted() {
    LISTS_DELETED.fetch_add(1, Ordering::Relaxed);
}

/// Called at the top of `wglSwapBuffers`, when the frame's work is done.
/// Returns the entry timestamp and records the CPU-side cost of this frame:
/// the time since the previous swap returned, which covers the engine's work
/// plus the proxy's own overlays but excludes the driver's present/VSync wait.
pub unsafe fn frame_begin() -> u64 {
    unsafe {
        let entry = now();
        let prev_end = LAST_SWAP_END.load(Ordering::Relaxed);
        if prev_end != 0 {
            let cpu = ticks_to_ns(entry.saturating_sub(prev_end));
            LAST_CPU_FRAME_NS.store(cpu, Ordering::Relaxed);
            CPU_FRAME_NS.fetch_add(cpu, Ordering::Relaxed);
            CPU_FRAMES.fetch_add(1, Ordering::Relaxed);
        }
        entry
    }
}

/// Called after the real swap. Records the present time and the wall-clock
/// swap-to-swap interval (engine time plus present time).
pub unsafe fn frame_end(entry: u64) {
    unsafe {
        let end = now();
        let swap = ticks_to_ns(end.saturating_sub(entry));
        LAST_SWAP_NS.store(swap, Ordering::Relaxed);
        SWAP_NS.fetch_add(swap, Ordering::Relaxed);
        SWAP_CALLS.fetch_add(1, Ordering::Relaxed);

        let prev = LAST_SWAP_END.swap(end, Ordering::Relaxed);
        if prev != 0 {
            let frame = ticks_to_ns(end.saturating_sub(prev));
            LAST_FRAME_NS.store(frame, Ordering::Relaxed);
            FRAME_NS.fetch_add(frame, Ordering::Relaxed);
            FRAMES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---- exports --------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_version() -> u32 {
    COKE_PROF_VERSION
}

#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_set_timing(on: u32) {
    TIMING.store(u32::from(on != 0), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_reset() {
    reset();
}

/// Legacy version 2 export. The caller provides the full frozen layout.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_prof_read(out: *mut CokeProfStats) -> i32 {
    unsafe { coke_prof_read_sized(out, size_of::<CokeProfStats>() as u32) }
}

/// Copy version 2 only when the caller declares enough writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_prof_read_sized(out: *mut CokeProfStats, capacity: u32) -> i32 {
    unsafe {
        if (capacity as usize) < size_of::<CokeProfStats>()
            || mod_span(out.cast::<u8>(), size_of::<CokeProfStats>(), true).is_none()
        {
            return 0;
        }
        let mut s = CokeProfStats::default();
        snapshot(&mut s);
        core::ptr::write_unaligned(out, s);
        1
    }
}

/// Convenience for a mod that only wants one counter. Kept out of the struct
/// path so a future layout change does not break it.
#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_last_frame_ns() -> u64 {
    LAST_FRAME_NS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_last_swap_ns() -> u64 {
    LAST_SWAP_NS.load(Ordering::Relaxed)
}

/// CPU-side cost of the most recent frame, excluding the present/VSync wait.
#[unsafe(no_mangle)]
pub extern "system" fn coke_prof_last_cpu_frame_ns() -> u64 {
    LAST_CPU_FRAME_NS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_read_leaves_short_buffer_and_guards_untouched() {
        let mut bytes = [0xa5u8; 240];
        let expected = bytes;
        let output = unsafe { bytes.as_mut_ptr().add(8).cast() };
        assert_eq!(unsafe { coke_prof_read_sized(output, 215) }, 0);
        assert_eq!(bytes, expected);
        assert_eq!(unsafe { coke_prof_read_sized(output, 216) }, 1);
        assert_eq!(&bytes[..8], &expected[..8]);
        assert_eq!(&bytes[224..], &expected[224..]);
        let snapshot: CokeProfStats = unsafe { output.read_unaligned() };
        assert_eq!((snapshot.version, snapshot.size), (2, 216));
    }
}
