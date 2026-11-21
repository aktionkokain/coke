//! Patching the packed engine's in-memory user32 import slots.

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::c_void;

// ---- code ----
// ------------------------------------------------------------- slot hooking

pub unsafe fn write_slot(slot: usize, value: usize) {
    unsafe {
        let mut old = 0u32;
        VirtualProtect(slot as *mut c_void, 4, PAGE_EXECUTE_READWRITE, &mut old);
        core::ptr::write_unaligned(slot as *mut usize, value);
        VirtualProtect(slot as *mut c_void, 4, old, &mut old);
    }
}

/// Rewrites the slot only if it does not already point at our hook, so the
/// steady-state poll costs three reads and no protection changes.
pub unsafe fn ensure_slot(slot: usize, hook: usize) {
    unsafe {
        let current = core::ptr::read_unaligned(slot as *const usize);
        if current != hook {
            write_slot(slot, hook);
        }
    }
}

/// The packed CMBS image has a fixed base, so its slots are known from the
/// profile. Other CM titles use a different build; for them the generic scanner
/// below finds the same user32 pointers in the module image after the packer has
/// built the IAT.
pub unsafe fn install_slot_hooks() {
    unsafe {
        let cfg = config().clone();
        static SLOTS_LOGGED: AtomicU32 = AtomicU32::new(0);
        if profile_ok() {
            if SLOTS_LOGGED.swap(1, Ordering::Relaxed) == 0 {
                let read = |slot: Option<usize>| match slot {
                    Some(s) => format!("0x{:x}", core::ptr::read_unaligned(s as *const usize)),
                    None => "unavailable".to_string(),
                };
                log(&format!(
                    "slots: CloseWindow={} GetSystemMetrics={} ChangeDisplaySettingsA={} GetCursorPos={}",
                    read(slot_close_window()),
                    read(slot_get_system_metrics()),
                    read(slot_change_display()),
                    read(slot_get_cursor_pos())
                ));
            }
            let mask = SLOT_OK_MASK.load(Ordering::Relaxed);
            let install = |bit: u32, slot: Option<usize>, real: usize, hook: usize| {
                if bit == 0 || mask & bit == 0 || real == 0 {
                    return;
                }
                if let Some(slot) = slot {
                    ensure_slot(slot, hook);
                }
            };
            install(
                if cfg.fix_alt_tab {
                    SLOT_BIT_CLOSE_WINDOW
                } else {
                    0
                },
                slot_close_window(),
                REAL_CLOSE_WINDOW.load(Ordering::Relaxed),
                hook_close_window as *const () as usize,
            );
            install(
                SLOT_BIT_GET_SYSTEM_METRICS,
                slot_get_system_metrics(),
                REAL_METRICS.load(Ordering::Relaxed),
                hook_metrics as *const () as usize,
            );
            install(
                SLOT_BIT_CHANGE_DISPLAY,
                slot_change_display(),
                REAL_CHANGE_DISPLAY.load(Ordering::Relaxed),
                hook_change_display as *const () as usize,
            );
            install(
                SLOT_BIT_GET_CURSOR_POS,
                slot_get_cursor_pos(),
                REAL_GET_CURSOR_POS.load(Ordering::Relaxed),
                hook_get_cursor_pos as *const () as usize,
            );
        }

        // ShowWindow is not part of the profile's slot list. It is also the generic
        // fallback for other CM titles, so scan for it until it has been found.
        // The generic scanner is also the fallback for any profile slot that is
        // missing or failed its content check.
        let pending = slots_pending();
        if pending && SCAN_TICKS.load(Ordering::Relaxed) < 24 {
            let now = GetTickCount();
            let last = LAST_SCAN_MS.load(Ordering::Relaxed);
            // A full image scan is not free; a few passes over the startup window
            // are enough for a packed image to have built its import table.
            if last == 0 || now.wrapping_sub(last) >= 500 {
                LAST_SCAN_MS.store(now, Ordering::Relaxed);
                scan_slots();
            }
        }
    }
}

/// Size of the main module image, read from its in-memory PE header.
pub unsafe fn module_image_size(base: usize) -> usize {
    unsafe {
        if base == 0 {
            return 0;
        }
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        let opt = base + e_lfanew + 24;
        core::ptr::read_unaligned((opt + 56) as *const u32) as usize
    }
}

/// `TimeDateStamp` of the main module's in-memory PE header.
pub unsafe fn pe_timestamp(base: usize) -> u32 {
    unsafe {
        if base == 0 {
            return 0;
        }
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        core::ptr::read_unaligned((base + e_lfanew + 8) as *const u32)
    }
}

/// Number of sections in the in-memory PE header. The shipped CMBS executable
/// is packed into two unnamed sections; the real engine image (and the unpacked
/// SF2/CW builds) have five or six. Used to avoid scanning a packed image.
pub unsafe fn module_section_count(base: usize) -> usize {
    unsafe {
        if base == 0 {
            return 0;
        }
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        core::ptr::read_unaligned((base + e_lfanew + 6) as *const u16) as usize
    }
}

/// `(rva, size)` of the code section from the in-memory section table.
///
/// The unpacked CMBS image has **no section names** (the packer zeroes them), so
/// a `.text` name lookup alone is not enough: fall back to the first section
/// flagged as executable code.
pub unsafe fn module_text_range(base: usize) -> Option<(usize, usize)> {
    unsafe {
        if base == 0 {
            return None;
        }
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        let nsec = core::ptr::read_unaligned((base + e_lfanew + 6) as *const u16) as usize;
        let opt_size = core::ptr::read_unaligned((base + e_lfanew + 20) as *const u16) as usize;
        if nsec == 0 || nsec > 96 || opt_size < 96 {
            return None;
        }
        const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
        const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
        let mut sec = base + e_lfanew + 24 + opt_size;
        let mut first_code = None;
        for _ in 0..nsec {
            let name = core::slice::from_raw_parts(sec as *const u8, 8);
            let vsize = core::ptr::read_unaligned((sec + 8) as *const u32) as usize;
            let vaddr = core::ptr::read_unaligned((sec + 12) as *const u32) as usize;
            let chars = core::ptr::read_unaligned((sec + 36) as *const u32);
            if vsize > 0 {
                if name.starts_with(b".text") {
                    return Some((vaddr, vsize));
                }
                if first_code.is_none()
                    && chars & IMAGE_SCN_MEM_EXECUTE != 0
                    && chars & IMAGE_SCN_CNT_CODE != 0
                {
                    first_code = Some((vaddr, vsize));
                }
            }
            sec += 40;
        }
        first_code
    }
}

/// Preferred image base from the in-memory PE optional header.
unsafe fn module_preferred_base(base: usize) -> usize {
    unsafe {
        if base == 0 {
            return 0;
        }
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        core::ptr::read_unaligned((base + e_lfanew + 24 + 28) as *const u32) as usize
    }
}

/// Dumps the live main module to `analysis/<exe-stem>-image-pe.bin` and returns
/// the path. `dump_module=1` enables it once at startup.
///
/// The copy is made section by section through `ReadProcessMemory`, so gaps
/// between sections that the loader left uncommitted cannot fault the game.
/// The section headers are rewritten from the mapped layout (`VirtualSize`,
/// `VirtualAddress`) to a file layout (`SizeOfRawData`, `PointerToRawData`),
/// which lets Ghidra and other decompilers import the result as a normal PE
/// instead of a raw blob.
///
/// Unpacked titles are already valid on disk; this is for the packed CMBS
/// image, where the on-disk code is not the code that runs.
pub unsafe fn dump_main_module() -> Option<std::path::PathBuf> {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        let image_size = module_image_size(base);
        if base == 0 || image_size == 0 || image_size > 512 * 1024 * 1024 {
            return None;
        }

        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        if e_lfanew == 0 || e_lfanew > 0x10000 {
            return None;
        }
        let nsec = core::ptr::read_unaligned((base + e_lfanew + 6) as *const u16) as usize;
        let opt_size = core::ptr::read_unaligned((base + e_lfanew + 20) as *const u16) as usize;
        let opt = base + e_lfanew + 24;
        let size_headers = core::ptr::read_unaligned((opt + 60) as *const u32) as usize;
        if nsec == 0 || nsec > 96 || opt_size < 96 || size_headers > image_size {
            return None;
        }

        let mut image = vec![0u8; image_size];
        if !rd_bytes(base, &mut image[..size_headers]) {
            return None;
        }

        let sec_table = e_lfanew + 24 + opt_size;
        let mut copied = 0usize;
        for i in 0..nsec {
            let s = sec_table + i * 40;
            let vsize = core::ptr::read_unaligned((base + s + 8) as *const u32) as usize;
            let vaddr = core::ptr::read_unaligned((base + s + 12) as *const u32) as usize;
            if vsize == 0 || vaddr >= image_size {
                continue;
            }
            let len = vsize.min(image_size - vaddr);
            if rd_bytes(base + vaddr, &mut image[vaddr..vaddr + len]) {
                copied += len;
            }
            // File alignment, not section alignment, for the on-disk copy.
            let raw_size = len.div_ceil(0x200) * 0x200;
            image[s + 16..s + 20].copy_from_slice(&(raw_size as u32).to_le_bytes());
            image[s + 20..s + 24].copy_from_slice(&(vaddr as u32).to_le_bytes());
        }
        if copied == 0 {
            return None;
        }

        let mut name_buf = [0u16; 512];
        let n = GetModuleFileNameW(
            core::ptr::null_mut(),
            name_buf.as_mut_ptr(),
            name_buf.len() as u32,
        );
        let exe = if n == 0 {
            "game".to_string()
        } else {
            let path = str_from_wide(name_buf.as_ptr());
            std::path::Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().replace(' ', "-"))
                .unwrap_or_else(|| "game".to_string())
        };

        let dir = exe_dir().join("analysis");
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let out = dir.join(format!("{exe}-image-pe.bin"));
        std::fs::write(&out, &image).ok()?;
        Some(out)
    }
}

/// Text-relative offsets of the load-time `HIGHLOW` relocations the loader has
/// already applied in memory, sorted and de-duplicated.
///
/// The unpacked titles have ASLR and many absolute pointers in `.text`; the
/// loader adds the load delta to each one, so hashing mapped bytes directly
/// makes the same build hash differently at every base. Rewinding each fixed-up
/// dword restores the on-disk value and keeps `text_fnv1a` a build identity.
/// Returns an empty vector when the image carries no text relocations and
/// `None` when the relocation directory cannot be parsed.
unsafe fn module_text_relocations(
    base: usize,
    text_rva: usize,
    text_size: usize,
) -> Option<Vec<usize>> {
    unsafe {
        const IMAGE_DIRECTORY_ENTRY_BASERELOC: usize = 5;
        const IMAGE_REL_BASED_HIGHLOW: u16 = 3;

        let image = module_image_size(base);
        let e_lfanew = core::ptr::read_unaligned((base + 0x3c) as *const u32) as usize;
        let opt = base + e_lfanew + 24;
        let dir = opt + 96 + IMAGE_DIRECTORY_ENTRY_BASERELOC * 8;
        let reloc_rva = core::ptr::read_unaligned(dir as *const u32) as usize;
        let reloc_size = core::ptr::read_unaligned((dir + 4) as *const u32) as usize;
        if reloc_size == 0 {
            return Some(Vec::new());
        }
        if reloc_rva == 0
            || !(8..=16 * 1024 * 1024).contains(&reloc_size)
            || reloc_rva.checked_add(reloc_size)? > image
        {
            return None;
        }

        let text_lo = text_rva;
        let text_hi = text_rva.checked_add(text_size)?;
        let mut out = Vec::new();
        let mut off = reloc_rva;
        let end = reloc_rva + reloc_size;
        while off + 8 <= end {
            let page = core::ptr::read_unaligned((base + off) as *const u32) as usize;
            let block = core::ptr::read_unaligned((base + off + 4) as *const u32) as usize;
            if block == 0 {
                break;
            }
            if block < 8 || off.checked_add(block)? > end {
                return None;
            }
            let entries = (block - 8) / 2;
            for i in 0..entries {
                let ent = core::ptr::read_unaligned((base + off + 8 + i * 2) as *const u16);
                if ent >> 12 != IMAGE_REL_BASED_HIGHLOW {
                    continue;
                }
                let target = page + (ent & 0x0fff) as usize;
                if target >= text_lo && target < text_hi {
                    out.push(target - text_lo);
                }
            }
            off += block;
        }
        out.sort_unstable();
        out.dedup();
        Some(out)
    }
}

/// FNV-1a over `size` mapped bytes, substituting the on-disk value for every
/// already-relocated dword in `relocs`.
unsafe fn fnv1a_canonical(text: usize, size: usize, delta: u32, relocs: &[usize]) -> u32 {
    unsafe {
        let mut hash = 0x811c_9dc5u32;
        let mut i = 0usize;
        let mut r = 0usize;
        while i < size {
            if r < relocs.len() && relocs[r] < i {
                r += 1;
                continue;
            }
            if r < relocs.len() && relocs[r] == i && i + 4 <= size {
                let v = core::ptr::read_unaligned((text + i) as *const u32).wrapping_sub(delta);
                for b in v.to_le_bytes() {
                    hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
                }
                i += 4;
                r += 1;
            } else {
                let b = core::ptr::read_unaligned((text + i) as *const u8);
                hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
                i += 1;
            }
        }
        hash
    }
}

/// FNV-1a over the mapped `.text`, normalized for load-time relocations. Cheap
/// (a few ms for a 6 MB section), and a much stronger build discriminator than
/// `SizeOfImage` alone. Returns `None` when the section cannot be located.
pub unsafe fn module_text_fnv1a(base: usize) -> Option<u32> {
    unsafe {
        // The unpacked titles are protected with a loader that rewrites the PE
        // header's ImageBase to the address it actually mapped the image at. The
        // original base is still in the file on disk, and the relocation deltas
        // were computed from it, so prefer that. Fall back to the in-memory header
        // for ordinary images (and when the file cannot be read).
        let preferred = disk_preferred_base().unwrap_or_else(|| module_preferred_base(base));
        module_text_fnv1a_with_preferred(base, preferred)
    }
}

/// Preferred image base from the executable on disk (PE32 or PE32+).
fn disk_preferred_base() -> Option<usize> {
    unsafe {
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        let path = str_from_wide(buf.as_ptr());
        let mut v = vec![0u8; 0x1000];
        let n = {
            use std::io::Read;
            std::fs::File::open(&path).ok()?.read(&mut v).ok()?
        };
        v.truncate(n);
        if v.len() < 0x40 || &v[0..2] != b"MZ" {
            return None;
        }
        let e = u32::from_le_bytes(v[0x3c..0x40].try_into().ok()?) as usize;
        if e.checked_add(24 + 32)? > v.len() || &v[e..e + 4] != b"PE\0\0" {
            return None;
        }
        let opt = e + 24;
        match u16::from_le_bytes(v[opt..opt + 2].try_into().ok()?) {
            0x10b => Some(u32::from_le_bytes(v[opt + 28..opt + 32].try_into().ok()?) as usize),
            0x20b => Some(u64::from_le_bytes(v[opt + 24..opt + 32].try_into().ok()?) as usize),
            _ => None,
        }
    }
}

unsafe fn module_text_fnv1a_with_preferred(base: usize, preferred: usize) -> Option<u32> {
    unsafe {
        let (rva, size) = module_text_range(base)?;
        let image = module_image_size(base);
        if size == 0 || size > 64 * 1024 * 1024 || rva.checked_add(size)? > image {
            return None;
        }
        let delta = base.wrapping_sub(preferred) as u32;
        if delta != 0 {
            if let Some(relocs) = module_text_relocations(base, rva, size) {
                if !cfg!(test) {
                    log(&format!(
                        "identity: text base=0x{base:x} preferred=0x{preferred:x} relocs={}",
                        relocs.len()
                    ));
                }
                return Some(fnv1a_canonical(base + rva, size, delta, &relocs));
            }
            if !cfg!(test) {
                log(&format!(
                    "identity: text reloc table unparsable (base=0x{base:x} preferred=0x{preferred:x}); using mapped bytes"
                ));
            }
        }
        let text = core::slice::from_raw_parts((base + rva) as *const u8, size);
        let mut hash = 0x811c_9dc5u32;
        for &b in text {
            hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
        }
        Some(hash)
    }
}

/// Resolve one profile slot RVA to its runtime address.
pub fn slot_addr(rva: u32) -> Option<usize> {
    profile_addr(rva)
}

pub fn slot_close_window() -> Option<usize> {
    slot_addr(slots()?.close_window)
}

pub fn slot_get_system_metrics() -> Option<usize> {
    slot_addr(slots()?.get_system_metrics)
}

pub fn slot_change_display() -> Option<usize> {
    slot_addr(slots()?.change_display)
}

pub fn slot_get_cursor_pos() -> Option<usize> {
    slot_addr(slots()?.get_cursor_pos)
}

/// Read each configured slot and check that it still points at the real user32
/// function. A match is the strongest proof that the profile belongs to the
/// loaded image; a mismatch disables only that hook.
pub unsafe fn validate_profile_slots() {
    unsafe {
        let mut mask = 0u32;
        let mut check = |bit: u32, slot: Option<usize>, real: usize, name: &str| {
            let (Some(slot), real) = (slot, real) else {
                return;
            };
            if real == 0 {
                // The real function did not resolve; leave the bit clear so the
                // generic scanner can still find the slot by value elsewhere.
                return;
            }
            let actual = core::ptr::read_unaligned(slot as *const usize);
            if actual == real {
                mask |= bit;
            } else {
                log(&format!(
                    "slots: {name} content mismatch at 0x{slot:x}: 0x{actual:x} != 0x{real:x}; \
                 hook disabled, scanner will retry"
                ));
            }
        };
        check(
            SLOT_BIT_CLOSE_WINDOW,
            slot_close_window(),
            REAL_CLOSE_WINDOW.load(Ordering::Relaxed),
            "CloseWindow",
        );
        check(
            SLOT_BIT_GET_SYSTEM_METRICS,
            slot_get_system_metrics(),
            REAL_METRICS.load(Ordering::Relaxed),
            "GetSystemMetrics",
        );
        check(
            SLOT_BIT_CHANGE_DISPLAY,
            slot_change_display(),
            REAL_CHANGE_DISPLAY.load(Ordering::Relaxed),
            "ChangeDisplaySettingsA",
        );
        check(
            SLOT_BIT_GET_CURSOR_POS,
            slot_get_cursor_pos(),
            REAL_GET_CURSOR_POS.load(Ordering::Relaxed),
            "GetCursorPos",
        );
        SLOT_OK_MASK.store(mask, Ordering::Relaxed);
        let configured = slots().map(|s| s.any()).unwrap_or(false);
        if configured {
            log(&format!(
                "slots: content check {}/4 passed",
                mask.count_ones()
            ));
        }
    }
}

/// Scan the main module's committed pages for function pointers that still
/// point at the real user32 functions, and replace them with our hooks.
///
/// The shipped executables are packed and build their import tables in memory,
/// so there is no usable import directory to walk. Matching the exact user32
/// pointer value is the one build-independent way to find those slots.
pub unsafe fn scan_slots() {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        let size = module_image_size(base);
        if base == 0 || size == 0 || !slots_pending() {
            return;
        }

        let mask = SLOT_OK_MASK.load(Ordering::Relaxed);
        let mut reals = [0usize; 7];
        let mut hooks = [0usize; 7];
        let mut counts = [0u32; 7];
        let mut first = [0usize; 7];
        let mut n = 0usize;

        let mut add = |real: usize, hook: usize| {
            if real != 0 && n < reals.len() {
                reals[n] = real;
                hooks[n] = hook;
                n += 1;
            }
        };

        // Slots the profile supplied *and* that passed the content check are
        // already patched; only the missing/failed ones are searched for here.
        if mask & SLOT_BIT_CLOSE_WINDOW == 0 {
            add(
                REAL_CLOSE_WINDOW.load(Ordering::Relaxed),
                hook_close_window as *const () as usize,
            );
        }
        if mask & SLOT_BIT_GET_SYSTEM_METRICS == 0 {
            add(
                REAL_METRICS.load(Ordering::Relaxed),
                hook_metrics as *const () as usize,
            );
        }
        if mask & SLOT_BIT_CHANGE_DISPLAY == 0 {
            add(
                REAL_CHANGE_DISPLAY.load(Ordering::Relaxed),
                hook_change_display as *const () as usize,
            );
        }
        if mask & SLOT_BIT_GET_CURSOR_POS == 0 {
            add(
                REAL_GET_CURSOR_POS.load(Ordering::Relaxed),
                hook_get_cursor_pos as *const () as usize,
            );
        }
        if SCAN_SHOW.load(Ordering::Relaxed) == 0 {
            add(
                REAL_SHOW_WINDOW.load(Ordering::Relaxed),
                hook_show_window as *const () as usize,
            );
        }
        if SCAN_SHOW_ASYNC.load(Ordering::Relaxed) == 0 {
            add(
                REAL_SHOW_WINDOW_ASYNC.load(Ordering::Relaxed),
                hook_show_window_async as *const () as usize,
            );
        }
        if REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed) != 0 {
            add(
                REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed),
                hook_async_key_state as *const () as usize,
            );
        }
        if n == 0 {
            return;
        }

        let image_end = base + size;
        let mut addr = base;
        while addr < image_end {
            let mut info: MemBasicInfo = core::mem::zeroed();
            let got = VirtualQuery(
                addr as *const c_void,
                &mut info,
                core::mem::size_of::<MemBasicInfo>(),
            );
            if got != core::mem::size_of::<MemBasicInfo>() {
                break;
            }
            let rbase = info.base_address as usize;
            let rsize = info.region_size.max(1);
            let next = rbase.saturating_add(rsize);
            if next <= addr {
                break;
            }
            addr = next;

            if info.state != MEM_COMMIT
                || info.protect & PAGE_NOACCESS != 0
                || info.protect & PAGE_GUARD != 0
            {
                continue;
            }

            let start = rbase.max(base);
            let end = next.min(image_end);
            if start >= end {
                continue;
            }

            let mut old = 0u32;
            if VirtualProtect(
                start as *mut c_void,
                end - start,
                PAGE_EXECUTE_READWRITE,
                &mut old,
            ) == 0
            {
                continue;
            }
            let mut p = (start + 3) & !3;
            while p + 4 <= end {
                let value = core::ptr::read_unaligned(p as *const usize);
                for i in 0..n {
                    if value == reals[i] && counts[i] < 4096 {
                        if first[i] == 0 {
                            first[i] = p;
                        }
                        core::ptr::write_unaligned(p as *mut usize, hooks[i]);
                        counts[i] += 1;
                    }
                }
                p += 4;
            }
            let mut ignored = 0u32;
            VirtualProtect(start as *mut c_void, end - start, old, &mut ignored);
        }

        let mut new_mask = mask;
        for i in 0..n {
            if counts[i] == 0 {
                continue;
            }
            let real = reals[i];
            if real == REAL_SHOW_WINDOW.load(Ordering::Relaxed) {
                SCAN_SHOW.store(1, Ordering::Relaxed);
                log(&format!(
                    "IAT scan: ShowWindow slot(s) patched: {}",
                    counts[i]
                ));
            } else if real == REAL_SHOW_WINDOW_ASYNC.load(Ordering::Relaxed) {
                SCAN_SHOW_ASYNC.store(1, Ordering::Relaxed);
                log(&format!(
                    "IAT scan: ShowWindowAsync slot(s) patched: {}",
                    counts[i]
                ));
            } else if real == REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed) {
                SCAN_ASYNC_KEY.store(1, Ordering::Relaxed);
                log(&format!(
                    "IAT scan: GetAsyncKeyState slot(s) patched: {}",
                    counts[i]
                ));
            } else {
                let name = if real == REAL_CLOSE_WINDOW.load(Ordering::Relaxed) {
                    new_mask |= SLOT_BIT_CLOSE_WINDOW;
                    "CloseWindow"
                } else if real == REAL_METRICS.load(Ordering::Relaxed) {
                    new_mask |= SLOT_BIT_GET_SYSTEM_METRICS;
                    "GetSystemMetrics"
                } else if real == REAL_CHANGE_DISPLAY.load(Ordering::Relaxed) {
                    new_mask |= SLOT_BIT_CHANGE_DISPLAY;
                    "ChangeDisplaySettingsA"
                } else if real == REAL_GET_CURSOR_POS.load(Ordering::Relaxed) {
                    new_mask |= SLOT_BIT_GET_CURSOR_POS;
                    "GetCursorPos"
                } else {
                    "user32"
                };
                log(&format!(
                    "IAT scan: {name} slot=0x{:x} rva=0x{:x} sites={}",
                    first[i],
                    first[i].wrapping_sub(base),
                    counts[i]
                ));
            }
        }
        SLOT_OK_MASK.store(new_mask, Ordering::Relaxed);
        SCAN_TICKS.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn slots_pending() -> bool {
    let mask = SLOT_OK_MASK.load(Ordering::Relaxed);
    SCAN_SHOW.load(Ordering::Relaxed) == 0
        || SCAN_SHOW_ASYNC.load(Ordering::Relaxed) == 0
        || (mask & SLOT_BIT_GET_SYSTEM_METRICS == 0 && REAL_METRICS.load(Ordering::Relaxed) != 0)
        || (mask & SLOT_BIT_GET_CURSOR_POS == 0 && REAL_GET_CURSOR_POS.load(Ordering::Relaxed) != 0)
        || (mask & SLOT_BIT_CHANGE_DISPLAY == 0 && REAL_CHANGE_DISPLAY.load(Ordering::Relaxed) != 0)
        || (mask & SLOT_BIT_CLOSE_WINDOW == 0 && REAL_CLOSE_WINDOW.load(Ordering::Relaxed) != 0)
        || (SCAN_ASYNC_KEY.load(Ordering::Relaxed) == 0
            && REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed) != 0)
}

/// Return neutral state for camera keys/buttons while a mod owns the camera.
/// The mod DLL reads user32 directly, so this only affects engine IAT sites.
pub unsafe extern "system" fn hook_async_key_state(vk: i32) -> i16 {
    let real = REAL_ASYNC_KEY_STATE.load(Ordering::Acquire);
    if real == 0 {
        return 0;
    }
    unsafe {
        let f: unsafe extern "system" fn(i32) -> i16 = core::mem::transmute(real);
        let state = f(vk);
        if camera_input_capture() && matches!(vk, 0x51 | 0x45) {
            // Q/E belong to the camera submod while capture is active. Never
            // let the game's configurable native action map turn them into
            // elevation commands or synthetic yaw inputs.
            return 0;
        }
        if camera_input_capture() && vk == 0x02 {
            set_camera_rmb_held(state < 0);
            return 0;
        }
        if camera_input_capture() && matches!(vk, 0x41 | 0x44 | 0x53 | 0x57) {
            return 0;
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PE32 header: e_lfanew=0x80, .text at RVA 0x1000, size 0x100.
    fn synthetic_image() -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        b[0x88..0x8c].copy_from_slice(&0x63d82c90u32.to_le_bytes()); // TimeDateStamp
        b[0x94..0x96].copy_from_slice(&224u16.to_le_bytes()); // SizeOfOptionalHeader
        let soi = 0x80 + 24 + 56;
        b[soi..soi + 4].copy_from_slice(&0x4000u32.to_le_bytes()); // SizeOfImage
        let sec = 0x80 + 24 + 224;
        b[sec..sec + 5].copy_from_slice(b".text");
        b[sec + 8..sec + 12].copy_from_slice(&0x100u32.to_le_bytes()); // VirtualSize
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
        b[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes()); // code+exec
        b
    }

    #[test]
    fn parses_pe_timestamp_from_a_synthetic_header() {
        let image = synthetic_image();
        unsafe {
            assert_eq!(pe_timestamp(image.as_ptr() as usize), 0x63d82c90);
            assert_eq!(module_section_count(image.as_ptr() as usize), 1);
            assert_eq!(
                module_text_range(image.as_ptr() as usize),
                Some((0x1000, 0x100))
            );
        }
    }

    #[test]
    fn finds_the_code_section_when_names_are_stripped() {
        // The unpacked CMBS image has no section names; the first executable
        // code section is the text range.
        let mut image = synthetic_image();
        let sec = 0x80 + 24 + 224;
        for b in &mut image[sec..sec + 8] {
            *b = 0;
        }
        unsafe {
            assert_eq!(
                module_text_range(image.as_ptr() as usize),
                Some((0x1000, 0x100))
            );
        }
    }

    #[test]
    fn text_hash_covers_the_mapped_code_section() {
        let mut image = vec![0u8; 0x2000];
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        image[0x88..0x8c].copy_from_slice(&0x63d82c90u32.to_le_bytes());
        image[0x94..0x96].copy_from_slice(&224u16.to_le_bytes());
        let soi = 0x80 + 24 + 56;
        image[soi..soi + 4].copy_from_slice(&0x2000u32.to_le_bytes());
        let sec = 0x80 + 24 + 224;
        for b in &mut image[sec..sec + 8] {
            *b = 0; // no section names
        }
        image[sec + 8..sec + 12].copy_from_slice(&0x10u32.to_le_bytes());
        image[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        image[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes());
        for (i, b) in image[0x1000..0x1010].iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut want = 0x811c_9dc5u32;
        for i in 0..0x10u32 {
            want = (want ^ i).wrapping_mul(0x0100_0193);
        }
        unsafe {
            let base = image.as_ptr() as usize;
            assert_eq!(module_text_fnv1a_with_preferred(base, base), Some(want));
        }
    }

    #[test]
    fn text_hash_undoes_load_time_relocations() {
        // PE32 preferred at 0x400000 with .text at RVA 0x1000. The dword at
        // .text+8 is a HIGHLOW relocation, so the loader adds the load delta to
        // it. The hash must still be the canonical on-disk value.
        let mut image = vec![0u8; 0x2000];
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        image[0x94..0x96].copy_from_slice(&224u16.to_le_bytes());
        let opt = 0x80 + 24;
        image[opt + 28..opt + 32].copy_from_slice(&0x400000u32.to_le_bytes());
        image[opt + 56..opt + 60].copy_from_slice(&0x2000u32.to_le_bytes());
        let dd = opt + 96 + 5 * 8; // IMAGE_DIRECTORY_ENTRY_BASERELOC
        image[dd..dd + 4].copy_from_slice(&0x1100u32.to_le_bytes());
        image[dd + 4..dd + 8].copy_from_slice(&12u32.to_le_bytes());
        image[0x1100..0x1104].copy_from_slice(&0x1000u32.to_le_bytes()); // page
        image[0x1104..0x1108].copy_from_slice(&12u32.to_le_bytes()); // block size
        image[0x1108..0x110a].copy_from_slice(&0x3008u16.to_le_bytes()); // HIGHLOW +8
        image[0x110a..0x110c].copy_from_slice(&0u16.to_le_bytes()); // ABSOLUTE pad
        let sec = 0x80 + 24 + 224;
        image[sec..sec + 5].copy_from_slice(b".text");
        image[sec + 8..sec + 12].copy_from_slice(&0x20u32.to_le_bytes());
        image[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        image[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes());

        let text = 0x1000usize;
        for i in 0..0x20 {
            image[text + i] = (i as u8).wrapping_mul(7).wrapping_add(1);
        }
        let disk = u32::from_le_bytes(image[text + 8..text + 12].try_into().unwrap());
        let base = image.as_ptr() as usize;
        let delta = (base - 0x400000) as u32;
        image[text + 8..text + 12].copy_from_slice(&disk.wrapping_add(delta).to_le_bytes());

        let mut want = 0x811c_9dc5u32;
        let mut canonical = image[text..text + 0x20].to_vec();
        canonical[8..12].copy_from_slice(&disk.to_le_bytes());
        for b in canonical {
            want = (want ^ b as u32).wrapping_mul(0x0100_0193);
        }
        unsafe {
            assert_eq!(module_text_fnv1a_with_preferred(base, 0x400000), Some(want));
        }
    }
}
