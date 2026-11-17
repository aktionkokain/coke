//! WH_CBT create-window hook: gives the engine its final window size at creation.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::ffi::c_void;

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

pub const MAX_HOOKED: usize = 32;
pub static HOOKED_TIDS: [AtomicU32; MAX_HOOKED] = [const { AtomicU32::new(0) }; MAX_HOOKED];
pub static HOOKED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The engine registers its window class as "CM Black Sea", either as an ANSI
/// or as a UTF-16 string.
pub fn class_is_cm_name(p: *const c_void) -> bool {
    if p.is_null() || (p as usize) < 0x1_0000 {
        return false; // class atom, not a string
    }
    const NAME: &[u8] = b"CM Black Sea";
    unsafe {
        let a = core::slice::from_raw_parts(p as *const u8, NAME.len());
        if a == NAME {
            return true;
        }
        let w = core::slice::from_raw_parts(p as *const u16, NAME.len());
        w.iter().zip(NAME.iter()).all(|(c, b)| *c == *b as u16)
    }
}

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
                    && class_is_cm_name((*cs).class)
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
        let n = HOOKED_COUNT.load(Ordering::Relaxed);
        for i in 0..n {
            if HOOKED_TIDS[i].load(Ordering::Relaxed) == tid {
                return;
            }
        }
        if n >= MAX_HOOKED {
            return;
        }
        let h = SetWindowsHookExW(
            WH_CBT,
            cbt_proc,
            INSTANCE_BASE.load(Ordering::Relaxed) as Hmodule,
            tid,
        );
        if !h.is_null() {
            HOOKED_TIDS[n].store(tid, Ordering::Relaxed);
            HOOKED_COUNT.store(n + 1, Ordering::Relaxed);
        } else {
            static FAIL_LOGGED: AtomicU32 = AtomicU32::new(0);
            if FAIL_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                log(&format!("cbt: SetWindowsHookEx failed for thread {tid}"));
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
