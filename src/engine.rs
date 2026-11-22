//! Patching the packed engine's in-memory user32 import slots.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::ffi::c_void;

// ---- code ----
// ------------------------------------------------------------- slot hooking

/// Read only aligned slots backed by the expected image allocation.
unsafe fn image_slot(base: usize, end: usize, slot: usize) -> Option<usize> {
    let len = size_of::<usize>();
    let stop = slot.checked_add(len)?;
    if slot < base || stop > end || !slot.is_multiple_of(len) {
        return None;
    }
    unsafe {
        let mut info: MemBasicInfo = core::mem::zeroed();
        if VirtualQuery(slot as *const c_void, &mut info, size_of::<MemBasicInfo>()) == 0
            || info.allocation_base as usize != base
            || info.state != MEM_COMMIT
            || info.protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            || !matches!(info.protect & 0xff, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80)
            || stop > (info.base_address as usize).checked_add(info.region_size)?
        {
            return None;
        }
        Some((&*(slot as *const AtomicUsize)).load(Ordering::Relaxed))
    }
}

unsafe fn replace_slot(base: usize, end: usize, slot: usize, expected: usize, hook: usize) -> bool {
    unsafe {
        if expected == 0 || image_slot(base, end, slot) != Some(expected) {
            return false;
        }
        let len = size_of::<usize>();
        let mut old = 0;
        if VirtualProtect(slot as *mut c_void, len, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        let changed = (&*(slot as *const AtomicUsize))
            .compare_exchange(expected, hook, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok();
        let mut ignored = 0;
        if VirtualProtect(slot as *mut c_void, len, old, &mut ignored) == 0 {
            log(&format!(
                "slots: could not restore protection at 0x{slot:x}"
            ));
        }
        changed
    }
}

pub unsafe fn install_slot_hooks() {
    if !profile_ok() {
        return;
    }
    unsafe {
        let base = module_base();
        let Some(end) = base.checked_add(run_image_size() as usize) else {
            return;
        };
        static LAST_IMPORT_CHECK: AtomicU32 = AtomicU32::new(0);
        let now = GetTickCount();
        let last = LAST_IMPORT_CHECK.load(Ordering::Relaxed);
        if (last == 0 || now.wrapping_sub(last) >= 500)
            && LAST_IMPORT_CHECK
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            install_verified_imports(base, end);
        }
        for (slot, expected, hook, name) in profile_slots() {
            if let Some(slot) = slot
                && replace_slot(base, end, slot, expected, hook)
            {
                log(&format!("slots: installed {name} at 0x{slot:x}"));
            }
        }
    }
}

/// Match an import's DLL and name through its original lookup table before
/// considering the bound IAT word. Packed executables may omit this metadata.
unsafe fn install_verified_imports(base: usize, end: usize) {
    let imports = profile_slots();
    let addr = |rva: usize, len: usize| {
        let start = base.checked_add(rva)?;
        (start.checked_add(len)? <= end).then_some(start)
    };
    let word = |rva| unsafe { rd_u32(addr(rva, 4)?) }.map(|n| n as usize);
    let named = |rva, expected: &str| {
        let mut bytes = [0; 32];
        let n = expected.len();
        addr(rva, n + 1).is_some_and(|p| unsafe { rd_bytes(p, &mut bytes[..=n]) })
            && bytes[n] == 0
            && bytes[..n].eq_ignore_ascii_case(expected.as_bytes())
    };
    let Some(nt) = word(0x3c).filter(|&nt| addr(nt, 0x88).is_some() && word(nt) == Some(0x4550))
    else {
        return;
    };
    if word(nt + 24).is_none_or(|header| header & 0xffff != 0x10b) {
        return;
    }
    let (Some(directory), Some(bytes)) = (word(nt + 0x80), word(nt + 0x84)) else {
        return;
    };
    if directory == 0 || addr(directory, bytes).is_none() {
        return;
    }
    for descriptor in (directory..directory + bytes / 20 * 20)
        .step_by(20)
        .take(1024)
    {
        let (Some(lookup), Some(name), Some(iat)) = (
            word(descriptor),
            word(descriptor + 12),
            word(descriptor + 16),
        ) else {
            break;
        };
        if lookup == 0 || iat == 0 || !named(name, "user32.dll") {
            continue;
        }
        for offset in (0..16_384).step_by(4) {
            let Some(import) = lookup
                .checked_add(offset)
                .and_then(word)
                .filter(|&import| import != 0)
            else {
                break;
            };
            if import & 0x8000_0000 != 0 {
                continue;
            }
            let Some(slot) = iat.checked_add(offset).and_then(|rva| addr(rva, 4)) else {
                break;
            };
            for (_, expected, hook, name) in imports {
                if name == "CloseWindow" && !config().fix_alt_tab {
                    continue;
                }
                if unsafe { image_slot(base, end, slot) } == Some(expected)
                    && expected != 0
                    && named(import + 2, name)
                    && unsafe { replace_slot(base, end, slot, expected, hook) }
                {
                    log(&format!(
                        "slots: installed verified {name} import at 0x{slot:x}"
                    ));
                    break;
                }
            }
        }
    }
}

fn profile_slots() -> [(Option<usize>, usize, usize, &'static str); 7] {
    [
        (
            slot_close_window().filter(|_| config().fix_alt_tab),
            REAL_CLOSE_WINDOW.load(Ordering::Relaxed),
            hook_close_window as *const () as usize,
            "CloseWindow",
        ),
        (
            slot_get_system_metrics(),
            REAL_METRICS.load(Ordering::Relaxed),
            hook_metrics as *const () as usize,
            "GetSystemMetrics",
        ),
        (
            slot_change_display(),
            REAL_CHANGE_DISPLAY.load(Ordering::Relaxed),
            hook_change_display as *const () as usize,
            "ChangeDisplaySettingsA",
        ),
        (
            slot_get_cursor_pos(),
            REAL_GET_CURSOR_POS.load(Ordering::Relaxed),
            hook_get_cursor_pos as *const () as usize,
            "GetCursorPos",
        ),
        (
            slots().and_then(|s| slot_addr(s.show_window)),
            REAL_SHOW_WINDOW.load(Ordering::Relaxed),
            hook_show_window as *const () as usize,
            "ShowWindow",
        ),
        (
            slots().and_then(|s| slot_addr(s.show_window_async)),
            REAL_SHOW_WINDOW_ASYNC.load(Ordering::Relaxed),
            hook_show_window_async as *const () as usize,
            "ShowWindowAsync",
        ),
        (
            slots().and_then(|s| slot_addr(s.get_async_key_state)),
            REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed),
            hook_async_key_state as *const () as usize,
            "GetAsyncKeyState",
        ),
    ]
}

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
        let base = module_base();
        let Some(end) = base.checked_add(run_image_size() as usize) else {
            return;
        };
        for (slot, expected, hook, name) in profile_slots() {
            if let Some(slot) = slot {
                let actual = image_slot(base, end, slot);
                if actual != Some(expected) && actual != Some(hook) {
                    log(&format!(
                        "slots: {name} at 0x{slot:x} has unexpected value {actual:?}; will retry when imports resolve"
                    ));
                }
            }
        }
    }
}

/// Candidate discovery for packed builds. These values do not authorize a patch.
pub unsafe fn diagnose_slots(base: usize, size: usize) {
    unsafe {
        let Some(end) = base.checked_add(size) else {
            return;
        };
        let targets = [
            ("CloseWindow", REAL_CLOSE_WINDOW.load(Ordering::Relaxed)),
            ("GetSystemMetrics", REAL_METRICS.load(Ordering::Relaxed)),
            (
                "ChangeDisplaySettingsA",
                REAL_CHANGE_DISPLAY.load(Ordering::Relaxed),
            ),
            ("GetCursorPos", REAL_GET_CURSOR_POS.load(Ordering::Relaxed)),
            ("ShowWindow", REAL_SHOW_WINDOW.load(Ordering::Relaxed)),
            (
                "ShowWindowAsync",
                REAL_SHOW_WINDOW_ASYNC.load(Ordering::Relaxed),
            ),
            (
                "GetAsyncKeyState",
                REAL_ASYNC_KEY_STATE.load(Ordering::Relaxed),
            ),
        ];
        let mut counts = [0; 7];
        let mut addr = base;
        while addr < end {
            let mut info: MemBasicInfo = core::mem::zeroed();
            if VirtualQuery(addr as *const c_void, &mut info, size_of::<MemBasicInfo>()) == 0 {
                break;
            }
            let next = (info.base_address as usize)
                .saturating_add(info.region_size)
                .min(end);
            if next <= addr {
                break;
            }
            if info.allocation_base as usize == base
                && info.state == MEM_COMMIT
                && info.protect & PAGE_GUARD == 0
                && matches!(info.protect & 0xff, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80)
            {
                let mut page = [0u8; 4096];
                for chunk in (addr..next).step_by(page.len()) {
                    let length = (next - chunk).min(page.len());
                    if !rd_bytes(chunk, &mut page[..length]) {
                        continue;
                    }
                    for (word, bytes) in page[..length].as_chunks::<4>().0.iter().enumerate() {
                        let value = u32::from_le_bytes(*bytes) as usize;
                        for (index, (name, expected)) in targets.iter().enumerate() {
                            if value != 0 && value == *expected && counts[index] < 8 {
                                counts[index] += 1;
                                log(&format!(
                                    "IAT scan: {name} candidate RVA 0x{:x}",
                                    chunk + word * 4 - base
                                ));
                            }
                        }
                    }
                }
            }
            addr = next;
        }
    }
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
        if camera_input_capture() && vk == 0x04 {
            return 0;
        }
        if camera_input_capture() && matches!(vk, 0x41 | 0x44 | 0x53 | 0x57) {
            return 0;
        }
        if camera_input_capture() && camera_freecam_active() && matches!(vk, 0x52 | 0x46) {
            // In free-camera mode R/F become vertical flight controls owned by
            // the submod instead of the engine's elevation bindings.
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
    fn slot_write_refuses_unexpected_values_and_failed_protection() {
        unsafe extern "system" {
            fn CreateFileMappingW(
                file: Handle,
                security: *mut c_void,
                protect: u32,
                high: u32,
                low: u32,
                name: *const u16,
            ) -> Handle;
            fn MapViewOfFile(
                mapping: Handle,
                access: u32,
                high: u32,
                low: u32,
                size: usize,
            ) -> *mut c_void;
            fn UnmapViewOfFile(view: *const c_void) -> i32;
        }
        unsafe {
            let mapping = CreateFileMappingW(
                (-1isize) as Handle,
                core::ptr::null_mut(),
                PAGE_READWRITE,
                0,
                4096,
                core::ptr::null(),
            );
            assert!(!mapping.is_null());
            let writer = MapViewOfFile(mapping, 2, 0, 0, 4096);
            let reader = MapViewOfFile(mapping, 4, 0, 0, 4096);
            assert!(!writer.is_null() && !reader.is_null());
            writer.cast::<usize>().write(7);
            let slot = reader as usize;
            assert_eq!(image_slot(slot, slot + 4096, slot), Some(7));
            assert!(!replace_slot(slot, slot + 4096, slot, 6, 9));
            assert!(!replace_slot(slot, slot + 4096, slot, 7, 9));
            assert_eq!(image_slot(slot, slot + 4096, slot), Some(7));
            UnmapViewOfFile(reader);
            UnmapViewOfFile(writer);
            CloseHandle(mapping);
        }
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
