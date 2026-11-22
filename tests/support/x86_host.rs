//! Standalone x86 process for exercising the DLL through the Windows loader.
use std::ffi::{c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(path: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const c_char) -> usize;
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn SetErrorMode(mode: u32) -> u32;
}

#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: unsafe extern "system" fn(usize, u32, usize, isize) -> isize,
    extra_class: i32,
    extra_window: i32,
    instance: usize,
    icon: usize,
    cursor: usize,
    background: usize,
    menu: *const u16,
    name: *const u16,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn DefWindowProcW(window: usize, message: u32, wparam: usize, lparam: isize) -> isize;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: usize,
        menu: usize,
        instance: usize,
        parameter: *mut c_void,
    ) -> usize;
    fn GetPropA(window: usize, name: *const c_char) -> usize;
    fn DestroyWindow(window: usize) -> i32;
}

fn test_window(name: &str, size: i32) -> usize {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WindowClass {
            style: 0,
            procedure: DefWindowProcW,
            extra_class: 0,
            extra_window: 0,
            instance,
            icon: 0,
            cursor: 0,
            background: 0,
            menu: std::ptr::null(),
            name: wide.as_ptr(),
        };
        assert_ne!(RegisterClassW(&class), 0, "fixture class registration");
        let window = CreateWindowExW(
            0x0800_0080,
            wide.as_ptr(),
            wide.as_ptr(),
            0x9000_0000,
            -4000,
            -4000,
            size,
            size,
            0,
            0,
            instance,
            std::ptr::null_mut(),
        );
        assert_ne!(window, 0, "fixture window creation");
        window
    }
}

fn wait_for_adoption(window: usize) {
    let started = std::time::Instant::now();
    while unsafe { GetPropA(window, c"Coke.MainWindowGeneration".as_ptr()) } == 0 {
        assert!(started.elapsed().as_secs() < 10, "window was not adopted");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn verify_windows(proxy: &Path) {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let mut foreign = Command::new(std::env::current_exe().unwrap())
        .arg(proxy)
        .arg("foreign-window")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(foreign.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    let foreign_window: usize = ready.trim().parse().expect("foreign window handle");

    let splash = test_window("Coke test splash", 400);
    call_wgl(load(proxy));
    wait_for_adoption(splash);
    let gameplay = test_window("CM Black Sea", 500);
    wait_for_adoption(gameplay);
    unsafe {
        let property = c"Coke.MainWindowGeneration".as_ptr();
        assert_eq!(GetPropA(splash, property), 0);
        assert_eq!(GetPropA(foreign_window, property), 0);
        DestroyWindow(gameplay);
        DestroyWindow(splash);
    }
    drop(foreign.stdin.take());
    assert!(foreign.wait().unwrap().success());
}

fn load(path: &Path) -> usize {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    assert_ne!(module, 0, "could not load {}", path.display());
    module
}

fn proc(module: usize, name: &[u8]) -> usize {
    let pointer = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
    assert_ne!(
        pointer,
        0,
        "missing export {:?}",
        String::from_utf8_lossy(name)
    );
    pointer
}

fn call_gl(module: usize) {
    let get_error: unsafe extern "system" fn() -> u32 =
        unsafe { std::mem::transmute(proc(module, b"glGetError\0")) };
    let get_string: unsafe extern "system" fn(u32) -> *const c_void =
        unsafe { std::mem::transmute(proc(module, b"glGetString\0")) };
    // These exports are valid without a context and exercise both argument
    // cleanup and the return register across the naked slow path.
    assert!(matches!(unsafe { get_error() }, 0 | 0x0502));
    assert!(unsafe { get_string(0x1f02) }.is_null());
}

fn call_wgl(module: usize) {
    let context: unsafe extern "system" fn() -> *const c_void =
        unsafe { std::mem::transmute(proc(module, b"wglGetCurrentContext\0")) };
    assert!(unsafe { context() }.is_null());
}

fn call_forwarded_wgl(module: usize) {
    let create: unsafe extern "system" fn(usize, i32) -> usize =
        unsafe { std::mem::transmute(proc(module, b"wglCreateLayerContext\0")) };
    // Invalid DC is a defined WGL failure and exercises a forwarded function
    // on threads that were created before the system DLL was loaded.
    assert_eq!(unsafe { create(0, 0) }, 0);
}

fn verify_safe_mode(proxy: &Path) {
    static MATCHING_WORD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    use std::sync::atomic::Ordering;
    let user32 = load(Path::new("user32.dll"));
    let real = proc(user32, b"GetSystemMetrics\0");
    MATCHING_WORD.store(real, Ordering::SeqCst);
    let log = std::env::current_exe().unwrap().with_file_name("coke.log");
    let old_len = std::fs::metadata(&log).map_or(0, |info| info.len()) as usize;
    call_wgl(load(proxy));
    let start = std::time::Instant::now();
    loop {
        if std::fs::read_to_string(&log)
            .ok()
            .and_then(|text| text.get(old_len..).map(str::to_owned))
            .is_some_and(|text| text.contains("[init] done"))
        {
            break;
        }
        assert!(start.elapsed().as_secs() < 10, "bootstrap did not complete");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        MATCHING_WORD.load(Ordering::SeqCst),
        real,
        "safe mode changed an engine image word"
    );
}

fn verify_ordinals(module: usize) {
    let original_name: Vec<u16> = "opengl32_real.dll".encode_utf16().chain([0]).collect();
    let system = unsafe { GetModuleHandleW(original_name.as_ptr()) };
    assert_ne!(system, 0);
    for line in include_str!("../../exports.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else { continue };
        let ordinal: usize = fields.next().unwrap().parse().unwrap();
        let name = std::ffi::CString::new(name).unwrap();
        let by_name = unsafe { GetProcAddress(module, name.as_ptr()) };
        let by_ordinal = unsafe { GetProcAddress(module, ordinal as *const c_char) };
        assert_ne!(by_name, 0, "{name:?} is missing");
        assert_eq!(by_name, by_ordinal, "wrong proxy ordinal for {name:?}");
        let system_name = unsafe { GetProcAddress(system, name.as_ptr()) };
        assert_ne!(system_name, 0);
        assert_eq!(
            system_name,
            unsafe { GetProcAddress(system, ordinal as *const c_char) },
            "system ordinal differs for {name:?}"
        );
    }
}

fn main() {
    unsafe { SetErrorMode(0x8003) };
    let mut args = std::env::args_os().skip(1);
    let proxy = args.next().expect("path to built proxy");
    match args.next().as_deref().and_then(|mode| mode.to_str()) {
        Some("gl") => call_gl(load(Path::new(&proxy))),
        Some("wgl") => call_wgl(load(Path::new(&proxy))),
        Some("concurrent") => {
            let module = load(Path::new(&proxy));
            let barrier = Arc::new(Barrier::new(16));
            std::thread::scope(|scope| {
                for i in 0..16 {
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        if i % 2 == 0 {
                            call_forwarded_wgl(module);
                        } else {
                            call_wgl(module);
                        }
                    });
                }
            });
            verify_ordinals(module);
        }
        Some("load-only") => {
            let module = load(Path::new(&proxy));
            let log = std::env::current_exe().unwrap().with_file_name("coke.log");
            let start = std::time::Instant::now();
            loop {
                if let Ok(text) = std::fs::read_to_string(&log)
                    && text.contains("coke: loaded coke_prof.dll")
                {
                    assert!(!text.contains("proxy profiler exports not found"));
                    let version: unsafe extern "system" fn() -> u32 =
                        unsafe { std::mem::transmute(proc(module, b"coke_prof_version\0")) };
                    let read: unsafe extern "system" fn(*mut u64) -> i32 =
                        unsafe { std::mem::transmute(proc(module, b"coke_prof_read\0")) };
                    let mut snapshot = [0u64; 28];
                    snapshot[27] = u64::MAX;
                    assert_eq!(unsafe { version() }, 2);
                    assert_eq!(unsafe { read(snapshot.as_mut_ptr()) }, 1);
                    assert_eq!(snapshot[0], (216u64 << 32) | 2);
                    assert_eq!(snapshot[27], u64::MAX);
                    break;
                }
                assert!(start.elapsed().as_secs() < 10, "profiler did not load");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        Some("safe") => verify_safe_mode(Path::new(&proxy)),
        Some("windows") => verify_windows(Path::new(&proxy)),
        Some("foreign-window") => {
            let window = test_window("CM Black Sea", 700);
            println!("{window}");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0]);
            unsafe { DestroyWindow(window) };
        }
        Some("fail") => {
            call_gl(load(Path::new(&proxy)));
            std::process::exit(97);
        }
        None => {
            let launch = |mode, no_cache| {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command.arg(&proxy).arg(mode);
                if mode != "safe" {
                    command.env("COKE_SAFE_START", "1");
                } else {
                    command.env_remove("COKE_SAFE_START");
                }
                if no_cache {
                    command.env_remove("LOCALAPPDATA");
                }
                command.status().unwrap()
            };
            let config = std::env::current_exe()
                .unwrap()
                .with_file_name("coke_config.ini");
            std::fs::write(
                config,
                "profile=none\nhook_slots=1\nprofile_dump=1\ncheck_updates=off\nhook_window=1\nhook_wndproc=0\nhook_mode=0\nhook_cbt=0\n",
            )
            .unwrap();
            for mode in ["gl", "wgl", "concurrent", "safe", "windows"] {
                let status = launch(mode, false);
                assert!(status.success(), "{mode} first use failed: {status}");
            }
            let result = launch("fail", true);
            assert!(
                !result.success() && result.code() != Some(97),
                "expected forwarding failure"
            );
            let log = std::fs::read_to_string(
                std::env::current_exe().unwrap().with_file_name("coke.log"),
            )
            .unwrap();
            assert!(log.contains("OpenGL forwarding: cannot prepare the system DLL"));
        }
        Some(mode) => panic!("unknown mode {mode}"),
    }
}
