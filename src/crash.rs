//! Vectored exception handler that logs the first fatal fault with a stack walk.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::ffi::c_void;

// ---- code ----
// --------------------------------------------------------------------- init

pub static INSTANCE_BASE: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn readable_u32(addr: usize) -> Option<u32> {
    unsafe {
        if addr == 0 {
            return None;
        }
        let mut m: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(
            addr as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        ) == 0
        {
            return None;
        }
        if m.state != 0x1000 {
            return None;
        }
        let p = m.protect & 0xff;
        if p == 0x00 || p == 0x01 || (m.protect & 0x100) != 0 {
            return None;
        }
        Some(core::ptr::read_unaligned(addr as *const u32))
    }
}

/// First-chance crash logger. It records where the process died and the first
/// few stack words so a fault inside the packed engine can be traced back to
/// the engine (or proxy) call that reached it.
pub unsafe extern "system" fn crash_handler(info: *mut ExceptionPointers) -> i32 {
    unsafe {
        if info.is_null() {
            return 0;
        }
        let rec = (*info).record;
        if rec.is_null() {
            return 0;
        }
        let code = (*rec).code;
        // Everything that normally kills a packed 32-bit engine, not just plain
        // access violations: stack overflow, heap corruption, fail-fast/abort and
        // fatal callbacks all used to disappear without a trace.
        let fatal = matches!(
            code,
            0xC000_0005 // access violation
            | 0xC000_001D // illegal instruction
            | 0xC000_0094 // integer divide by zero
            | 0xC000_0006 // in-page error
            | 0xC000_00FD // stack overflow
            | 0xC000_0374 // heap corruption
            | 0xC000_0409 // fail fast (abort / __fastfail)
            | 0xC000_041D // fatal user-mode callback
            | 0xC000_0420 // assertion failure
            | 0x8000_0003 // breakpoint / int3
        );
        if !fatal {
            if code == 0xE06D_7363 {
                // C++ throws are normal traffic; report the first one only so a
                // quiet log still shows that the engine got into its C++ startup.
                static CPP_LOGGED: AtomicU32 = AtomicU32::new(0);
                if CPP_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                    log("exception: first C++ throw (0xe06d7363), continuing");
                }
            }
            return 0;
        }
        // Log EVERY fatal-class exception (throttled), not just the first: the
        // engine handles some first-chance faults internally with SEH, and a
        // once-ever flag burned on such a fault left the real crash unlogged.
        static FATAL_N: AtomicU32 = AtomicU32::new(0);
        let n = FATAL_N.fetch_add(1, Ordering::Relaxed);
        if n >= 12 {
            return 0;
        }
        let addr = (*rec).address as usize;
        let ctx = (*info).context as *const u8;
        let (eip, esp, ebp) = if ctx.is_null() {
            (0, 0, 0)
        } else {
            (
                core::ptr::read_unaligned(ctx.add(184) as *const u32) as usize,
                core::ptr::read_unaligned(ctx.add(196) as *const u32) as usize,
                core::ptr::read_unaligned(ctx.add(180) as *const u32) as usize,
            )
        };
        let game = GetModuleHandleW(core::ptr::null()) as usize;
        let me = INSTANCE_BASE.load(Ordering::Relaxed);
        let off = addr.wrapping_sub(game);

        let mut m: MemBasicInfo = core::mem::zeroed();
        VirtualQuery(
            addr as *const c_void,
            &mut m,
            core::mem::size_of::<MemBasicInfo>(),
        );
        log(&format!(
            "CRASH[{n}] code=0x{code:08x} addr=0x{addr:08x} game=0x{game:08x} game_off=0x{off:x} \
         eip=0x{eip:08x} esp=0x{esp:08x} ebp=0x{ebp:08x} me=0x{me:08x} \
         region_base=0x{:x} region_size=0x{:x} state=0x{:x} protect=0x{:x} type=0x{:x}",
            m.base_address as usize, m.region_size, m.state, m.protect, m.ty
        ));

        // The stack walk is only worth its cost for the first couple of faults;
        // later ones still get the summary line above.
        if n >= 2 {
            return 0;
        }

        let mut line = String::from("CRASH stack:");
        for i in 0..40 {
            let a = esp.wrapping_add(i * 4);
            let Some(v) = readable_u32(a) else { break };
            let v = v as usize;
            let tag = if v >= game && v < game.wrapping_add(0x0200_0000) {
                " game"
            } else if v >= me && v < me.wrapping_add(0x0040_0000) {
                " mod"
            } else {
                ""
            };
            line.push_str(&format!(" [{i}]{v:08x}{tag}"));
            if i % 5 == 4 {
                log(&line);
                line = String::from("CRASH stack:");
            }
        }
        if line.len() > 12 {
            log(&line);
        }

        let mut bp = ebp;
        for depth in 0..8 {
            let Some(next) = readable_u32(bp) else { break };
            let next = next as usize;
            let Some(ret) = readable_u32(bp.wrapping_add(4)) else {
                break;
            };
            let ret = ret as usize;
            let tag = if ret >= game && ret < game.wrapping_add(0x0200_0000) {
                format!("game+0x{:x}", ret - game)
            } else if ret >= me && ret < me.wrapping_add(0x0040_0000) {
                format!("mod+0x{:x}", ret - me)
            } else {
                format!("0x{ret:08x}")
            };
            log(&format!("CRASH frame[{depth}] ebp=0x{bp:08x} ret={tag}"));
            if next <= bp {
                break;
            }
            bp = next;
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }
}
