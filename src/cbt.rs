//! WH_CBT create-window hook: gives the engine its final window size at creation.

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::c_void;
use std::sync::Mutex;

// ---- code ----
// ------------------------------------------------------------------ CBT hook

pub const WH_CBT: i32 = 5;
pub const HCBT_CREATEWND: i32 = 3;

/// CREATESTRUCT (32-bit layout).
#[repr(C)]
pub struct CreateStruct {
    pub create_params: *mut c_void,
    pub instance: Hmodule,
    pub menu: Handle,
    pub parent: Hwnd,
    pub cy: i32,
    pub cx: i32,
    pub y: i32,
    pub x: i32,
    pub style: u32,
    pub name: *const c_void,
    pub class: *const c_void,
    pub ex_style: u32,
}

#[repr(C)]
pub struct CbtCreateWnd {
    pub cs: *mut CreateStruct,
    pub insert_after: Hwnd,
}

const MAX_HOOKED: usize = 32;

struct ThreadHook {
    id: u32,
    thread: usize,
    hook: usize,
}

static THREAD_HOOKS: Mutex<Vec<ThreadHook>> = Mutex::new(Vec::new());

/// Framed window rectangle (outer size) used for the initial mode.
pub unsafe fn framed_window_geometry() -> (i32, i32, i32, i32) {
    unsafe {
        let (sx, sy, sw, sh) = (
            FRAMED_LEFT.load(Ordering::Relaxed),
            FRAMED_TOP.load(Ordering::Relaxed),
            FRAMED_W.load(Ordering::Relaxed),
            FRAMED_H.load(Ordering::Relaxed),
        );
        if sw > 0 && sh > 0 {
            return (sx, sy, sw, sh);
        }
        // No window exists yet, so this resolves to the primary monitor.
        framed_geometry(core::ptr::null_mut())
    }
}

/// Create-time hook: the engine builds its window at full monitor size and we
/// used to shrink it afterwards, which showed a fullscreen frame and left the
/// intro's DirectShow graph sized for the wrong client area. Rewriting the
/// CREATESTRUCT gives the engine the window it should have had from the start.
pub unsafe extern "system" fn cbt_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    unsafe {
        if code == HCBT_CREATEWND && lparam != 0 {
            let cbt = lparam as *const CbtCreateWnd;
            if !cbt.is_null() {
                let cs = (*cbt).cs;
                if !cs.is_null()
                    && (*cs).parent.is_null()
                    && (*cs).style & WS_POPUP != 0
                    && (*cs).cx >= 800
                    && (*cs).cy >= 600
                    && owns_window(wparam as Hwnd)
                    && class_is_cm(wparam as Hwnd)
                {
                    let (x, y, w, h) = framed_window_geometry();
                    let (old_style, old_cx, old_cy) = ((*cs).style, (*cs).cx, (*cs).cy);
                    (*cs).style = FRAMED_STYLE;
                    (*cs).x = x;
                    (*cs).y = y;
                    (*cs).cx = w;
                    (*cs).cy = h;
                    static LOGGED: AtomicU32 = AtomicU32::new(0);
                    if LOGGED.swap(1, Ordering::Relaxed) == 0 {
                        log(&format!(
                            "cbt: CM window created {w}x{h} at ({x},{y}) \
                         instead of {old_cx}x{old_cy} (style 0x{old_style:08x} -> 0x{FRAMED_STYLE:08x})"
                        ));
                    }
                }
            }
        }
        CallNextHookEx(core::ptr::null_mut(), code, wparam, lparam)
    }
}

pub unsafe fn hook_thread(tid: u32) {
    unsafe {
        let Ok(mut hooks) = THREAD_HOOKS.lock() else {
            return;
        };
        hooks.retain(|entry| {
            if WaitForSingleObject(entry.thread as Handle, 0) == 0 {
                UnhookWindowsHookEx(entry.hook as Handle);
                CloseHandle(entry.thread as Handle);
                false
            } else {
                true
            }
        });
        if hooks.len() >= MAX_HOOKED || hooks.iter().any(|entry| entry.id == tid) {
            return;
        }
        let thread = OpenThread(0x0010_0000, 0, tid); // SYNCHRONIZE
        if thread.is_null() {
            return;
        }
        let hook = SetWindowsHookExW(
            WH_CBT,
            cbt_proc,
            INSTANCE_BASE.load(Ordering::Relaxed) as Hmodule,
            tid,
        );
        if !hook.is_null() {
            hooks.push(ThreadHook {
                id: tid,
                thread: thread as usize,
                hook: hook as usize,
            });
            if config().verbose {
                log(&format!("cbt: installed on thread {tid}"));
            }
        } else {
            let error = GetLastError();
            CloseHandle(thread);
            static FAIL_LOGGED: AtomicU32 = AtomicU32::new(0);
            if FAIL_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!(
                    "cbt: SetWindowsHookEx failed for thread {tid}: {error}"
                ));
            }
        }
    }
}

/// Install the create-time hook on every thread of this process.
pub unsafe fn install_cbt_hooks() {
    unsafe {
        let me = GetCurrentProcessId();
        let my_tid = GetCurrentThreadId();
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == (-1isize) as Handle {
            return;
        }
        let mut entry: ThreadEntry32 = core::mem::zeroed();
        entry.dw_size = core::mem::size_of::<ThreadEntry32>() as u32;
        if Thread32First(snap, &mut entry) != 0 {
            loop {
                if entry.th32_owner_process_id == me && entry.th32_thread_id != my_tid {
                    hook_thread(entry.th32_thread_id);
                }
                entry.dw_size = core::mem::size_of::<ThreadEntry32>() as u32;
                if Thread32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
}
