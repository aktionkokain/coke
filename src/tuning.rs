//! Process priority, CPU affinity and worker thread priority.

use crate::*;

// ---- code ----
// ---------------------------------------------------------- worker priorities

pub unsafe fn boost_workers() {
    unsafe {
        let pid = GetCurrentProcessId();
        let me = GetCurrentThreadId();
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == (-1isize) as Handle {
            return;
        }
        let mut entry: ThreadEntry32 = core::mem::zeroed();
        entry.dw_size = core::mem::size_of::<ThreadEntry32>() as u32;
        let mut changed = 0u32;
        let mut seen = 0u32;
        if Thread32First(snap, &mut entry) != 0 {
            loop {
                if entry.th32_owner_process_id == pid && entry.th32_thread_id != me {
                    seen += 1;
                    let thread = OpenThread(
                        THREAD_QUERY_INFORMATION | THREAD_SET_INFORMATION,
                        0,
                        entry.th32_thread_id,
                    );
                    if !thread.is_null() {
                        let pri = GetThreadPriority(thread);
                        if pri == THREAD_PRIORITY_IDLE {
                            SetThreadPriority(thread, THREAD_PRIORITY_BELOW_NORMAL);
                            changed += 1;
                        }
                        CloseHandle(thread);
                    }
                }
                entry.dw_size = core::mem::size_of::<ThreadEntry32>() as u32;
                if Thread32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        log(&format!(
            "boost_workers: {seen} threads seen, {changed} moved from IDLE to BELOW_NORMAL"
        ));
    }
}

pub unsafe fn apply_process_tuning(cfg: &Config) {
    unsafe {
        if cfg.priority != 255 {
            let class = PRIORITY_CLASS
                .iter()
                .find(|(i, _)| *i == cfg.priority as i32)
                .map(|(_, c)| *c)
                .unwrap_or(0x0000_8000);
            let ok = SetPriorityClass(GetCurrentProcess(), class);
            log(&format!("priority class 0x{class:08x} -> {ok}"));
        }
        if cfg.affinity != 0 {
            let ok = SetProcessAffinityMask(GetCurrentProcess(), cfg.affinity);
            log(&format!("affinity mask 0x{:x} -> {ok}", cfg.affinity));
        }
    }
}
