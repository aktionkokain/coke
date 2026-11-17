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

/// Register `cb` for an F1-F12 key. Returns `false` for a non-function key, a
/// full table, or a duplicate `(vk, callback)` pair. Safe to call from any thread.
pub fn register(vk: i32, cb: HotkeyFn) -> bool {
    if !(VK_F1..=VK_F12).contains(&vk) {
        log(&format!(
            "hotkey: rejected vk=0x{vk:x} (only F1-F12 are reserved for mods)"
        ));
        return false;
    }
    if vk == OWNED_F9 || vk == OWNED_F11 {
        log(&format!(
            "hotkey: F{} is owned by the proxy and cannot be registered",
            vk - VK_F1 + 1
        ));
        return false;
    }
    let mut guard = bus().lock().unwrap();
    if guard
        .iter()
        .any(|b| b.vk == vk && b.cb as usize == cb as usize)
    {
        return true;
    }
    if guard.len() >= MAX_BINDINGS {
        log("hotkey: table full");
        return false;
    }
    guard.push(Binding {
        vk,
        cb,
        down: false,
    });
    log(&format!("hotkey: F{} registered", vk - VK_F1 + 1));
    true
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
        if !register(vk, toggle_fps) {
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

/// Poll every registered key once. Run on the watcher thread. `focused` is the
/// host's foreground check, so a key pressed in another application never fires.
pub unsafe fn poll(focused: bool) {
    unsafe {
        if BUS.get().is_none() {
            return;
        }
        let mut fire: Vec<HotkeyFn> = Vec::new();
        {
            let Ok(mut guard) = bus().try_lock() else {
                return;
            };
            for binding in guard.iter_mut() {
                let down = focused && GetAsyncKeyState(binding.vk) < 0;
                if down && !binding.down {
                    fire.push(binding.cb);
                }
                binding.down = down;
            }
        }
        for cb in fire {
            cb();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
