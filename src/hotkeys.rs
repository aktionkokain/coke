//! Host-owned hotkey bus.
//!
//! The game's `Data/hotkeys.txt` cannot bind function keys or modifiers, so the
//! proxy polls `GetAsyncKeyState` itself. Mods used to repeat that loop on their
//! own thread; now they register an F1-F12 key here and the host owns polling,
//! focus checks and edge de-duplication.
//!
//! Rules:
//! - only F1-F12 are accepted (debug-style keys, never game bindings);
//! - a callback fires once per press (rising edge), never while unfocused;
//! - several callbacks may share one key; each fires once per press;
//! - the core FPS counter uses [`fps_hotkey`], configured in `coke_config.ini`.
//!
//! Reserved map (documented in `docs/ai/CONFIG.md`):
//!
//! | Key | Owner |
//! |---|---|
//! | F7 | `coke-prof` reset |
//! | F8 | `coke-prof` HUD |
//! | F9 | widget-tree dump (host) |
//! | F10 | core FPS counter / `coke-fps` |
//! | F11 | windowed <-> borderless |
//! | F12 | free |
//! | F1-F6 | free for debug-style mods |

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// `VK_F1`; F1..F12 are contiguous.
pub const VK_F1: i32 = 0x70;
pub const VK_F12: i32 = 0x7b;
/// Host-owned keys that never go through the bus: F9 dumps the widget tree and
/// F11 toggles the window mode in the legacy watcher path.
const OWNED_F9: i32 = 0x78;
const OWNED_F11: i32 = 0x7a;

pub type HotkeyFn = unsafe extern "system" fn();

/// Maximum number of registered callbacks, so a broken mod cannot grow the table
/// without bound.
const MAX_BINDINGS: usize = 64;

struct Binding {
    owner: usize,
    vk: i32,
    cb: HotkeyFn,
    /// Previous poll state, for rising-edge detection.
    down: bool,
}

static BUS: OnceLock<Mutex<Vec<Binding>>> = OnceLock::new();
static FPS_VISIBLE: AtomicU32 = AtomicU32::new(1);

fn bus() -> &'static Mutex<Vec<Binding>> {
    BUS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Reserve registrations in the caller's initialization transaction.
pub(crate) fn stage(pending: &mut Vec<(i32, HotkeyFn)>, vk: i32, cb: HotkeyFn) -> bool {
    if !(VK_F1..=VK_F12).contains(&vk) || vk == OWNED_F9 || vk == OWNED_F11 {
        log(&format!(
            "hotkey: key 0x{vk:x} is unavailable for registration"
        ));
        return false;
    }
    if pending
        .iter()
        .any(|&(key, existing)| key == vk && existing as usize == cb as usize)
    {
        return true;
    }
    if bus().lock().unwrap().len() + pending.len() >= MAX_BINDINGS {
        log("hotkey: table full");
        return false;
    }
    pending.push((vk, cb));
    true
}

/// The loader is the only registrar after host setup. Same-key callbacks
/// run in registration order, and duplicate pairs within one owner coalesce.
pub(crate) fn publish(owner: usize, pending: Vec<(i32, HotkeyFn)>) {
    let mut guard = bus().lock().unwrap();
    for (vk, cb) in pending {
        guard.push(Binding {
            owner,
            vk,
            cb,
            down: true,
        });
        log(&format!(
            "hotkey: owner=0x{owner:x} F{} registered",
            vk - VK_F1 + 1
        ));
    }
}

/// `1..=12` (an F-key number) to its virtual-key code. `0` = hotkey disabled.
pub fn vk_for_fkey(n: u8) -> Option<i32> {
    (1..=12).contains(&n).then_some(VK_F1 + n as i32 - 1)
}

/// The configured F-key number for the core FPS counter, if any.
pub fn fps_hotkey(cfg: &Config) -> Option<i32> {
    vk_for_fkey(cfg.fps_hotkey)
}

/// Install the host's own hotkeys. Call once after the config is loaded and
/// before the watcher starts polling.
pub fn init_host(cfg: &Config) {
    FPS_VISIBLE.store(cfg.show_fps as u32, Ordering::Relaxed);
    if let Some(vk) = fps_hotkey(cfg) {
        let mut pending = Vec::new();
        if stage(&mut pending, vk, toggle_fps) {
            publish(0, pending);
        } else {
            log("hotkey: FPS hotkey could not be registered");
        }
    } else {
        log("hotkey: FPS toggle disabled (fps_hotkey=off)");
    }
}

/// Whether the core FPS overlay should draw this frame. Starts at `show_fps` and
/// is toggled at runtime by the hotkey.
pub fn fps_visible() -> bool {
    FPS_VISIBLE.load(Ordering::Relaxed) != 0
}

unsafe extern "system" fn toggle_fps() {
    let visible = FPS_VISIBLE.fetch_xor(1, Ordering::Relaxed) ^ 1;
    log(&format!(
        "hotkey: FPS counter {}",
        if visible != 0 { "on" } else { "off" }
    ));
}

/// Suppress a held key until a focused poll observes its release.
pub(crate) fn key_pressed(was_down: &mut bool, focused: bool, down: bool) -> bool {
    let pressed = focused && down && !*was_down;
    *was_down = !focused || down;
    pressed
}

/// Poll registered keys on the watcher thread and dispatch outside the bus lock.
pub unsafe fn poll(focused: bool) {
    if BUS.get().is_none() {
        return;
    }
    let keys = if focused {
        core::array::from_fn(|i| unsafe { GetAsyncKeyState(VK_F1 + i as i32) < 0 })
    } else {
        [false; 12]
    };
    dispatch(focused, keys);
}

fn dispatch(focused: bool, keys: [bool; 12]) {
    let mut fire = Vec::new();
    {
        let Ok(mut guard) = bus().try_lock() else {
            return;
        };
        for binding in guard.iter_mut() {
            let down = keys[(binding.vk - VK_F1) as usize];
            if key_pressed(&mut binding.down, focused, down) {
                fire.push((binding.owner, binding.cb));
            }
        }
    }
    for (_owner, callback) in fire {
        unsafe { callback() };
    }
}

#[cfg(test)]
pub(crate) fn press_for_test(vk: i32) {
    dispatch(true, [false; 12]);
    let mut keys = [false; 12];
    keys[(vk - VK_F1) as usize] = true;
    dispatch(true, keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_regain_requires_key_release() {
        let mut down = false;
        assert!(key_pressed(&mut down, true, true));
        assert!(!key_pressed(&mut down, false, true));
        assert!(!key_pressed(&mut down, true, true));
        assert!(!key_pressed(&mut down, true, false));
        assert!(key_pressed(&mut down, true, true));
    }

    #[test]
    fn only_f_keys_are_accepted() {
        assert_eq!(vk_for_fkey(1), Some(0x70));
        assert_eq!(vk_for_fkey(10), Some(0x79));
        assert_eq!(vk_for_fkey(12), Some(0x7b));
        assert_eq!(vk_for_fkey(0), None);
        assert_eq!(vk_for_fkey(13), None);
    }

    #[test]
    fn fps_hotkey_parses_the_config_field() {
        let mut cfg = Config::default();
        assert_eq!(cfg.fps_hotkey, 10);
        assert_eq!(fps_hotkey(&cfg), Some(0x79));
        cfg.fps_hotkey = 0;
        assert_eq!(fps_hotkey(&cfg), None);
    }
}
