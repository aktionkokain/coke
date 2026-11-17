//! Build profiles: engine facts loaded at runtime instead of compiled in.
//!
//! Every address in a profile is an **RVA** (offset from the image's preferred
//! base). The engine's packed CMBS image never relocates, but the unpacked
//! Combat Mission titles do (ASLR), so an absolute address is only valid on the
//! machine it was recorded on. Resolve through [`profile_addr`] instead.
//!
//! Selection is by build identity (`SizeOfImage` + PE timestamp [+ `.text`
//! FNV-1a]), never by address or exe name alone. The exe name only decides
//! where to *look*. No profile match means safe mode: no engine writes.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Fallback for the battle-scene top inset when a profile has no `[scene]`
/// block and the engine has not taught us the value yet. Layout constant, not
/// an address.
pub const DEFAULT_SCENE_INSET: i32 = 155;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Status {
    #[default]
    Experimental,
    Verified,
    Partial,
}

impl Status {
    fn parse(s: &str) -> Status {
        match s.to_ascii_lowercase().as_str() {
            "verified" => Status::Verified,
            "partial" => Status::Partial,
            _ => Status::Experimental,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Status::Verified => "verified",
            Status::Partial => "partial",
            Status::Experimental => "experimental",
        }
    }
}

/// The engine's in-memory user32 import slots, as RVAs. `0` = not provided.
#[derive(Clone, Copy, Default, Debug)]
pub struct Slots {
    pub close_window: u32,
    pub get_system_metrics: u32,
    pub change_display: u32,
    pub get_cursor_pos: u32,
}

impl Slots {
    pub fn any(&self) -> bool {
        self.close_window | self.get_system_metrics | self.change_display | self.get_cursor_pos != 0
    }
}

/// The app object that caches the engine canvas.
#[derive(Clone, Copy, Debug)]
pub struct Canvas {
    pub app_ptr: u32,
    /// Object offset (not an RVA): where the engine stores its HWND.
    pub app_hwnd_off: u32,
    pub size_tolerance_pct: u32,
}

/// The battle-scene objects. Offsets are object offsets, not RVAs.
#[derive(Clone, Debug)]
pub struct Scene {
    /// Static global that holds the scene owner. Zero when the build has no
    /// stable owner global (Cold War); use `widget_class` instead.
    pub owner_ptr: u32,
    pub blur_quad_ptr: u32,
    /// Resolve the scene widget from the live widget tree by RTTI class.
    /// This is the fallback for builds where the battle screen is reachable
    /// only through the heap/widget tree, not a static owner pointer.
    pub widget_class: Option<String>,
    pub top_inset: i32,
    pub owner_widget_off: u32,
    pub widget_rect_off: u32,
    pub blur_quad_rect_off: u32,
    pub blur_quad_backlink_off: u32,
    pub blur_quad_enable_off: u32,
}

impl Scene {
    /// Fill omitted offsets with the layout the engine family has used so far.
    /// Only offsets are defaulted; addresses are never invented.
    fn with_defaults(mut self) -> Scene {
        if self.owner_widget_off == 0 {
            self.owner_widget_off = 0x20;
        }
        if self.widget_rect_off == 0 {
            self.widget_rect_off = 0x10;
        }
        if self.blur_quad_rect_off == 0 {
            self.blur_quad_rect_off = 0x28;
        }
        if self.blur_quad_backlink_off == 0 {
            self.blur_quad_backlink_off = 0x54;
        }
        if self.blur_quad_enable_off == 0 {
            self.blur_quad_enable_off = 0x50;
        }
        self
    }
}

/// The `cokelos` read-only anchors. Absence disables the overlay.
#[derive(Clone, Default, Debug)]
pub struct Los {
    pub world_ptr: u32,
    pub scenario_ptr: u32,
    pub action_map_off: u32,
    pub map_globals: Vec<u32>,
    pub vt_action_map: u32,
    pub vt_terrain_map: u32,
    pub vt_mapview: u32,
    pub vt_camera: u32,
    pub vt_camera_persp: u32,
    pub vt_unit: u32,
    pub height_off: u32,
    pub grid_w_off: u32,
    pub grid_h_off: u32,
    pub world_w_off: u32,
    pub world_h_off: u32,
}

impl Los {
    fn with_defaults(mut self) -> Los {
        if self.height_off == 0 {
            self.height_off = 0x9c;
        }
        if self.grid_w_off == 0 {
            self.grid_w_off = 0xa0;
        }
        if self.grid_h_off == 0 {
            self.grid_h_off = 0xa4;
        }
        if self.world_w_off == 0 {
            self.world_w_off = 0x04;
        }
        if self.world_h_off == 0 {
            self.world_h_off = 0x08;
        }
        self
    }
}

/// One build of one executable.
#[derive(Clone, Debug)]
pub struct Profile {
    pub exe: String,
    pub build: String,
    pub status: Status,
    /// Informational only: RVA + preferred_base = the VA an external tool sees.
    pub preferred_base: u32,
    pub image_size: u32,
    pub pe_timestamp: u32,
    pub text_fnv1a: Option<u32>,
    pub slots: Slots,
    pub canvas: Option<Canvas>,
    pub scene: Option<Scene>,
    /// Class name -> vtable RVA. Names keep their original case.
    pub rtti: Vec<(String, u32)>,
    pub los: Option<Los>,
    /// Where this profile came from, for the selection log line.
    pub source: String,
}

impl Profile {
    pub fn rtti_rva(&self, name: &str) -> Option<u32> {
        self.rtti
            .iter()
            .find(|(n, rva)| n.eq_ignore_ascii_case(name) && *rva != 0)
            .map(|(_, rva)| *rva)
    }

    /// Block presence, used for the capability log line.
    pub fn blocks(&self) -> String {
        let mut v = Vec::new();
        if self.slots.any() {
            v.push("slots");
        }
        if self.canvas.is_some() {
            v.push("canvas");
        }
        if self.scene.is_some() {
            v.push("scene");
        }
        if !self.rtti.is_empty() {
            v.push("rtti");
        }
        if self.los.is_some() {
            v.push("los");
        }
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(",")
        }
    }
}

// ----------------------------------------------------------- runtime binding

static PROFILE: OnceLock<Profile> = OnceLock::new();
static IMAGE_BASE: AtomicUsize = AtomicUsize::new(0);
/// Identity of the running image, captured once by [`init()`].
static IMAGE_SIZE: AtomicU32 = AtomicU32::new(0);
static PE_TIMESTAMP: AtomicU32 = AtomicU32::new(0);
static TEXT_FNV1A: AtomicU32 = AtomicU32::new(0);

/// The selected profile, or `None` in safe mode.
pub fn profile() -> Option<&'static Profile> {
    PROFILE.get()
}

/// Live module base, captured during init (falls back to the loader).
pub fn module_base() -> usize {
    let cached = IMAGE_BASE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}

pub fn run_image_size() -> u32 {
    IMAGE_SIZE.load(Ordering::Relaxed)
}

pub fn run_timestamp() -> u32 {
    PE_TIMESTAMP.load(Ordering::Relaxed)
}

/// Resolve a profile RVA against the live module base.
///
/// Returns `None` when no profile is selected, the RVA is zero/unset, or it
/// falls outside the validated image. Callers must treat `None` as "this
/// feature is unavailable", never as "use a default".
pub fn profile_addr(rva: u32) -> Option<usize> {
    if rva == 0 {
        return None;
    }
    let p = profile()?;
    let size = if p.image_size != 0 {
        p.image_size
    } else {
        run_image_size()
    };
    if rva < 0x1000 || rva.saturating_add(4) > size {
        return None;
    }
    let base = module_base();
    if base == 0 {
        return None;
    }
    Some(base + rva as usize)
}

/// A vtable RVA, usable where a `u32` vtable value is compared.
pub fn profile_vt(rva: u32) -> u32 {
    profile_addr(rva).map(|a| a as u32).unwrap_or(0)
}

pub fn slots() -> Option<&'static Slots> {
    profile().map(|p| &p.slots)
}

pub fn canvas() -> Option<&'static Canvas> {
    profile().and_then(|p| p.canvas.as_ref())
}

pub fn scene() -> Option<&'static Scene> {
    profile().and_then(|p| p.scene.as_ref())
}

pub fn los() -> Option<&'static Los> {
    profile().and_then(|p| p.los.as_ref())
}

/// The profile's scene inset, or the last-resort layout constant.
pub fn scene_inset_default() -> i32 {
    scene()
        .map(|s| s.top_inset)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SCENE_INSET)
}

// ------------------------------------------------------------------- parsing

/// Parse the profile format. Preserves case (unlike `config::apply_setting`,
/// which lowercases values and would destroy `GameView` and exe names).
pub fn parse(text: &str, source: &str) -> Result<Profile, String> {
    let mut p = Profile {
        exe: String::new(),
        build: "default".to_string(),
        status: Status::Experimental,
        preferred_base: 0x0040_0000,
        image_size: 0,
        pe_timestamp: 0,
        text_fnv1a: None,
        slots: Slots::default(),
        canvas: None,
        scene: None,
        rtti: Vec::new(),
        los: None,
        source: source.to_string(),
    };
    let mut section = String::new();
    let mut canvas = Canvas {
        app_ptr: 0,
        app_hwnd_off: 0,
        size_tolerance_pct: 0,
    };
    let mut scene = Scene {
        owner_ptr: 0,
        blur_quad_ptr: 0,
        widget_class: None,
        top_inset: 0,
        owner_widget_off: 0,
        widget_rect_off: 0,
        blur_quad_rect_off: 0,
        blur_quad_backlink_off: 0,
        blur_quad_enable_off: 0,
    };
    let mut los = Los::default();
    let mut have_canvas = false;
    let mut have_scene = false;
    let mut have_los = false;
    let mut unknown = Vec::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw
            .split(['#', ';'])
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            match section.as_str() {
                "canvas" => have_canvas = true,
                "scene" => have_scene = true,
                "los" => have_los = true,
                "profile" | "slots" | "rtti" => {}
                other => unknown.push(format!("section [{other}]")),
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected key = value", lineno + 1));
        };
        let key = key.trim();
        let value = value.trim();
        match section.as_str() {
            "profile" => match key.to_ascii_lowercase().as_str() {
                "exe" | "executable" => p.exe = value.to_string(),
                "build" | "name" => p.build = value.to_string(),
                "status" => p.status = Status::parse(value),
                "preferred_base" => p.preferred_base = num(value)?,
                "image_size" => p.image_size = num(value)?,
                "pe_timestamp" | "timestamp" => p.pe_timestamp = num(value)?,
                "text_fnv1a" | "text_hash" => {
                    let v = num(value)?;
                    p.text_fnv1a = if v == 0 { None } else { Some(v) };
                }
                _ => unknown.push(format!("{section}.{key}")),
            },
            "slots" => {
                let v = num(value)?;
                match key.to_ascii_lowercase().as_str() {
                    "close_window" => p.slots.close_window = v,
                    "get_system_metrics" | "metrics" => p.slots.get_system_metrics = v,
                    "change_display" | "change_display_settings_a" => p.slots.change_display = v,
                    "get_cursor_pos" => p.slots.get_cursor_pos = v,
                    _ => unknown.push(format!("{section}.{key}")),
                }
            }
            "canvas" => {
                let v = num(value)?;
                match key.to_ascii_lowercase().as_str() {
                    "app_ptr" => canvas.app_ptr = v,
                    "app_hwnd_off" => canvas.app_hwnd_off = v,
                    "size_tolerance_pct" => canvas.size_tolerance_pct = v.min(20),
                    _ => unknown.push(format!("{section}.{key}")),
                }
            }
            "scene" => {
                if key.eq_ignore_ascii_case("widget_class")
                    || key.eq_ignore_ascii_case("widget_rtti")
                {
                    let name = value.trim();
                    if name.is_empty() {
                        return Err("[scene] widget_class is empty".to_string());
                    }
                    if name.len() > 64 {
                        return Err("[scene] widget_class is too long".to_string());
                    }
                    scene.widget_class = Some(name.to_string());
                    continue;
                }
                let v = num(value)?;
                match key.to_ascii_lowercase().as_str() {
                    "owner_ptr" => scene.owner_ptr = v,
                    "blur_quad_ptr" => scene.blur_quad_ptr = v,
                    "top_inset" => scene.top_inset = v as i32,
                    "owner_widget_off" => scene.owner_widget_off = v,
                    "widget_rect_off" => scene.widget_rect_off = v,
                    "blur_quad_rect_off" => scene.blur_quad_rect_off = v,
                    "blur_quad_backlink_off" => scene.blur_quad_backlink_off = v,
                    "blur_quad_enable_off" => scene.blur_quad_enable_off = v,
                    _ => unknown.push(format!("{section}.{key}")),
                }
            }
            "rtti" => {
                let v = num(value)?;
                p.rtti.push((key.to_string(), v));
            }
            "los" => {
                if key.eq_ignore_ascii_case("map_globals") {
                    for part in value.split(',') {
                        let part = part.trim();
                        if part.is_empty() {
                            continue;
                        }
                        los.map_globals.push(num(part)?);
                    }
                    continue;
                }
                let v = num(value)?;
                match key.to_ascii_lowercase().as_str() {
                    "world_ptr" => los.world_ptr = v,
                    "scenario_ptr" => los.scenario_ptr = v,
                    "action_map_off" => los.action_map_off = v,
                    "vt_action_map" => los.vt_action_map = v,
                    "vt_terrain_map" => los.vt_terrain_map = v,
                    "vt_mapview" => los.vt_mapview = v,
                    "vt_camera" => los.vt_camera = v,
                    "vt_camera_persp" => los.vt_camera_persp = v,
                    "vt_unit" => los.vt_unit = v,
                    "height_off" => los.height_off = v,
                    "grid_w_off" => los.grid_w_off = v,
                    "grid_h_off" => los.grid_h_off = v,
                    "world_w_off" => los.world_w_off = v,
                    "world_h_off" => los.world_h_off = v,
                    _ => unknown.push(format!("{section}.{key}")),
                }
            }
            "" => return Err(format!("line {}: value before any [section]", lineno + 1)),
            _ => unknown.push(format!("{section}.{key}")),
        }
    }

    if p.exe.is_empty() {
        return Err("missing [profile] exe".to_string());
    }
    if p.image_size == 0 {
        return Err("missing [profile] image_size".to_string());
    }
    if p.pe_timestamp == 0 {
        return Err("missing [profile] pe_timestamp".to_string());
    }
    if have_canvas {
        if canvas.app_ptr == 0 {
            return Err("[canvas] has no app_ptr".to_string());
        }
        p.canvas = Some(canvas);
    }
    if have_scene {
        if scene.owner_ptr == 0 && scene.blur_quad_ptr == 0 && scene.widget_class.is_none() {
            return Err("[scene] has no owner_ptr, blur_quad_ptr or widget_class".to_string());
        }
        p.scene = Some(scene.with_defaults());
    }
    if have_los {
        if los.world_ptr == 0 && los.map_globals.is_empty() {
            return Err("[los] has neither world_ptr nor map_globals".to_string());
        }
        p.los = Some(los.with_defaults());
    }
    if !unknown.is_empty() {
        log(&format!(
            "profile: {}: ignoring unknown keys: {}",
            source,
            unknown.join(", ")
        ));
    }
    Ok(p)
}

fn num(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let (radix, digits) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, h)
    } else {
        (10, s)
    };
    u32::from_str_radix(digits, radix).map_err(|_| format!("invalid number '{s}'"))
}

/// Reject addresses that cannot belong to this image. Object offsets are only
/// bounded, not resolved.
pub fn validate_addresses(p: &Profile) -> Result<(), String> {
    let size = p.image_size;
    let check = |what: &str, rva: u32| -> Result<(), String> {
        if rva == 0 {
            return Ok(());
        }
        if rva < 0x1000 || rva.saturating_add(4) > size {
            return Err(format!(
                "{what} rva 0x{rva:x} outside the image (size 0x{size:x})"
            ));
        }
        Ok(())
    };
    check("slots.close_window", p.slots.close_window)?;
    check("slots.get_system_metrics", p.slots.get_system_metrics)?;
    check("slots.change_display", p.slots.change_display)?;
    check("slots.get_cursor_pos", p.slots.get_cursor_pos)?;
    if let Some(c) = &p.canvas {
        check("canvas.app_ptr", c.app_ptr)?;
        if c.app_hwnd_off > 0x10000 {
            return Err(format!(
                "canvas.app_hwnd_off 0x{:x} is too large",
                c.app_hwnd_off
            ));
        }
        if c.size_tolerance_pct > 20 {
            return Err("canvas.size_tolerance_pct must be 0..=20".to_string());
        }
    }
    if let Some(s) = &p.scene {
        check("scene.owner_ptr", s.owner_ptr)?;
        check("scene.blur_quad_ptr", s.blur_quad_ptr)?;
        if let Some(name) = &s.widget_class
            && (name.trim().is_empty() || name.len() > 64)
        {
            return Err("[scene] widget_class is empty or too long".to_string());
        }
        for (what, off) in [
            ("owner_widget_off", s.owner_widget_off),
            ("widget_rect_off", s.widget_rect_off),
            ("blur_quad_rect_off", s.blur_quad_rect_off),
            ("blur_quad_backlink_off", s.blur_quad_backlink_off),
            ("blur_quad_enable_off", s.blur_quad_enable_off),
        ] {
            if off > 0x10000 {
                return Err(format!("scene.{what} 0x{off:x} is too large"));
            }
        }
    }
    for (name, rva) in &p.rtti {
        check(&format!("rtti.{name}"), *rva)?;
    }
    if let Some(l) = &p.los {
        for (what, rva) in [
            ("world_ptr", l.world_ptr),
            ("scenario_ptr", l.scenario_ptr),
            ("vt_action_map", l.vt_action_map),
            ("vt_terrain_map", l.vt_terrain_map),
            ("vt_mapview", l.vt_mapview),
            ("vt_camera", l.vt_camera),
            ("vt_camera_persp", l.vt_camera_persp),
            ("vt_unit", l.vt_unit),
        ] {
            check(&format!("los.{what}"), rva)?;
        }
        for g in &l.map_globals {
            check("los.map_globals", *g)?;
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- discovery

/// Basename of the main module, e.g. `CM Cold War.exe`.
pub fn discover_exe() -> String {
    unsafe {
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 || n as usize >= buf.len() {
            return String::new();
        }
        let path = str_from_wide(buf.as_ptr());
        path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string()
    }
}

struct Candidate {
    file: Option<PathBuf>,
    /// A file found by folder discovery must agree with the folder name.
    /// An explicitly selected file does not (it may be a scratch copy).
    require_name_match: bool,
}

fn profiles_dir() -> PathBuf {
    exe_dir().join(PROFILE_DIR)
}

fn ini_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            ini_files(&p, out);
        } else if p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("ini"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

/// Resolve an explicit `profile=` value into candidates.
fn explicit_candidates(value: &str) -> Vec<Candidate> {
    let value = value.trim();
    let mut out = Vec::new();
    if value.is_empty() {
        return out;
    }
    let make = |p: PathBuf| Candidate {
        file: Some(p),
        require_name_match: false,
    };
    if value.contains(['\\', '/']) || value.to_ascii_lowercase().ends_with(".ini") {
        let p = PathBuf::from(value);
        let p = if p.is_absolute() || p.exists() {
            p
        } else {
            exe_dir().join(p)
        };
        out.push(make(p));
        return out;
    }
    // Bare name: profiles/<name>/ first, then profiles/<name>.ini.
    let base = profiles_dir();
    let dir = base.join(value);
    if dir.is_dir() {
        let mut files = Vec::new();
        ini_files(&dir, &mut files);
        files.sort();
        out.extend(files.into_iter().map(make));
        return out;
    }
    let single = base.join(format!("{value}.ini"));
    if single.exists() {
        out.push(make(single));
    } else {
        out.push(make(dir.join("profile.ini")));
    }
    out
}

/// Everything that could describe the running exe, in discovery order.
fn candidates(cfg: &Config, exe: &str) -> Vec<Candidate> {
    let discovered = |path: PathBuf| Candidate {
        file: Some(path),
        require_name_match: true,
    };
    if let Some(sel) = cfg.profile.as_deref() {
        if sel.eq_ignore_ascii_case("none") || sel.eq_ignore_ascii_case("safe") {
            log("profile: safe mode requested by profile=none");
            return Vec::new();
        }
        let explicit = explicit_candidates(sel);
        if !explicit.is_empty() {
            log(&format!("profile: explicit selection '{sel}'"));
            return explicit;
        }
    }

    let mut out = Vec::new();
    let dir = profiles_dir().join(exe);
    if dir.is_dir() {
        let mut files = Vec::new();
        ini_files(&dir, &mut files);
        files.sort();
        let single = profiles_dir().join(format!("{exe}.ini"));
        if single.exists() {
            log(&format!(
                "profile: ignoring {} (directory {} takes precedence)",
                single.display(),
                dir.display()
            ));
        }
        out.extend(files.into_iter().map(discovered));
    } else {
        let single = profiles_dir().join(format!("{exe}.ini"));
        if single.exists() {
            out.push(discovered(single));
        }
    }
    if out.is_empty() {
        // Renamed exe: sweep every profile by identity.
        let mut files = Vec::new();
        ini_files(&profiles_dir(), &mut files);
        files.sort();
        out.extend(files.into_iter().map(discovered));
    }
    out
}

fn candidate_owner(c: &Candidate, p: &Profile) -> Result<(), String> {
    if !c.require_name_match {
        return Ok(());
    }
    // The exe field must agree with the folder (or file) it was found in. This
    // catches copy-paste and "dropped in the wrong folder" mistakes.
    let expected = if let Some(path) = &c.file {
        let parent = path.parent().unwrap_or(Path::new(""));
        let parent_name = parent
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if parent_name.eq_ignore_ascii_case(PROFILE_DIR) {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            parent_name
        }
    } else {
        String::new()
    };
    if !expected.is_empty() && !p.exe.eq_ignore_ascii_case(&expected) {
        return Err(format!(
            "exe field '{}' does not match '{}'",
            p.exe, expected
        ));
    }
    Ok(())
}

/// Two candidates for the same build with the same facts are the same profile
/// (e.g. the shipped `profiles/` copy and the embedded copy). They must not
/// make selection ambiguous.
fn same_facts(a: &Profile, b: &Profile) -> bool {
    a.exe.eq_ignore_ascii_case(&b.exe)
        && a.build.eq_ignore_ascii_case(&b.build)
        && a.status == b.status
        && a.image_size == b.image_size
        && a.pe_timestamp == b.pe_timestamp
        && a.text_fnv1a == b.text_fnv1a
        && a.slots.close_window == b.slots.close_window
        && a.slots.get_system_metrics == b.slots.get_system_metrics
        && a.slots.change_display == b.slots.change_display
        && a.slots.get_cursor_pos == b.slots.get_cursor_pos
        && a.canvas.map(|c| (c.app_ptr, c.app_hwnd_off))
            == b.canvas.map(|c| (c.app_ptr, c.app_hwnd_off))
        && a.scene.as_ref().map(|s| {
            (
                s.owner_ptr,
                s.blur_quad_ptr,
                s.top_inset,
                s.widget_class.as_deref(),
            )
        }) == b.scene.as_ref().map(|s| {
            (
                s.owner_ptr,
                s.blur_quad_ptr,
                s.top_inset,
                s.widget_class.as_deref(),
            )
        })
        && a.rtti == b.rtti
        && a.los.is_some() == b.los.is_some()
}

/// Outcome of profile selection.
// A `Profile` is a few hundred bytes, but selection runs once at startup before
// any hook is installed; boxing would only add indirection to that single call.
#[allow(clippy::large_enum_variant)]
pub enum Selection {
    Matched(Profile),
    SafeMode(String),
}

/// Discovery + identity match. Called once from `init` before any hook runs.
pub unsafe fn load(cfg: &Config) -> Selection {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        IMAGE_BASE.store(base, Ordering::Relaxed);
        let size = module_image_size(base) as u32;
        let ts = pe_timestamp(base);
        IMAGE_SIZE.store(size, Ordering::Relaxed);
        PE_TIMESTAMP.store(ts, Ordering::Relaxed);
        let hash = module_text_fnv1a(base);
        TEXT_FNV1A.store(hash.unwrap_or(0), Ordering::Relaxed);

        let exe = discover_exe();
        let mut parsed: Vec<(Candidate, Profile)> = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        for c in candidates(cfg, &exe) {
            let Some(path) = &c.file else { continue };
            let (source, text) = match std::fs::read_to_string(path) {
                Ok(t) => (format!("file:{}", path.display()), t),
                Err(e) => {
                    rejected.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            match parse(&text, &source) {
                Ok(p) => {
                    if let Err(reason) = validate_addresses(&p) {
                        rejected.push(format!("{source}: {reason}"));
                        continue;
                    }
                    if let Err(reason) = candidate_owner(&c, &p) {
                        rejected.push(format!("{source}: {reason}"));
                        continue;
                    }
                    parsed.push((c, p));
                }
                Err(e) => rejected.push(format!("{source}: {e}")),
            }
        }
        for r in &rejected {
            log(&format!("profile: rejected {r}"));
        }

        let mut unique: Vec<(Candidate, Profile)> = Vec::new();
        for (c, p) in parsed.into_iter().filter(|(_, p)| {
            p.image_size == size
                && p.pe_timestamp == ts
                && p.text_fnv1a
                    .map(|want| hash.map(|got| got == want).unwrap_or(false))
                    .unwrap_or(true)
        }) {
            if unique.iter().any(|(_, q)| same_facts(q, &p)) {
                continue;
            }
            unique.push((c, p));
        }

        if unique.len() == 1 {
            let (_cand, p) = unique.into_iter().next().unwrap();
            if !p.exe.eq_ignore_ascii_case(&exe) {
                log(&format!(
                    "profile: matched by build identity; exe name differs; rename '{}' -> '{}'",
                    p.exe, exe
                ));
            }
            if PROFILE.set(p.clone()).is_ok() {
                PROFILE_OK.store(1, Ordering::Relaxed);
            }
            Selection::Matched(p)
        } else if unique.len() > 1 {
            let names: Vec<String> = unique.iter().map(|(_, p)| p.source.clone()).collect();
            Selection::SafeMode(format!(
                "ambiguous: {} different profiles match this build ({})",
                names.len(),
                names.join(", ")
            ))
        } else {
            Selection::SafeMode(format!(
                "no profile matches exe='{exe}' image=0x{size:x} ts=0x{ts:x}{}",
                hash.map(|h| format!(" text=0x{h:x}")).unwrap_or_default()
            ))
        }
    }
}

/// UTF-8 FNV-1a over the mapped `.text` (see `engine::module_text_fnv1a`).
pub fn text_hash() -> Option<u32> {
    match TEXT_FNV1A.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v),
    }
}

static RETRIES: AtomicU32 = AtomicU32::new(0);

/// Retry selection for a packed executable that exposes its real image after
/// `DllMain` has run. Bounded, silent on failure: engine writes stay gated by
/// `PROFILE_OK`, so a late match is safe.
pub unsafe fn retry(cfg: &Config) -> bool {
    unsafe {
        if profile().is_some() {
            return true;
        }
        if RETRIES.fetch_add(1, Ordering::Relaxed) >= 20 {
            return false;
        }
        if let Selection::Matched(p) = load(cfg) {
            log(&format!(
                "profile: matched after startup (late image): exe={} build={} source={}",
                p.exe, p.build, p.source
            ));
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = "\
[profile]
exe = CM Cold War.exe
build = gog-32
status = partial
preferred_base = 0x00400000
image_size = 0x00b2b000
pe_timestamp = 0x68d02464
text_fnv1a = 0xca318fe2

[slots]
get_system_metrics = 0x005d5400

[canvas]
app_ptr = 0x009764b4
app_hwnd_off = 0xbac

[rtti]
GameView = 0x00826340
";

    #[test]
    fn parses_all_blocks() {
        let p = parse(MINI, "test").unwrap();
        assert_eq!(p.exe, "CM Cold War.exe");
        assert_eq!(p.build, "gog-32");
        assert_eq!(p.status, Status::Partial);
        assert_eq!(p.image_size, 0xb2b000);
        assert_eq!(p.pe_timestamp, 0x68d02464);
        assert_eq!(p.text_fnv1a, Some(0xca318fe2));
        assert_eq!(p.slots.get_system_metrics, 0x5d5400);
        assert_eq!(p.canvas.unwrap().app_hwnd_off, 0xbac);
        assert_eq!(p.rtti_rva("gameview"), Some(0x826340));
        assert!(p.los.is_none() && p.scene.is_none());
        validate_addresses(&p).unwrap();
    }

    #[test]
    fn scene_can_be_resolved_by_widget_class() {
        // Cold War has no stable scene-owner global; the battle GameView is
        // reachable only through the live widget tree.
        let text = format!("{MINI}\n[scene]\nwidget_class = GameView\ntop_inset = 155\n");
        let p = parse(&text, "test").unwrap();
        let scene = p.scene.as_ref().unwrap();
        assert_eq!(scene.owner_ptr, 0);
        assert_eq!(scene.blur_quad_ptr, 0);
        assert_eq!(scene.widget_class.as_deref(), Some("GameView"));
        assert_eq!(scene.top_inset, 155);
        // Offsets still get their family defaults.
        assert_eq!(scene.owner_widget_off, 0x20);
        assert_eq!(scene.widget_rect_off, 0x10);
        validate_addresses(&p).unwrap();
    }

    #[test]
    fn empty_scene_is_rejected() {
        let text = format!("{MINI}\n[scene]\ntop_inset = 155\n");
        let e = parse(&text, "test").unwrap_err();
        assert!(e.contains("widget_class"), "{e}");
    }

    #[test]
    fn shipped_cmbs_profile_is_a_normal_file_profile() {
        // The DLL no longer embeds CMBS data; the profile ships as a file like
        // every other title.
        let rel = format!("{}/CM Black Sea.exe/steam-32.ini", PROFILE_DIR);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let p = parse(&text, "file").unwrap();
        assert_eq!(p.exe, "CM Black Sea.exe");
        assert_eq!(p.image_size, 0xa5f000);
        assert_eq!(p.pe_timestamp, 0x63d82c90);
        assert_eq!(p.slots.get_system_metrics, 0x5d5400);
        assert_eq!(p.canvas.unwrap().app_ptr, 0x9764b4);
        let scene = p.scene.as_ref().unwrap();
        assert_eq!(scene.owner_ptr, 0x97a6b4);
        assert_eq!(scene.blur_quad_ptr, 0x97a6c8);
        assert_eq!(scene.top_inset, 155);
        assert_eq!(p.rtti_rva("View"), Some(0x7ac7b4));
        assert_eq!(p.rtti_rva("CommandButtonPanel"), Some(0x7992d8));
        assert!(p.los.is_some());
        validate_addresses(&p).unwrap();
    }

    #[test]
    fn no_profile_means_safe_mode() {
        let cfg = Config {
            profile: Some("none".to_string()),
            ..Config::default()
        };
        assert!(
            candidates(&cfg, "CM Black Sea.exe").is_empty(),
            "profile=none must select no candidate"
        );
    }

    #[test]
    fn explicit_file_profile_selects() {
        let rel = format!("{}/CM Black Sea.exe/steam-32.ini", PROFILE_DIR);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
        let cfg = Config {
            profile: Some(path.to_string_lossy().into_owned()),
            ..Config::default()
        };
        let list = candidates(&cfg, "CM Black Sea.exe");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].file.as_deref(), Some(path.as_path()));
        assert!(!list[0].require_name_match);
    }

    #[test]
    fn missing_required_key_is_rejected() {
        let e = parse("[profile]\nexe = X.exe\n", "t").unwrap_err();
        assert!(e.contains("image_size"), "{e}");
    }

    #[test]
    fn address_outside_the_image_is_rejected() {
        let text = MINI.replace("app_ptr = 0x009764b4", "app_ptr = 0x00c00000");
        let p = parse(&text, "t").unwrap();
        let e = validate_addresses(&p).unwrap_err();
        assert!(e.contains("outside the image"), "{e}");
    }

    #[test]
    fn offsets_are_not_rvas() {
        let p = parse(MINI, "t").unwrap();
        // 0xbac is an object offset inside the image range and stays accepted;
        // a 0x20000 offset must be refused.
        let text = MINI.replace("app_hwnd_off = 0xbac", "app_hwnd_off = 0x20000");
        let p2 = parse(&text, "t").unwrap();
        assert!(p.canvas.is_some());
        assert!(validate_addresses(&p2).is_err());
    }

    #[test]
    fn case_is_preserved_for_class_names() {
        let p = parse(MINI, "t").unwrap();
        assert_eq!(p.rtti[0].0, "GameView");
    }

    #[test]
    fn status_parsing() {
        assert_eq!(Status::parse("VERIFIED"), Status::Verified);
        assert_eq!(Status::parse("partial"), Status::Partial);
        assert_eq!(Status::parse("whatever"), Status::Experimental);
    }

    #[test]
    fn identical_copies_are_not_ambiguous() {
        // A shipped profiles/ copy and the embedded copy of the same build must
        // count as one profile, not two.
        let a = parse(MINI, "a").unwrap();
        let b = parse(MINI, "b").unwrap();
        assert!(same_facts(&a, &b));
        let c = parse(
            &MINI.replace("GameView = 0x00826340", "GameView = 0x00826344"),
            "c",
        )
        .unwrap();
        assert!(!same_facts(&a, &c));
    }

    #[test]
    fn shipped_profiles_parse_and_validate() {
        let profiles = PROFILE_DIR.to_string();
        for rel in [
            format!("{profiles}/CM Black Sea.exe/steam-32.ini"),
            format!("{profiles}/CM Shock Force 2.exe/gog-32.ini"),
            format!("{profiles}/CM Cold War.exe/gog-32.ini"),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let p = parse(&text, &rel).unwrap_or_else(|e| panic!("{rel}: {e}"));
            validate_addresses(&p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            // A profile may carry anchors and still be `partial` until they are
            // validated in a live battle. The reverse is not allowed: a
            // `verified` profile must have both anchors.
            if p.status == Status::Verified {
                assert!(p.canvas.is_some(), "{rel}: verified without [canvas]");
                assert!(p.scene.is_some(), "{rel}: verified without [scene]");
            }
            assert!(!p.rtti.is_empty(), "{rel}: no rtti block");
        }
    }
}
