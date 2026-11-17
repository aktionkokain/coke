//! `coke-prof` — in-process profiler for Combat Mission.
//!
//! The mod is loaded by the proxy's `coke/` loader. It measures:
//!   * frame time / percentiles and swap time,
//!   * per-thread CPU time via `GetThreadTimes`,
//!   * GL call counts and (optionally) call time, read from the proxy's
//!     `coke_prof_*` exports,
//!   * instruction-pointer samples of the GL thread, mapped to function start
//!     addresses from `analysis/decompiled` (addresses only; see symbols.rs).
//!
//! The HUD is drawn from `coke_hud` (host API v3), i.e. after every world-space
//! overlay. On older hosts it falls back to `coke_frame`.
//!
//! Hotkeys: F7 reset counters, F8 toggle HUD. The log is `coke-prof.csv` next
//! to the game executable.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc, clippy::too_many_arguments)]

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

mod symbols;

// ---------------------------------------------------------------- host ABI

use coke_host::CokeHost;

static HOST: OnceLock<CokeHost> = OnceLock::new();
static HUD_MODE: AtomicU32 = AtomicU32::new(0);

const _: () = assert!(core::mem::size_of::<CokeHost>() == 132);
const _: () = assert!(core::mem::size_of::<CokeProfStats>() == 216);

fn host() -> Option<&'static CokeHost> {
    HOST.get()
}

unsafe fn log(msg: &str) {
    unsafe {
        if let Some(f) = host().and_then(|h| h.log) {
            let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
            f(c.as_ptr());
        }
    }
}

unsafe fn draw_text(x: i32, y: i32, text: &str, rgb: u32) {
    unsafe {
        if let Some(f) = host().and_then(|h| h.draw_text) {
            let c = CString::new(text.replace('\0', " ")).unwrap_or_default();
            f(x, y, c.as_ptr(), rgb);
        }
    }
}

fn fps() -> f32 {
    unsafe { host().and_then(|h| h.get_fps).map(|f| f()).unwrap_or(0.0) }
}

fn module_base() -> u32 {
    unsafe {
        host()
            .and_then(|h| h.module_base)
            .map(|f| f())
            .unwrap_or(0x0040_0000)
    }
}

// --------------------------------------------------------- proxy profiler

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
    pub cpu_frames: u64,
    pub cpu_frame_ns: u64,
    pub last_cpu_frame_ns: u64,
}

type ProfReadFn = unsafe extern "system" fn(*mut CokeProfStats) -> i32;
type ProfVoidFn = unsafe extern "system" fn();
type ProfTimingFn = unsafe extern "system" fn(u32);
type ProfU64Fn = unsafe extern "system" fn() -> u64;

static PROF_READ: AtomicUsize = AtomicUsize::new(0);
static PROF_RESET: AtomicUsize = AtomicUsize::new(0);
static PROF_TIMING: AtomicUsize = AtomicUsize::new(0);
static PROF_LAST_CPU: AtomicUsize = AtomicUsize::new(0);

unsafe fn resolve_profiler() {
    unsafe {
        let h = GetModuleHandleW(wide("opengl32.dll").as_ptr());
        let h = if h == 0 {
            GetModuleHandleW(core::ptr::null())
        } else {
            h
        };
        if h == 0 {
            return;
        }
        let name = |s: &str| CString::new(s).unwrap_or_default();
        PROF_READ.store(
            GetProcAddress(h, name("coke_prof_read").as_ptr()) as usize,
            Ordering::Relaxed,
        );
        PROF_RESET.store(
            GetProcAddress(h, name("coke_prof_reset").as_ptr()) as usize,
            Ordering::Relaxed,
        );
        PROF_TIMING.store(
            GetProcAddress(h, name("coke_prof_set_timing").as_ptr()) as usize,
            Ordering::Relaxed,
        );
        PROF_LAST_CPU.store(
            GetProcAddress(h, name("coke_prof_last_cpu_frame_ns").as_ptr()) as usize,
            Ordering::Relaxed,
        );
    }
}

/// CPU-side frame time of the current frame, or 0 when the proxy is too old.
fn last_cpu_frame_ns() -> u64 {
    let p = PROF_LAST_CPU.load(Ordering::Relaxed);
    if p == 0 {
        return 0;
    }
    let f: ProfU64Fn = unsafe { core::mem::transmute(p) };
    unsafe { f() }
}

fn read_stats(out: &mut CokeProfStats) -> bool {
    let p = PROF_READ.load(Ordering::Relaxed);
    if p == 0 {
        return false;
    }
    let f: ProfReadFn = unsafe { core::mem::transmute(p) };
    unsafe {
        f(out) != 0
            && out.version == 2
            && out.size as usize == core::mem::size_of::<CokeProfStats>()
    }
}

unsafe fn reset_profiler() {
    unsafe {
        let p = PROF_RESET.load(Ordering::Relaxed);
        if p != 0 {
            let f: ProfVoidFn = core::mem::transmute(p);
            f();
        }
    }
}

unsafe fn set_timing(on: bool) {
    unsafe {
        let p = PROF_TIMING.load(Ordering::Relaxed);
        if p != 0 {
            let f: ProfTimingFn = core::mem::transmute(p);
            f(u32::from(on));
        }
    }
}

// ---------------------------------------------------------------- config

#[derive(Clone, Copy)]
struct Config {
    enabled: bool,
    hud: bool,
    sample: bool,
    sample_hz: u32,
    interval_ms: u64,
    top: usize,
    log_csv: bool,
    timing: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            hud: true,
            sample: true,
            sample_hz: 25,
            interval_ms: 1000,
            top: 3,
            log_csv: true,
            timing: false,
        }
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn load_config() -> Config {
    let mut cfg = Config::default();
    let mut candidates = Vec::new();
    if let Some(d) = exe_dir() {
        candidates.push(d.join("coke-prof.ini"));
        candidates.push(d.join("coke").join("coke-prof.ini"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("coke-prof.ini"));
    }
    let Some(path) = candidates.into_iter().find(|p| p.is_file()) else {
        return cfg;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        let yes = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        let num = v.parse::<u64>().unwrap_or(0);
        match k.as_str() {
            "enabled" => cfg.enabled = yes,
            "hud" | "show_hud" => cfg.hud = yes,
            "sample" | "sampling" => cfg.sample = yes,
            "sample_hz" => cfg.sample_hz = (num as u32).clamp(1, 200),
            "interval_ms" | "interval" => cfg.interval_ms = num.clamp(250, 10_000),
            "top" => cfg.top = (num as usize).clamp(1, 8),
            "log" | "csv" => cfg.log_csv = yes,
            "timing" | "time_gl" => cfg.timing = yes,
            _ => {}
        }
    }
    cfg
}

// ---------------------------------------------------------------- Win32

type Handle = isize;

#[repr(C)]
struct ThreadEntry32 {
    dw_size: u32,
    cnt_usage: u32,
    th32_thread_id: u32,
    th32_owner_process_id: u32,
    tp_base_pri: i32,
    tp_delta_pri: i32,
    dw_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    lo: u32,
    hi: u32,
}

impl FileTime {
    fn get(self) -> u64 {
        ((self.hi as u64) << 32) | self.lo as u64
    }
}

#[repr(C, align(16))]
struct Context {
    buf: [u8; 0x2cc],
}

unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Thread32First(snap: Handle, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snap: Handle, entry: *mut ThreadEntry32) -> i32;
    fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn GetThreadTimes(
        h: Handle,
        create: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GetProcessTimes(
        h: Handle,
        create: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetTickCount64() -> u64;
    fn QueryPerformanceCounter(v: *mut i64) -> i32;
    fn QueryPerformanceFrequency(v: *mut i64) -> i32;
    fn SuspendThread(h: Handle) -> u32;
    fn ResumeThread(h: Handle) -> u32;
    fn GetThreadContext(h: Handle, ctx: *mut Context) -> i32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(h: Handle, name: *const c_char) -> *const c_void;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn qpc() -> u64 {
    let mut v: i64 = 0;
    unsafe { QueryPerformanceCounter(&mut v) };
    v as u64
}

static QPC_FREQ: AtomicU64 = AtomicU64::new(0);
fn qpc_freq() -> u64 {
    let f = QPC_FREQ.load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let mut v: i64 = 0;
    unsafe { QueryPerformanceFrequency(&mut v) };
    let v = v.max(1) as u64;
    QPC_FREQ.store(v, Ordering::Relaxed);
    v
}

fn cpu_now() -> u64 {
    unsafe {
        let mut c = FileTime::default();
        let mut e = FileTime::default();
        let mut k = FileTime::default();
        let mut u = FileTime::default();
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) == 0 {
            return 0;
        }
        k.get().saturating_add(u.get())
    }
}

fn thread_cpu(tid: u32) -> Option<u64> {
    unsafe {
        let h = OpenThread(0x0040, 0, tid); // THREAD_QUERY_INFORMATION
        if h == 0 {
            return None;
        }
        let mut c = FileTime::default();
        let mut e = FileTime::default();
        let mut k = FileTime::default();
        let mut u = FileTime::default();
        let ok = GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u);
        CloseHandle(h);
        if ok == 0 {
            None
        } else {
            Some(k.get().saturating_add(u.get()))
        }
    }
}

fn thread_ids() -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(0x0000_0004, 0); // TH32CS_SNAPTHREAD
        if snap == -1 {
            return out;
        }
        let mut e = ThreadEntry32 {
            dw_size: core::mem::size_of::<ThreadEntry32>() as u32,
            cnt_usage: 0,
            th32_thread_id: 0,
            th32_owner_process_id: 0,
            tp_base_pri: 0,
            tp_delta_pri: 0,
            dw_flags: 0,
        };
        let me = GetCurrentProcessId();
        if Thread32First(snap, &mut e) != 0 {
            loop {
                if e.th32_owner_process_id == me {
                    out.push(e.th32_thread_id);
                }
                e.dw_size = core::mem::size_of::<ThreadEntry32>() as u32;
                if Thread32Next(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

// -------------------------------------------------------------- frame ring

const RING: usize = 256;
static FRAME_RING: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static FRAME_POS: AtomicU32 = AtomicU32::new(0);
static LAST_FRAME_QPC: AtomicU64 = AtomicU64::new(0);
static GL_TID: AtomicU32 = AtomicU32::new(0);

/// Frame times in microseconds, sorted for the percentiles.
fn frame_percentiles() -> (f32, f32, f32, u32) {
    let mut v = Vec::with_capacity(RING);
    for slot in &FRAME_RING {
        let us = slot.load(Ordering::Relaxed);
        if us > 0 {
            v.push(us);
        }
    }
    if v.is_empty() {
        return (0.0, 0.0, 0.0, 0);
    }
    v.sort_unstable();
    let avg = v.iter().sum::<u64>() as f32 / v.len() as f32 / 1000.0;
    let p = |pct: usize| -> f32 {
        let i = (v.len().saturating_sub(1) * pct) / 100;
        v[i] as f32 / 1000.0
    };
    (avg, p(95), p(99), v.len() as u32)
}

// -------------------------------------------------------------- sampling

static SAMPLES: OnceLock<Mutex<BTreeMap<u32, u32>>> = OnceLock::new();
static LAST_SAMPLE_MS: AtomicU64 = AtomicU64::new(0);

fn samples() -> &'static Mutex<BTreeMap<u32, u32>> {
    SAMPLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn sample_gl_thread() {
    let tid = GL_TID.load(Ordering::Relaxed);
    if tid == 0 || tid == unsafe { GetCurrentThreadId() } {
        return;
    }
    unsafe {
        let h = OpenThread(0x0002 | 0x0008, 0, tid); // SUSPEND_RESUME | GET_CONTEXT
        if h == 0 {
            return;
        }
        if SuspendThread(h) == u32::MAX {
            CloseHandle(h);
            return;
        }
        let mut ctx = Context { buf: [0u8; 0x2cc] };
        ctx.buf[0..4].copy_from_slice(&0x0001_0001u32.to_le_bytes()); // CONTEXT_CONTROL (i386)
        if GetThreadContext(h, &mut ctx) != 0 {
            let eip =
                u32::from_le_bytes([ctx.buf[0xB8], ctx.buf[0xB9], ctx.buf[0xBA], ctx.buf[0xBB]]);
            let base = module_base();
            if eip >= base {
                let rva = eip - base;
                if let Ok(mut m) = samples().try_lock() {
                    *m.entry(rva).or_insert(0) += 1;
                }
            }
        }
        ResumeThread(h);
        CloseHandle(h);
    }
}

fn symbol_for(rva: u32) -> &'static str {
    match symbols::SYMBOLS.binary_search_by_key(&rva, |(a, _)| *a) {
        Ok(i) => symbols::SYMBOLS[i].1,
        Err(0) => "?",
        Err(i) => symbols::SYMBOLS[i - 1].1,
    }
}

// ----------------------------------------------------------- thread stats

struct ThreadCpu {
    last_ms: u64,
    proc: u64,
    threads: BTreeMap<u32, u64>,
}

static THREAD_CPU: OnceLock<Mutex<ThreadCpu>> = OnceLock::new();

fn thread_cpu_state() -> &'static Mutex<ThreadCpu> {
    THREAD_CPU.get_or_init(|| {
        Mutex::new(ThreadCpu {
            last_ms: 0,
            proc: 0,
            threads: BTreeMap::new(),
        })
    })
}

/// Returns (process %, thread count, top threads as "tid xxx%").
fn sample_threads() -> (f32, u32, Vec<(u32, f32)>) {
    let now = unsafe { GetTickCount64() };
    let proc = cpu_now();
    let ids = thread_ids();
    let mut current = BTreeMap::new();
    for tid in ids {
        if let Some(v) = thread_cpu(tid) {
            current.insert(tid, v);
        }
    }
    let Ok(mut st) = thread_cpu_state().lock() else {
        return (0.0, 0, Vec::new());
    };
    if st.last_ms == 0 {
        st.last_ms = now;
        st.proc = proc;
        st.threads = current;
        return (0.0, st.threads.len() as u32, Vec::new());
    }
    let dt_ms = now.saturating_sub(st.last_ms).max(1);
    let denom = dt_ms as f64 * 10_000.0; // 100ns units per ms
    let proc_pct = (proc.saturating_sub(st.proc) as f64 / denom * 100.0) as f32;
    let mut rows = Vec::new();
    for (tid, v) in &current {
        let prev = st.threads.get(tid).copied().unwrap_or(*v);
        let pct = (v.saturating_sub(prev) as f64 / denom * 100.0) as f32;
        if pct > 0.05 {
            rows.push((*tid, pct));
        }
    }
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    st.last_ms = now;
    st.proc = proc;
    st.threads = current;
    (proc_pct, rows.len() as u32, rows)
}

// -------------------------------------------------------------- snapshot

static HUD_LINES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static LAST_STATS: OnceLock<Mutex<CokeProfStats>> = OnceLock::new();
static HUD_VISIBLE: AtomicU32 = AtomicU32::new(1);
static CFG: OnceLock<Config> = OnceLock::new();

fn cfg() -> Config {
    *CFG.get().unwrap_or(&Config::default())
}

fn hud_lines() -> &'static Mutex<Vec<String>> {
    HUD_LINES.get_or_init(|| Mutex::new(Vec::new()))
}

fn csv_file() -> &'static Mutex<Option<std::fs::File>> {
    static CSV: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
    CSV.get_or_init(|| Mutex::new(None))
}

fn open_csv() {
    let Some(dir) = exe_dir() else { return };
    let path = dir.join("coke-prof.csv");
    let mut f = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => {
            unsafe { log(&format!("coke-prof: cannot open {}: {e}", path.display())) };
            return;
        }
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        let _ = writeln!(
            f,
            "ms,fps,frame_ms,p95_ms,p99_ms,cpu_pct,threads,gl_list,gl_list_ms,gl_tex,gl_tex_ms,gl_begin,gl_push,gl_draw,verts,gl_tex_up,swap_ms,top_func,top_pct"
        );
    }
    *csv_file().lock().unwrap() = Some(f);
}

fn write_csv(row: &str) {
    if let Ok(mut g) = csv_file().try_lock()
        && let Some(f) = g.as_mut()
    {
        let _ = writeln!(f, "{row}");
    }
}

/// One snapshot interval. Builds the HUD lines and appends one CSV row.
unsafe fn snapshot_interval() {
    let c = cfg();
    let (avg, p95, p99, _n) = frame_percentiles();
    let (cpu_pct, threads, rows) = sample_threads();

    let mut st = CokeProfStats::default();
    let have_gl = read_stats(&mut st);

    let mut d = CokeProfStats::default();
    {
        let last = LAST_STATS.get_or_init(|| Mutex::new(CokeProfStats::default()));
        if let Ok(mut prev) = last.lock() {
            d.call_list = st.call_list.saturating_sub(prev.call_list);
            d.call_list_ns = st.call_list_ns.saturating_sub(prev.call_list_ns);
            d.bind_texture = st.bind_texture.saturating_sub(prev.bind_texture);
            d.bind_texture_ns = st.bind_texture_ns.saturating_sub(prev.bind_texture_ns);
            d.begin_end = st.begin_end.saturating_sub(prev.begin_end);
            d.push_pop = st.push_pop.saturating_sub(prev.push_pop);
            d.draw_arrays = st.draw_arrays.saturating_sub(prev.draw_arrays);
            d.draw_elements = st.draw_elements.saturating_sub(prev.draw_elements);
            d.vertices = st.vertices.saturating_sub(prev.vertices);
            d.tex_upload = st.tex_upload.saturating_sub(prev.tex_upload);
            d.tex_upload_bytes = st.tex_upload_bytes.saturating_sub(prev.tex_upload_bytes);
            d.swap_ns = st.swap_ns.saturating_sub(prev.swap_ns);
            d.frames = st.frames.saturating_sub(prev.frames);
            *prev = st;
        }
    }

    let gl_list_ms = d.call_list_ns as f64 / 1_000_000.0;
    let gl_tex_ms = d.bind_texture_ns as f64 / 1_000_000.0;
    let frames = d.frames.max(1) as f64;
    let pf = |n: u64| n as f64 / frames;
    let swap_ms = if d.frames > 0 {
        (d.swap_ns as f64 / d.frames as f64) / 1_000_000.0
    } else {
        st.last_swap_ns as f64 / 1_000_000.0
    };

    // Top sampled functions. Percent is relative to the samples in this
    // interval, not to wall time, so it answers "where was the GL thread?".
    let mut top: Vec<(u32, u32)> = Vec::new();
    if c.sample
        && let Ok(mut m) = samples().try_lock()
    {
        top = m.iter().map(|(k, v)| (*k, *v)).collect();
        m.clear();
    }
    top.sort_by_key(|a| std::cmp::Reverse(a.1));
    let total: u32 = top.iter().map(|(_, v)| *v).sum::<u32>().max(1);
    top.truncate(c.top);
    let top_str = top
        .iter()
        .map(|(rva, n)| {
            format!(
                "{} {:.0}%",
                symbol_for(*rva),
                *n as f64 * 100.0 / total as f64
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");

    let mut lines = Vec::new();
    lines.push(format!(
        "PROF {:.1} fps  frame {:.1} / p95 {:.1} / p99 {:.1} ms  cpu {:.0}%  threads {}",
        fps(),
        avg,
        p95,
        p99,
        cpu_pct,
        threads
    ));
    if have_gl {
        lines.push(format!(
            "GL  list {:.0}/f {:.1}ms | tex {:.0}/f {:.1}ms | begin {:.0}/f | push {:.0}/f | draw {:.0}/f | verts {:.0}/f",
            pf(d.call_list),
            gl_list_ms / frames,
            pf(d.bind_texture),
            gl_tex_ms / frames,
            pf(d.begin_end),
            pf(d.push_pop),
            pf(d.draw_arrays + d.draw_elements),
            pf(d.vertices)
        ));
        lines.push(format!(
            "GL  upload {} ({:.1} MB) | read {} | swap {:.1} ms{}",
            d.tex_upload,
            d.tex_upload_bytes as f64 / 1_048_576.0,
            d.read_pixels,
            swap_ms,
            if c.timing { " | timed" } else { "" }
        ));
    } else {
        lines.push("GL  counters unavailable (proxy has no coke_prof_* exports)".to_string());
    }
    if !rows.is_empty() {
        let threads = rows
            .iter()
            .take(3)
            .map(|(tid, pct)| format!("{tid}:{pct:.0}%"))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!("CPU {threads}"));
    }
    if !top_str.is_empty() {
        lines.push(format!("HOT {top_str}"));
    }

    if let Ok(mut g) = hud_lines().lock() {
        *g = lines;
    }

    if c.log_csv {
        write_csv(&format!(
            "{},{:.1},{:.2},{:.2},{:.2},{:.1},{},{},{:.2},{},{:.2},{},{},{},{},{},{:.2},{},{}",
            unsafe { GetTickCount64() },
            fps(),
            avg,
            p95,
            p99,
            cpu_pct,
            threads,
            d.call_list,
            gl_list_ms,
            d.bind_texture,
            gl_tex_ms,
            d.begin_end,
            d.push_pop,
            d.draw_arrays + d.draw_elements,
            d.vertices,
            d.tex_upload,
            swap_ms,
            top.first().map(|(rva, _)| symbol_for(*rva)).unwrap_or("-"),
            top.first()
                .map(|(_, n)| *n as f64 * 100.0 / total as f64)
                .unwrap_or(0.0)
        ));
    }
}

// ---------------------------------------------------------------- hotkeys
//
// F7/F8 are registered with the host's F1-F12 bus (API v4). The host handles
// polling, focus checks and rising-edge de-duplication on the watcher thread;
// these callbacks only touch atomics and provably short-lived state.

unsafe extern "system" fn on_reset() {
    unsafe {
        reset_profiler();
        if let Ok(mut m) = samples().try_lock() {
            m.clear();
        }
        for slot in &FRAME_RING {
            slot.store(0, Ordering::Relaxed);
        }
        FRAME_POS.store(0, Ordering::Relaxed);
        if let Some(last) = LAST_STATS.get()
            && let Ok(mut prev) = last.lock()
        {
            *prev = CokeProfStats::default();
        }
        log("coke-prof: counters reset (F7)");
    }
}

unsafe extern "system" fn on_toggle_hud() {
    let v = HUD_VISIBLE.fetch_xor(1, Ordering::Relaxed) ^ 1;
    unsafe {
        log(&format!(
            "coke-prof: HUD {} (F8)",
            if v != 0 { "on" } else { "off" }
        ));
    }
}

// ---------------------------------------------------------------- exports

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_init(host: *const CokeHost) -> i32 {
    unsafe {
        if host.is_null() {
            return 0;
        }
        let h = &*host;
        if h.api_version < 2 || (h.size as usize) < core::mem::size_of::<CokeHost>() {
            return 0;
        }
        if HOST.set(*h).is_err() {
            return 1;
        }
        let c = load_config();
        let _ = CFG.set(c);
        if !c.enabled {
            log("coke-prof: disabled (enabled=0)");
            return 0;
        }
        // Host API v3 calls `coke_hud` after all world-space overlays.
        HUD_MODE.store(u32::from(h.api_version >= 3), Ordering::Relaxed);
        HUD_VISIBLE.store(u32::from(c.hud), Ordering::Relaxed);
        resolve_profiler();
        if c.log_csv {
            open_csv();
        }
        if c.timing {
            set_timing(true);
        }
        log(&format!(
            "coke-prof: api={} size={} sample={}Hz timing={} hud={} csv={}",
            h.api_version, h.size, c.sample_hz, c.timing, c.hud, c.log_csv
        ));
        if PROF_READ.load(Ordering::Relaxed) == 0 {
            log("coke-prof: proxy profiler exports not found; frame/thread data only");
        }
        // API v4: HUD on top of other mod HUDs, and F7/F8 via the host bus.
        if let Some(set_layer) = h.set_layer {
            set_layer(10);
        }
        if let Some(register) = h.register_hotkey {
            if register(0x76, Some(on_reset)) == 0 {
                log("coke-prof: F7 hotkey rejected");
            }
            if register(0x77, Some(on_toggle_hud)) == 0 {
                log("coke-prof: F8 hotkey rejected");
            }
        } else {
            log("coke-prof: no hotkey bus; F7/F8 unavailable");
        }
        log("coke-prof: F7 reset, F8 HUD. Log: coke-prof.csv");
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_frame(_w: i32, _h: i32, _fps: f32) {
    unsafe {
        GL_TID.store(GetCurrentThreadId(), Ordering::Relaxed);
        let now = qpc();
        let last = LAST_FRAME_QPC.swap(now, Ordering::Relaxed);
        if last != 0 {
            let freq = qpc_freq();
            let wall_us = (now.saturating_sub(last)).saturating_mul(1_000_000) / freq;
            // Prefer the proxy's CPU-side frame time: it stops at swap entry, so it
            // does not include the previous present/VSync wait. Older proxies have
            // no such export, so fall back to the wall-clock interval.
            let cpu_ns = last_cpu_frame_ns();
            let us = if cpu_ns > 0 { cpu_ns / 1_000 } else { wall_us };
            let pos = FRAME_POS.fetch_add(1, Ordering::Relaxed) as usize & (RING - 1);
            FRAME_RING[pos].store(us.max(1), Ordering::Relaxed);
        }
        // Old hosts do not call `coke_hud`.
        if HUD_MODE.load(Ordering::Relaxed) == 0 {
            draw_hud();
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_hud(_w: i32, _h: i32, _fps: f32) {
    unsafe {
        if HUD_MODE.load(Ordering::Relaxed) != 0 {
            draw_hud();
        }
    }
}

unsafe fn draw_hud() {
    unsafe {
        if HUD_VISIBLE.load(Ordering::Relaxed) == 0 {
            return;
        }
        let Ok(g) = hud_lines().try_lock() else {
            return;
        };
        let mut y = 60;
        for line in g.iter() {
            draw_text(8, y, line, 0x00D8_F0FF);
            y += 18;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_tick() {
    unsafe {
        let c = cfg();

        if c.sample {
            let now = GetTickCount64();
            let period = 1000 / c.sample_hz.max(1) as u64;
            if now.saturating_sub(LAST_SAMPLE_MS.load(Ordering::Relaxed)) >= period {
                LAST_SAMPLE_MS.store(now, Ordering::Relaxed);
                sample_gl_thread();
            }
        }

        static LAST_SNAP: AtomicU64 = AtomicU64::new(0);
        let now = GetTickCount64();
        if now.saturating_sub(LAST_SNAP.load(Ordering::Relaxed)) >= c.interval_ms {
            LAST_SNAP.store(now, Ordering::Relaxed);
            snapshot_interval();
        }
    }
}
