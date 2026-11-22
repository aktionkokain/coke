//! Compact first-chance exception records for in-process diagnostics.

use crate::*;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;

pub static INSTANCE_BASE: AtomicUsize = AtomicUsize::new(0);
static FAULT_LOG: OnceLock<std::fs::File> = OnceLock::new();

pub(crate) unsafe fn install_exception_logger() -> Handle {
    let log_path = exe_dir().join(LOG_FILE);
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = FAULT_LOG.set(file);
        unsafe { AddVectoredExceptionHandler(1, crash_handler) }
    } else {
        core::ptr::null_mut()
    }
}

struct Record {
    buf: [u8; 192],
    len: usize,
}

impl Write for Record {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let end = self.len + value.len();
        let span = self.buf.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        span.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

unsafe extern "system" fn crash_handler(info: *mut ExceptionPointers) -> i32 {
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    let Some(file) = FAULT_LOG.get() else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let Some(info) = (unsafe { info.as_ref() }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let Some(record) = (unsafe { info.record.as_ref() }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    let code = record.code;
    if !matches!(
        code,
        0xc000_0005
            | 0xc000_001d
            | 0xc000_0094
            | 0xc000_0006
            | 0xc000_00fd
            | 0xc000_0374
            | 0xc000_0409
            | 0xc000_041d
            | 0xc000_0420
            | 0x8000_0003
    ) {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    static RECORD_COUNT: AtomicU32 = AtomicU32::new(0);
    let index = RECORD_COUNT.fetch_add(1, Ordering::Relaxed);
    if index < 12 {
        let mut line = Record {
            buf: [0; 192],
            len: 0,
        };
        let address = record.address as usize;
        let me = INSTANCE_BASE.load(Ordering::Relaxed);
        let _ = writeln!(
            line,
            "exception: first-chance [{index}] code=0x{code:08x} addr=0x{address:08x} proxy=0x{me:08x}"
        );
        let mut written = 0;
        unsafe {
            WriteFile(
                file.as_raw_handle(),
                line.buf.as_ptr().cast(),
                line.len as u32,
                &mut written,
                core::ptr::null_mut(),
            );
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}
