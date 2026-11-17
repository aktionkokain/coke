//! Configuration file parsing and the resolved `Config` value.

use crate::*;
use std::sync::OnceLock;

// ---- code ----
// --------------------------------------------------------------- configuration

#[derive(Clone)]
pub struct Config {
    /// Start borderless fullscreen instead of a framed window.
    pub start_fullscreen: bool,
    /// Logical resolution. 0 means "the monitor's native resolution".
    pub width: i32,
    pub height: i32,
    /// Framed window size as a percentage of the logical resolution.
    pub window_scale: u32,
    /// Pixels shaved off the borderless fullscreen window so Windows does not
    /// classify it as a fullscreen app. 0 = cover the monitor exactly.
    pub fullscreen_inset: i32,
    /// Edge the inset is taken from: 0 top, 1 bottom, 2 left, 3 right,
    /// 4 right+bottom (legacy behaviour).
    pub fullscreen_inset_edge: u8,
    /// 1 = fill the window (may distort), 0 = keep the aspect ratio.
    pub stretch: bool,
    /// Keep the last frame on focus loss instead of minimising.
    pub fix_alt_tab: bool,
    /// 0 = uncapped.
    pub limit_fps: u32,
    /// Force wglSwapIntervalEXT to this value; `None` leaves the game alone.
    pub vsync: Option<i32>,
    /// Raise the engine's IDLE-priority worker threads to BELOW_NORMAL.
    pub boost_workers: bool,
    /// Process priority class: 0 idle .. 4 high. 255 = leave alone.
    pub priority: u32,
    /// Process affinity mask. 0 = leave alone (all cores).
    pub affinity: usize,
    /// Apply the logical -> physical mapping (set 0 to debug layout issues).
    pub scale_fixes: bool,
    /// Promote the in-game scene subviewport to the full canvas. Only correct
    /// when the engine's canvas already fills the logical screen; if the engine
    /// blits a smaller canvas (game resolution < logical resolution) this
    /// stretches the picture without moving the engine's hit-testing, so unit
    /// picking breaks. Default off.
    pub promote_scene: bool,
    /// Patch the engine's user32 IAT slots (screen size, cursor, display mode).
    pub hook_slots: bool,
    /// Write the new client size into the engine's cached canvas after a resize.
    /// Must be off when `width`/`height` pin a fixed canvas.
    pub canvas_patch: bool,
    /// Expand a stale full-screen viewport that still uses the startup canvas
    /// (e.g. `(0,0,1920,1080)` after a resize to 2560x1439).
    pub stale_viewport_fix: bool,
    /// Restore the battle scene's top inset when the engine draws it at `y=0`
    /// after the canvas grows. `(0,0,w,h-155)` -> `(0,155,w,h-155)`.
    pub scene_rect_fix: bool,
    /// Legacy fallback for `scene_rect_fix`: rewrite the *drawn* viewport
    /// instead of the engine's rects. Breaks click-to-unit picking (the engine
    /// picks with its own un-rewritten rect), so default off.
    pub scene_vp_rewrite: bool,
    /// Re-center widget rects that were laid out for the startup or previous
    /// canvas after the canvas changes (menu panel, dialogs). Fixes stale
    /// screens in both directions because it repairs the engine's own hit
    /// testing, not just the viewport.
    pub menu_recenter: bool,
    /// Top inset for the battle scene. 0 = learn it at runtime from a battle
    /// frame drawn at the startup canvas (falls back to 155).
    pub scene_inset: i32,
    /// Debug: dump the engine's live widget tree (read-only) after each canvas
    /// change, on the first battle frame, and on F9. Temporary diagnostic.
    pub widget_dump: bool,
    /// Native re-layout after a canvas change: replay the engine's widget
    /// rects against the new canvas. 0 = off, 1 = apply, 2 = dry-run (log the
    /// planned rects only).
    pub widget_relayout: u8,
    /// Milliseconds between the canvas change and the re-layout pass. Small
    /// values make the resize snap near-instantly; the engine does not rebuild
    /// a screen on resize, so there is nothing to wait for.
    pub relayout_delay_ms: u32,
    /// Install the WH_CBT create-window hook on the engine's threads.
    pub hook_cbt: bool,
    /// Discover/hook the main window, apply start mode and F11.
    pub hook_window: bool,
    /// Subclass the game's window procedure (maximise/close/mouse mapping).
    pub hook_wndproc: bool,
    /// Apply the start window size and F11 mode changes.
    pub hook_mode: bool,
    /// Log the GL renderer, viewports, fps and window geometry.
    pub verbose: bool,
    /// Draw an FPS counter overlay in the top-right corner of the window.
    pub show_fps: bool,
    /// F-key number (1..=12) that toggles the FPS counter at runtime. `0`
    /// disables the hotkey. See the key map in `docs/ai/CONFIG.md`.
    pub fps_hotkey: u8,
    /// cokelos: flip the projected X axis (camera handedness differs between
    /// screenshots). 0 = normal.
    pub los_flip_x: bool,
    /// cokelos: flip the projected Y axis.
    pub los_flip_y: bool,
    /// cokelos: project using the scene widget rect instead of the full window.
    /// The engine's 3D viewport is the whole window on this build, so the
    /// default is false; enable if the overlay is vertically offset.
    pub los_scene_rect: bool,
    /// cokelos: percent of the window height at the bottom that is HUD, so the
    /// overlay is clipped instead of drawing over the command panel.
    pub los_clip_bottom_pct: u32,
    /// Explicit profile selector (`profile=<name-or-path>`). `None` = discover
    /// by exe name; `Some("none")` forces safe mode. Case is preserved.
    pub profile: Option<String>,
    /// Log the full build identity, slot RVAs and resolved RTTI vtables once.
    /// Turns adding a profile for a new build into "launch once, copy numbers".
    pub profile_dump: bool,
    /// Debug: write the live main module to `analysis/<exe>-image-pe.bin` once,
    /// after the packed image has unpacked. For decompiling, not for play.
    pub dump_module: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            start_fullscreen: false,
            width: 0,
            height: 0,
            window_scale: 75,
            fullscreen_inset: 1,
            fullscreen_inset_edge: 0,
            stretch: true,
            fix_alt_tab: true,
            limit_fps: 0,
            vsync: None,
            boost_workers: false,
            priority: 255,
            affinity: 0,
            scale_fixes: true,
            promote_scene: false,
            hook_slots: true,
            canvas_patch: true,
            stale_viewport_fix: true,
            scene_rect_fix: true,
            scene_vp_rewrite: false,
            menu_recenter: true,
            scene_inset: 0,
            widget_dump: true,
            widget_relayout: 1,
            relayout_delay_ms: 30,
            hook_cbt: true,
            hook_window: true,
            hook_wndproc: true,
            hook_mode: true,
            verbose: true,
            show_fps: true,
            fps_hotkey: 10,
            los_flip_x: false,
            los_flip_y: false,
            los_scene_rect: false,
            los_clip_bottom_pct: 17,
            profile: None,
            profile_dump: false,
            dump_module: false,
        }
    }
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

// ---------------------------------------------------------------- config file

/// Reads the config file next to the executable. The names come from
/// `names.ini` (`CONFIG_FILE` first, then any `LEGACY_CONFIG_FILES`), so an
/// existing install keeps working after an upgrade.
pub fn load_config() -> Config {
    let mut cfg = Config::default();
    let dir = exe_dir();
    let path = core::iter::once(CONFIG_FILE)
        .chain(LEGACY_CONFIG_FILES.iter().copied())
        .map(|name| dir.join(name))
        .find(|p| p.exists());
    let Some(path) = path else { return cfg };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            // Tolerate `key value` too.
            let mut it = line.split_whitespace();
            let (Some(k), Some(v)) = (it.next(), it.next()) else {
                continue;
            };
            apply_setting(&mut cfg, k, v);
            continue;
        };
        apply_setting(&mut cfg, key, value);
    }
    cfg
}

pub fn apply_setting(cfg: &mut Config, key: &str, value: &str) {
    let key = key.trim().to_ascii_lowercase();
    // Profile selectors are paths/names: keep their case.
    let raw_value = value.trim().to_string();
    let value = value.trim().to_ascii_lowercase();
    let on = matches!(value.as_str(), "1" | "true" | "yes" | "on");
    match key.as_str() {
        // ---- display
        "fullscreen" | "fullscreen_at_start" => cfg.start_fullscreen = on,
        "windowed" => cfg.start_fullscreen = !on, // old key
        "width" | "resolution_x" => {
            if let Ok(v) = value.parse::<i32>() {
                cfg.width = v;
            }
        }
        "height" | "resolution_y" => {
            if let Ok(v) = value.parse::<i32>() {
                cfg.height = v;
            }
        }
        "window_scale" | "window_size" | "windowed_scale" => {
            if let Ok(v) = value.trim_end_matches('%').parse::<u32>()
                && (25..=100).contains(&v)
            {
                cfg.window_scale = v;
            }
        }
        "fullscreen_inset" | "fullscreen_border" => {
            if let Ok(v) = value.parse::<i32>() {
                cfg.fullscreen_inset = v.clamp(0, 16);
            }
        }
        "fullscreen_inset_edge" | "fullscreen_inset_side" | "fullscreen_border_edge" => {
            cfg.fullscreen_inset_edge = match value.as_str() {
                "top" => 0,
                "bottom" => 1,
                "left" => 2,
                "right" => 3,
                "both" | "right_bottom" | "bottom_right" => 4,
                _ => cfg.fullscreen_inset_edge,
            };
        }
        "stretch" | "fill" => cfg.stretch = on,
        "keep_aspect" => cfg.stretch = !on,
        // ---- performance
        "limit_fps" | "fps" | "fps_limit" | "max_fps" => {
            if let Ok(v) = value.parse::<u32>() {
                cfg.limit_fps = v;
            }
        }
        "vsync" => {
            cfg.vsync = match value.as_str() {
                "on" | "1" | "true" | "yes" => Some(1),
                "off" | "0" | "false" | "no" => Some(0),
                _ => None,
            }
        }
        "priority" | "process_priority" => {
            cfg.priority = match value.as_str() {
                "idle" => 0,
                "below_normal" | "below" => 1,
                "normal" => 2,
                "above_normal" | "above" => 3,
                "high" => 4,
                _ => 255,
            }
        }
        "affinity" | "cpu_affinity" => {
            cfg.affinity = parse_affinity(&value);
        }
        "boost_workers" | "boost_threads" => cfg.boost_workers = on,
        // ---- compat / debug
        "fix_alt_tab" => cfg.fix_alt_tab = on,
        "scale_fixes" | "scale" => cfg.scale_fixes = on,
        "promote_scene" | "promote" | "scene_promote" => cfg.promote_scene = on,
        "hook_slots" => cfg.hook_slots = on,
        "canvas_patch" | "engine_canvas" | "patch_canvas" => cfg.canvas_patch = on,
        "stale_viewport_fix" | "fix_stale_viewport" => cfg.stale_viewport_fix = on,
        "scene_rect_fix" | "fix_scene_rect" | "scene_offset_fix" => cfg.scene_rect_fix = on,
        "scene_vp_rewrite" | "rewrite_scene_vp" => cfg.scene_vp_rewrite = on,
        "menu_recenter" | "recenter_ui" | "widget_recenter" => cfg.menu_recenter = on,
        "scene_inset" | "scene_top_inset" => {
            if let Ok(v) = value.parse::<i32>() {
                cfg.scene_inset = v.clamp(0, 480);
            }
        }
        "widget_dump" | "dump_widgets" => cfg.widget_dump = on,
        "widget_relayout" | "relayout_ui" | "native_relayout" => {
            cfg.widget_relayout = match value.as_str() {
                "0" | "off" | "false" | "no" => 0,
                "2" | "dry" | "dry_run" | "dry-run" => 2,
                _ => 1,
            };
        }
        "relayout_delay_ms" | "relayout_delay" => {
            if let Ok(v) = value.parse::<u32>() {
                cfg.relayout_delay_ms = v.min(2000);
            }
        }
        "hook_cbt" | "cbt_hook" => cfg.hook_cbt = on,
        "hook_window" => cfg.hook_window = on,
        "hook_wndproc" => cfg.hook_wndproc = on,
        "hook_mode" => cfg.hook_mode = on,
        "verbose" => cfg.verbose = on,
        "show_fps" | "fps_overlay" | "fps_counter" => cfg.show_fps = on,
        "fps_hotkey" | "fps_key" | "fps_toggle" => {
            if let Some(n) = parse_hotkey(&value) {
                cfg.fps_hotkey = n;
            }
        }
        "los_flip_x" | "cokelos_flip_x" => cfg.los_flip_x = on,
        "los_flip_y" | "cokelos_flip_y" => cfg.los_flip_y = on,
        "los_scene_rect" | "cokelos_scene_rect" => cfg.los_scene_rect = on,
        "los_clip_bottom" | "los_clip_bottom_pct" => {
            if let Ok(x) = value.parse::<u32>() {
                cfg.los_clip_bottom_pct = x.min(40);
            }
        }
        // ---- profiles / diagnostics
        "profile" | "build_profile" | "game_profile" => {
            if !raw_value.is_empty() {
                cfg.profile = Some(raw_value);
            }
        }
        "profile_dump" | "dump_profile" => cfg.profile_dump = on,
        "dump_module" | "dump_image" => cfg.dump_module = on,
        _ => {}
    }
}

/// `fps_hotkey=f10`, `fps_hotkey=10` or `fps_hotkey=off`. Returns the F-key
/// number (1..=12), or `0` for an explicit disable (`off`/`none`/`0`).
pub fn parse_hotkey(value: &str) -> Option<u8> {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    match v.as_str() {
        "off" | "none" | "no" | "0" => return Some(0),
        _ => {}
    }
    let digits = v.strip_prefix('f').unwrap_or(v.as_str());
    let n = digits.parse::<u8>().ok()?;
    (1..=12).contains(&n).then_some(n)
}

/// `affinity=all`, `affinity=0x3`, `affinity=3` or `affinity=0-3,6`.
pub fn parse_affinity(value: &str) -> usize {
    if value.is_empty() || value == "all" || value == "*" {
        return 0;
    }
    let mut mask = 0usize;
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-')
            && let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>())
        {
            for i in a..=b.min(63) {
                mask |= 1usize << i;
            }
            continue;
        }
        if let Some(hex) = part.strip_prefix("0x")
            && let Ok(v) = usize::from_str_radix(hex, 16)
        {
            mask |= v;
            continue;
        }
        if let Ok(v) = part.parse::<u32>() {
            // A bare number is an index, a big number is a mask.
            if v < 64 {
                mask |= 1usize << v;
            } else {
                mask |= v as usize;
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_and_bare_values() {
        let mut cfg = Config::default();
        apply_setting(&mut cfg, "width", " 1280 ");
        apply_setting(&mut cfg, "height", "720");
        apply_setting(&mut cfg, "fullscreen", "yes");
        apply_setting(&mut cfg, "stretch", "0");
        apply_setting(&mut cfg, "priority", "high");
        apply_setting(&mut cfg, "vsync", "off");
        assert_eq!((cfg.width, cfg.height), (1280, 720));
        assert!(cfg.start_fullscreen);
        assert!(!cfg.stretch);
        assert_eq!(cfg.priority, 4);
        assert_eq!(cfg.vsync, Some(0));
    }

    #[test]
    fn windowed_is_the_inverse_of_fullscreen() {
        let mut cfg = Config::default();
        apply_setting(&mut cfg, "windowed", "1");
        assert!(!cfg.start_fullscreen);
        apply_setting(&mut cfg, "windowed", "0");
        assert!(cfg.start_fullscreen);
    }

    #[test]
    fn clamp_and_range_checks() {
        let mut cfg = Config::default();
        apply_setting(&mut cfg, "window_scale", "5");
        assert_eq!(cfg.window_scale, 75, "out-of-range values are ignored");
        apply_setting(&mut cfg, "window_scale", "90%");
        assert_eq!(cfg.window_scale, 90);
        apply_setting(&mut cfg, "fullscreen_inset", "999");
        assert_eq!(cfg.fullscreen_inset, 16);
        apply_setting(&mut cfg, "relayout_delay_ms", "99999");
        assert_eq!(cfg.relayout_delay_ms, 2000);
        apply_setting(&mut cfg, "los_clip_bottom_pct", "99");
        assert_eq!(cfg.los_clip_bottom_pct, 40);
    }

    #[test]
    fn relayout_modes_are_distinct() {
        let mut cfg = Config::default();
        apply_setting(&mut cfg, "widget_relayout", "0");
        assert_eq!(cfg.widget_relayout, 0);
        apply_setting(&mut cfg, "widget_relayout", "2");
        assert_eq!(cfg.widget_relayout, 2);
        apply_setting(&mut cfg, "widget_relayout", "1");
        assert_eq!(cfg.widget_relayout, 1);
    }

    #[test]
    fn affinity_forms() {
        assert_eq!(parse_affinity("all"), 0);
        assert_eq!(parse_affinity(""), 0);
        assert_eq!(parse_affinity("0x3"), 0b11);
        assert_eq!(parse_affinity("2"), 0b100);
        assert_eq!(parse_affinity("0-2"), 0b111);
        assert_eq!(parse_affinity("0,3"), 0b1001);
        assert_eq!(parse_affinity("0x10000"), 0x10000);
    }

    #[test]
    fn fps_hotkey_forms() {
        assert_eq!(parse_hotkey("f10"), Some(10));
        assert_eq!(parse_hotkey("F1"), Some(1));
        assert_eq!(parse_hotkey("12"), Some(12));
        assert_eq!(parse_hotkey("off"), Some(0));
        assert_eq!(parse_hotkey("0"), Some(0));
        assert_eq!(parse_hotkey("13"), None);
        assert_eq!(parse_hotkey("f13"), None);
        assert_eq!(parse_hotkey("banana"), None);

        let mut cfg = Config::default();
        apply_setting(&mut cfg, "fps_hotkey", "F3");
        assert_eq!(cfg.fps_hotkey, 3);
        apply_setting(&mut cfg, "fps_hotkey", "off");
        assert_eq!(cfg.fps_hotkey, 0);
        // An invalid value leaves the previous one alone.
        apply_setting(&mut cfg, "fps_hotkey", "F99");
        assert_eq!(cfg.fps_hotkey, 0);
    }

    #[test]
    fn unknown_keys_do_not_change_anything() {
        let before = Config::default();
        let mut cfg = Config::default();
        apply_setting(&mut cfg, "not_a_real_setting", "1");
        assert_eq!(cfg.width, before.width);
        assert_eq!(cfg.show_fps, before.show_fps);
        assert_eq!(cfg.widget_relayout, before.widget_relayout);
    }
}
