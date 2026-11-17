//! C++ RTTI helpers: resolve a widget's class name from its vtable.
//!
//! The engine's UI objects are normal MSVC C++ objects, so every widget vtable
//! is preceded by a `RTTICompleteObjectLocator` which points at an
//! `RTTITypeDescriptor`, whose `+8` bytes are the decorated class name
//! (`.?AVCommandView@@`, ...). Three fault-safe reads therefore turn an
//! anonymous widget pointer into a stable, build-specific class name:
//!
//! ```text
//! vtable[-4]  -> complete object locator
//! col + 0x0c  -> type descriptor
//! td + 0x08   -> char[] (NUL terminated)
//! ```
//!
//! Class identity is *not* derived from geometry, and it is identical on every
//! machine running the same build. `init_class_table` resolves the handful of
//! classes the layout code cares about by name, so a build that only moves
//! addresses keeps working as long as the RTTI names are present.
//!
//! Everything here is read-only.

use crate::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// The widget classes the re-layout and diagnostics use. `Other` is any
/// unknown class (including plain `Control` subclasses we do not name).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum WidgetClass {
    #[default]
    Other,
    View,
    GameView,
    CommandView,
    UnitView,
    SoldierView,
    CommandButtonPanel,
}

impl WidgetClass {
    pub fn tag(self) -> &'static str {
        match self {
            WidgetClass::Other => "Other",
            WidgetClass::View => "View",
            WidgetClass::GameView => "GameView",
            WidgetClass::CommandView => "CommandView",
            WidgetClass::UnitView => "UnitView",
            WidgetClass::SoldierView => "SoldierView",
            WidgetClass::CommandButtonPanel => "CommandButtonPanel",
        }
    }

    /// Kept as a predicate for future role checks; the current re-layout matches
    /// the concrete classes directly.
    #[allow(dead_code)]
    pub fn is_view(self) -> bool {
        matches!(self, WidgetClass::View)
    }
}

/// Classes are resolved by RTTI name at startup. A profile may also carry the
/// vtables of a known build; those are only used after their RTTI name has been
/// verified, and are resolved as `module_base + rva`.
const CLASS_NAMES: [(&str, WidgetClass); 6] = [
    ("View", WidgetClass::View),
    ("GameView", WidgetClass::GameView),
    ("CommandView", WidgetClass::CommandView),
    ("UnitView", WidgetClass::UnitView),
    ("SoldierView", WidgetClass::SoldierView),
    ("CommandButtonPanel", WidgetClass::CommandButtonPanel),
];

static CLASS_VT: [AtomicUsize; 6] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
static CLASS_INIT: AtomicU32 = AtomicU32::new(0);
static CLASS_TRIES: AtomicU32 = AtomicU32::new(0);
static CLASS_NOT_BEFORE: AtomicU32 = AtomicU32::new(0);
static RTTI_OK: AtomicU32 = AtomicU32::new(0);

fn in_image(v: usize, base: usize, end: usize) -> bool {
    v >= base && v < end
}

#[inline]
unsafe fn rd_u32_at(addr: usize) -> Option<u32> {
    unsafe {
        let mut b = [0u8; 4];
        let mut got = 0usize;
        if ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            b.as_mut_ptr() as *mut c_void,
            b.len(),
            &mut got,
        ) != 0
            && got == b.len()
        {
            Some(u32::from_le_bytes(b))
        } else {
            None
        }
    }
}

unsafe fn rd_bytes(addr: usize, out: &mut [u8]) -> bool {
    unsafe {
        let mut got = 0usize;
        ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            out.as_mut_ptr() as *mut c_void,
            out.len(),
            &mut got,
        ) != 0
            && got == out.len()
    }
}

/// Resolve the decorated class name of a vtable. Returns `None` when the
/// pointer is not a vtable of this module or its RTTI is missing.
pub unsafe fn class_name_of_vtable(vt: usize) -> Option<String> {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            return None;
        }
        let end = base.saturating_add(module_image_size(base));
        if !in_image(vt, base, end) {
            return None;
        }
        let col = rd_u32_at(vt.wrapping_sub(4))? as usize;
        if !in_image(col, base, end) {
            return None;
        }
        let td = rd_u32_at(col.saturating_add(0x0c))? as usize;
        if !in_image(td, base, end) {
            return None;
        }
        let mut buf = [0u8; 128];
        if !rd_bytes(td.saturating_add(8), &mut buf) {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if len < 4 {
            return None;
        }
        let name = String::from_utf8_lossy(&buf[..len]).into_owned();
        Some(name)
    }
}

fn class_from_name(name: &str) -> WidgetClass {
    // RTTI names look like `.?AVCommandView@@`; accept both decorated and bare.
    let bare = name
        .trim_start_matches(".?AV")
        .trim_start_matches(".?AU")
        .trim_end_matches("@@");
    let bare = match bare.split_once('@') {
        Some((head, _)) => head,
        None => bare,
    };
    for (n, c) in CLASS_NAMES {
        if bare == n {
            return c;
        }
    }
    WidgetClass::Other
}

/// Resolve any class's vtable by RTTI name from the live, unpacked image.
/// Used for classes outside the widget table (e.g. `MapView`), so a build that
/// moves addresses only breaks if the RTTI name itself changes.
pub unsafe fn vtable_by_name(name: &str) -> Option<usize> {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            return None;
        }
        let end = base.saturating_add(module_image_size(base));
        if end <= base {
            return None;
        }
        resolve_vtable_by_name(base, end, name)
    }
}

/// Resolve one class's vtable by scanning the module image for its RTTI name.
/// The image is read through `ReadProcessMemory` in chunks, so a hostile or
/// partially committed image cannot fault the caller.
unsafe fn resolve_vtable_by_name(base: usize, end: usize, name: &str) -> Option<usize> {
    unsafe {
        let mangled = format!(".?AV{name}@@");
        let needle = mangled.as_bytes();
        let size = end.saturating_sub(base);
        if size == 0 || size > 0x0400_0000 {
            return None;
        }
        // One chunked copy of the image: the RTTI scan needs multiple passes and
        // per-byte RPM would be far too slow. The buffer is bounded at 64 MiB.
        let mut img = vec![0u8; size.min(64 * 1024 * 1024)];
        if !rd_bytes(base, &mut img) {
            return None;
        }
        let len = size.min(img.len());

        // 1. type-descriptor name string
        let mut name_off = None;
        for i in 0..len.saturating_sub(needle.len()) {
            if &img[i..i + needle.len()] == needle && img.get(i + needle.len()) == Some(&0) {
                name_off = Some(i);
                break;
            }
        }
        let name_off = name_off?;
        let td_va = base + name_off.saturating_sub(8);

        // 2/3. A complete object locator points at the type descriptor from +0x0c,
        // and a vtable stores that locator immediately before its first entry.
        // Several structures can reference the same TD, so every candidate must be
        // tried - stopping at the first one is why `MapView` failed to resolve even
        // though its RTTI is intact.
        let scan_end = len.saturating_sub(4);
        for i in (0..scan_end).step_by(4) {
            let v = u32::from_le_bytes([img[i], img[i + 1], img[i + 2], img[i + 3]]) as usize;
            if v != td_va || i < 0x0c {
                continue;
            }
            let col_off = i - 0x0c;
            if col_off + 0x14 > len {
                continue;
            }
            // Plausible locator: its class-descriptor pointer (+0x10) points into
            // the image. This rejects unrelated data that happens to hold the TD.
            let cd = u32::from_le_bytes([
                img[col_off + 0x10],
                img[col_off + 0x11],
                img[col_off + 0x12],
                img[col_off + 0x13],
            ]) as usize;
            if cd < base || cd >= base + len {
                continue;
            }
            let col_va = base + col_off;
            for k in (0..scan_end).step_by(4) {
                let w = u32::from_le_bytes([img[k], img[k + 1], img[k + 2], img[k + 3]]) as usize;
                if w == col_va {
                    let vt = base + k + 4;
                    if class_name_of_vtable(vt)
                        .map(|n| class_from_name(&n) != WidgetClass::Other || n.contains(name))
                        .unwrap_or(false)
                    {
                        return Some(vt);
                    }
                }
            }
        }
        None
    }
}

/// One-shot class-table initialisation, called from the watcher thread.
///
/// Resolution is by RTTI name and is read-only, so it runs on every build,
/// including safe mode. A profile's `[rtti]` block is a fallback for the case
/// where the name scan fails; it is only trusted after its RTTI name matches.
pub unsafe fn init_class_table() {
    unsafe {
        if CLASS_INIT.load(Ordering::Relaxed) != 0 {
            return;
        }
        let now = GetTickCount();
        if now.wrapping_sub(CLASS_NOT_BEFORE.load(Ordering::Relaxed)) > 0x8000_0000 {
            return;
        }
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        let end = base.saturating_add(module_image_size(base));
        if base == 0 || end <= base {
            return;
        }
        // The packed CMBS image has two unnamed sections and no RTTI. Do not spend
        // a 6 MB scan per class on it; wait for the unpacker to expose the real
        // image (five or six sections).
        if module_section_count(base) < 3 {
            CLASS_NOT_BEFORE.store(now.wrapping_add(250), Ordering::Relaxed);
            return;
        }

        let mut resolved = 0usize;
        for (i, (name, _)) in CLASS_NAMES.iter().enumerate() {
            let mut vt = resolve_vtable_by_name(base, end, name).unwrap_or(0);
            if vt == 0 {
                // Same build as the profile: trust its recorded vtable, but only
                // after the RTTI name check. This is an RVA, so it works on a
                // relocated image too.
                if let Some(rva) = profile::profile().and_then(|p| p.rtti_rva(name))
                    && let Some(candidate) = profile_addr(rva)
                {
                    let ok = class_name_of_vtable(candidate)
                        .map(|n| class_from_name(&n) != WidgetClass::Other && n.contains(name))
                        .unwrap_or(false);
                    if ok {
                        vt = candidate;
                    }
                }
            }
            if vt != 0 {
                resolved += 1;
            }
            CLASS_VT[i].store(vt, Ordering::Relaxed);
        }

        let ok = resolved == CLASS_NAMES.len();
        RTTI_OK.store(ok as u32, Ordering::Relaxed);
        if ok {
            CLASS_INIT.store(1, Ordering::Relaxed);
        } else {
            // The packed image has no RTTI until the unpacker has produced the real
            // image. Retry on a bounded budget instead of giving up immediately.
            let fails = CLASS_TRIES.fetch_add(1, Ordering::Relaxed);
            CLASS_NOT_BEFORE.store(now.wrapping_add(500), Ordering::Relaxed);
            if fails >= 24 {
                CLASS_INIT.store(1, Ordering::Relaxed);
                log(&format!(
                    "rtti: giving up after {fails} attempts ({resolved}/{} classes); \
                 layout uses the structural fallback",
                    CLASS_NAMES.len()
                ));
                return;
            }
        }
        if ok {
            let mut tags = String::new();
            for (i, (name, _)) in CLASS_NAMES.iter().enumerate() {
                if i > 0 {
                    tags.push(' ');
                }
                tags.push_str(&format!(
                    "{name}=0x{:x}",
                    CLASS_VT[i].load(Ordering::Relaxed)
                ));
            }
            log(&format!("rtti: class table resolved by name: {tags}"));
            // The LOS mod needs MapView; log whether it resolves by RTTI name too,
            // so a build that moved the address is visible without entering a
            // battle (the hard-coded vtable is only a fallback).
            match vtable_by_name("MapView") {
                Some(vt) => log(&format!("rtti: MapView=0x{vt:x}")),
                None => log("rtti: MapView not resolved by RTTI name"),
            }
        } else {
            let mut tags = String::new();
            for (i, (name, _)) in CLASS_NAMES.iter().enumerate() {
                if i > 0 {
                    tags.push(' ');
                }
                tags.push_str(&format!(
                    "{name}=0x{:x}",
                    CLASS_VT[i].load(Ordering::Relaxed)
                ));
            }
            log(&format!(
                "rtti: incomplete class table ({resolved}/{}): {tags}; \
             layout will use the structural fallback",
                CLASS_NAMES.len()
            ));
        }
    }
}

/// Whether RTTI class resolution succeeded. Exposed for diagnostics and future
/// mod-host capability queries.
#[allow(dead_code)]
pub fn rtti_available() -> bool {
    RTTI_OK.load(Ordering::Relaxed) != 0
}

/// Classify a vtable using the resolved table. Cheap enough for every node of
/// a widget-tree walk.
pub fn classify_vt(vt: u32) -> WidgetClass {
    let vt = vt as usize;
    for (i, (_, class)) in CLASS_NAMES.iter().enumerate() {
        if CLASS_VT[i].load(Ordering::Relaxed) == vt {
            return *class;
        }
    }
    WidgetClass::Other
}

/// Resolve a class name for the log without requiring the table (read-only).
#[allow(dead_code)]
pub unsafe fn class_name_for_log(vt: u32) -> Option<String> {
    unsafe { class_name_of_vtable(vt as usize) }
}
