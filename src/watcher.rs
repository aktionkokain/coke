//! Background thread that finds the window and keeps every hook installed.

use crate::*;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::{c_char, c_void};

// ---- code ----
// ------------------------------------------------------------------ watcher

static DUMP_DONE: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
struct WindowCandidates {
    game: (usize, usize),
    fallback: (usize, usize),
}

pub(crate) unsafe fn owns_window(hwnd: Hwnd) -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) != 0 && pid == GetCurrentProcessId() }
}

extern "system" fn enum_main_window(hwnd: Hwnd, param: isize) -> i32 {
    unsafe {
        if !owns_window(hwnd)
            || IsWindowVisible(hwnd) == 0
            || !GetWindow(hwnd, GW_OWNER).is_null()
            || GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD != 0
        {
            return 1;
        }
        let mut class = [0u8; 64];
        let len = GetClassNameA(hwnd, class.as_mut_ptr().cast(), class.len() as i32);
        if len <= 0 || class[..len as usize].eq_ignore_ascii_case(dialog::CLASS_NAME.as_bytes()) {
            return 1;
        }
        let mut r = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut r) == 0 {
            return 1;
        }
        let w = (r.right - r.left).max(0) as usize;
        let h = (r.bottom - r.top).max(0) as usize;
        if w < 320 || h < 240 {
            return 1;
        }
        let candidates = &mut *(param as *mut WindowCandidates);
        let area = w.saturating_mul(h);
        if area > candidates.fallback.0 {
            candidates.fallback = (area, hwnd as usize);
        }
        if class_is_cm(hwnd) && area > candidates.game.0 {
            candidates.game = (area, hwnd as usize);
        }
        1
    }
}

unsafe fn find_main_window(game_only: bool) -> Hwnd {
    let mut candidates = WindowCandidates::default();
    unsafe {
        EnumWindows(
            enum_main_window,
            &mut candidates as *mut WindowCandidates as isize,
        )
    };
    if candidates.game.1 != 0 {
        candidates.game.1 as Hwnd
    } else if game_only {
        core::ptr::null_mut()
    } else {
        candidates.fallback.1 as Hwnd
    }
}

const GENERATION_PROPERTY: *const c_char = c"Coke.MainWindowGeneration".as_ptr();

pub(crate) unsafe fn current_window(hwnd: Hwnd) -> bool {
    unsafe {
        MAIN_WINDOW.load(Ordering::Acquire) == hwnd as usize
            && owns_window(hwnd)
            && GetPropA(hwnd, GENERATION_PROPERTY) as usize
                == WINDOW_GENERATION.load(Ordering::Acquire) as usize
    }
}

pub extern "system" fn watcher_thread(_param: *mut c_void) -> u32 {
    unsafe { watcher() }
}

pub unsafe fn watcher() -> u32 {
    unsafe {
        let cfg = config().clone();
        let mut was_f11 = true;
        let mut was_f9 = true;
        let mut last_client = 0usize;
        let mut client_since = GetTickCount();
        let mut ticks = 0u32;
        let mut tuning_done = false;
        let t0 = GetTickCount();
        log("watcher: entered");
        // Resolve the widget RTTI class names once. This is what lets the layout
        // identify the battle bar/scene/flanks by class instead of by geometry.
        init_class_table();
        // The watcher thread is the safe place for LoadLibrary: DllMain still holds
        // the loader lock while `init` runs.
        load_coke_mods();

        loop {
            ticks += 1;

            // Resolve widget class names once the unpacked image is available.
            // (No-op after the first success; retries are throttled internally.)
            init_class_table();

            // Optional: write the live (unpacked) main module to analysis/ once.
            // The packed image on disk is not the code that runs, and DllMain runs
            // before the unpacker, so wait until the section table looks real.
            if cfg.dump_module
                && DUMP_DONE.load(Ordering::Relaxed) == 0
                && ticks.is_multiple_of(100)
            {
                let base = GetModuleHandleW(core::ptr::null()) as usize;
                if module_section_count(base) >= 3 && DUMP_DONE.swap(1, Ordering::Relaxed) == 0 {
                    match dump_main_module() {
                        Some(path) => log(&format!("dump: wrote {}", path.display())),
                        None => log("dump: could not read the main module image"),
                    }
                }
            }

            // Startup heartbeat: tells "the process died here" apart from "the
            // process is alive but no engine window was found yet".
            if cfg.verbose
                && ticks < 2000
                && ((ticks < 400 && ticks.is_multiple_of(20)) || ticks.is_multiple_of(200))
            {
                log(&format!(
                    "alive t={} ms hwnd=0x{:x}",
                    GetTickCount().wrapping_sub(t0),
                    MAIN_WINDOW.load(Ordering::Relaxed)
                ));
            }

            // Keep the create-time hook installed on every thread: it is what
            // creates the engine's window at its final size instead of resizing it
            // after the fact. Threads appear during startup, so rescan for a while.
            if cfg.hook_cbt && ticks < 4000 && ticks % 200 == 1 {
                install_cbt_hooks();
            }

            // Keep the slots installed: the unpacker may overwrite them while it
            // resolves imports, and a late write would silently undo the fix.
            if ticks < 12000 {
                if cfg.hook_slots {
                    install_slot_hooks();
                }
            } else if ticks.is_multiple_of(50) && cfg.hook_slots {
                install_slot_hooks();
            }

            // A packed executable can expose its real image only after DllMain.
            // Retry profile selection for a bounded time; writes stay gated by
            // PROFILE_OK, so a late match cannot race a hook that already ran.
            if !profile_ok()
                && (100..4000).contains(&ticks)
                && ticks % 100 == 50
                && profile::retry(&cfg)
            {
                validate_profile_slots();
            }

            // Worker priorities: the engine creates them during startup.
            if cfg.boost_workers && ticks.is_multiple_of(200) && ticks < 800 {
                boost_workers();
            }

            // The engine sets its own priority class early; apply ours once it has.
            if !tuning_done && ticks == 600 {
                apply_process_tuning(&cfg);
                tuning_done = true;
            }

            let mut hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as Hwnd;

            if !cfg.hook_window {
                Sleep(50);
                continue;
            }

            // The engine finishes its own display init by destroying the window it
            // started with and creating a new one. A cached handle then points at
            // dead memory, so drop it and look the window up again.
            let replacement = if !hwnd.is_null() && ticks.is_multiple_of(25) && !class_is_cm(hwnd) {
                find_main_window(true)
            } else {
                core::ptr::null_mut()
            };
            if !hwnd.is_null() && (!current_window(hwnd) || !replacement.is_null()) {
                if current_window(hwnd) {
                    RemovePropA(hwnd, GENERATION_PROPERTY);
                }
                MAIN_WINDOW.store(0, Ordering::Release);
                INITIAL_MODE_DONE.store(0, Ordering::Relaxed);
                WINDOW_DRAGGING.store(0, Ordering::Relaxed);
                last_client = 0;
                was_f9 = true;
                was_f11 = true;
                set_camera_input_capture(0);
                hotkeys::poll(false);
                log("main window changed; looking for the current game window");
                hwnd = core::ptr::null_mut();
            }

            if hwnd.is_null() {
                hwnd = if replacement.is_null() {
                    find_main_window(false)
                } else {
                    replacement
                };
                let generation = WINDOW_GENERATION
                    .load(Ordering::Relaxed)
                    .wrapping_add(1)
                    .max(1);
                if owns_window(hwnd)
                    && SetPropA(hwnd, GENERATION_PROPERTY, generation as Handle) != 0
                {
                    WINDOW_GENERATION.store(generation, Ordering::Release);
                    SCENE_GENERATION.fetch_add(1, Ordering::Relaxed);
                    MAIN_WINDOW.store(hwnd as usize, Ordering::Release);
                    log_geometry(hwnd, "main window found");
                    if cfg.hook_wndproc {
                        install_wndproc_hook(hwnd);
                    }
                } else {
                    Sleep(250);
                    continue;
                }
            }

            // The engine installs its own window procedure during startup and can
            // replace ours. Re-check it while the process is still settling.
            if (ticks < 12000 || ticks.is_multiple_of(100)) && cfg.hook_wndproc {
                install_wndproc_hook(hwnd);
            }

            if !current_window(hwnd) {
                continue;
            }

            // Keep the logical -> physical mapping in step with the real client
            // area, whatever resized it. This is what makes live resize work.
            //
            // When `width`/`height` are unset the logical resolution *follows* the
            // client area: the engine then renders natively at the window's size and
            // the mapping is the identity. That switch also re-lays out the engine,
            // so it is debounced until the size has settled - otherwise every mouse
            // position during a drag would recreate the engine's buffers.
            let mut c = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetClientRect(hwnd, &mut c);
            let cw = c.right - c.left;
            let ch = c.bottom - c.top;
            let size_key = ((cw as usize) << 16) | (ch as usize & 0xffff);
            if size_key != last_client {
                let (old_sx, old_sy) = scale_xy();
                sync_scale(hwnd);
                let (sx, sy) = scale_xy();
                if last_client == 0 || (sx - old_sx).abs() > 1e-4 || (sy - old_sy).abs() > 1e-4 {
                    log(&format!(
                        "client {cw}x{ch} -> scale {sx:.4}x{sy:.4} (logical {}x{})",
                        LOGICAL_W.load(Ordering::Relaxed),
                        LOGICAL_H.load(Ordering::Relaxed)
                    ));
                }
                last_client = size_key;
                client_since = GetTickCount();
            } else if GetTickCount().wrapping_sub(client_since) >= 250 {
                // Size has been stable for a moment: hand it to the engine.
                // (No-op when a fixed logical resolution is configured.)
                adopt_client_logical(hwnd);
            }

            // Safety net: if the app object did not exist when `adopt` ran, or the
            // engine recreated it, make the cached canvas catch up now that
            // LOGICAL and the client agree.
            keep_engine_canvas_in_sync(hwnd);

            // Re-center engine widgets laid out for a previous canvas (runs the
            // scan only shortly after an actual canvas patch).
            widget_recenter_tick();

            // Diagnostic: read-only widget-tree dump, scheduled by a canvas change
            // or the first battle frame.
            widget_dump_tick();

            // Native re-layout: replay the engine's rects against the new canvas.
            widget_relayout_tick();

            // Apply the requested start mode exactly once per engine window. This
            // must not wait for a swap: the intro window never goes through
            // wglSwapBuffers, so a frame-based gate would leave it fullscreen for
            // its whole lifetime.
            let iconic = IsIconic(hwnd) != 0;
            let visible = IsWindowVisible(hwnd) != 0;
            if !iconic
                && visible
                && INITIAL_MODE_DONE.swap(1, Ordering::Relaxed) == 0
                && cfg.hook_mode
            {
                log_geometry(hwnd, "before initial window mode");
                apply_window_mode(hwnd, WINDOW_MODE.load(Ordering::Relaxed) == 1);
                log_geometry(hwnd, "after initial window mode");
            }

            let focused = !iconic && GetForegroundWindow() == hwnd;
            if !focused && camera_input_capture() {
                set_camera_input_capture(0);
            }
            if hotkeys::key_pressed(&mut was_f11, focused, GetAsyncKeyState(VK_F11) < 0)
                && cfg.hook_mode
            {
                let fullscreen = WINDOW_MODE.load(Ordering::Relaxed) == 1;
                log(&format!(
                    "F11: {}",
                    if fullscreen {
                        "framed window"
                    } else {
                        "borderless fullscreen"
                    }
                ));
                apply_window_mode(hwnd, !fullscreen);
                log_geometry(hwnd, "after F11");
            }
            if hotkeys::key_pressed(&mut was_f9, focused, GetAsyncKeyState(VK_F9) < 0)
                && cfg.hook_mode
            {
                schedule_widget_dump(DUMP_ON_KEY);
            }
            hotkeys::poll(focused);

            // Opt-in release check. Starts a worker at most once, a few seconds
            // after the window appears; no-op when `check_updates=off`.
            if !iconic {
                updates::tick(hwnd);
            }

            // While a heap scan is in progress, poll fast: the F11 key and the
            // window-geometry work are handled on this thread, and the scan
            // yields every ~10 ms slice.
            Sleep(
                if crate::widgets::scan_in_progress() || relayout_pending() {
                    2
                } else if ticks < 12000 {
                    5
                } else {
                    50
                },
            );
        }
    }
}
