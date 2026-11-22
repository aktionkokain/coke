//! Real opengl32.dll loading and the one-shot DllMain initialisation sequence.

use crate::*;
use core::sync::atomic::Ordering;
use std::path::PathBuf;

/// Runs after the loader has released the loader lock. `DllMain` must only
/// schedule this worker; loading the real OpenGL DLL and reading profiles from
/// inside `DllMain` can deadlock Windows startup.
pub extern "system" fn init_thread(param: *mut core::ffi::c_void) -> u32 {
    unsafe { init(param as Hmodule) };
    0
}

// ---- code ----
// ------------------------------------------------------------- real opengl32
/// This module is loaded as `opengl32.dll`, so `LoadLibraryExW` on the System32
/// `opengl32.dll` returns *this* module again (the loader matches by base name).
/// Keep a copy under a different name and load that instead.
unsafe fn real_dll_path() -> Option<PathBuf> {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;

    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(TEMP_DIR);
    if !local.is_absolute() {
        return None;
    }
    let mut sysdir = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(sysdir.as_mut_ptr(), sysdir.len() as u32) };
    if n == 0 || n as usize >= sysdir.len() {
        return None;
    }
    let source = PathBuf::from(unsafe { str_from_wide(sysdir.as_ptr()) }).join("opengl32.dll");
    let image = fs::read(source).ok()?;
    if image.is_empty() {
        return None;
    }
    let signature = image.iter().fold(0xcbf29ce484222325u64, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    });

    // Never truncate a DLL another game is about to load. Windows updates get
    // a fresh directory, and concurrent processes can reuse a complete copy.
    let dir = local.join(format!("{signature:016x}"));
    fs::create_dir_all(&dir).ok()?;
    if !ordinary_directory(&local) || !ordinary_directory(&dir) {
        return None;
    }
    let target = dir.join(REAL_DLL);
    let complete = || fs::read(&target).is_ok_and(|existing| existing == image);
    if complete() {
        return Some(target);
    }

    let temp = dir.join(format!("coke-{}-{}.tmp", std::process::id(), unsafe {
        GetTickCount()
    }));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .ok()?;
    let prepared = file
        .write_all(&image)
        .and_then(|()| file.sync_all())
        .is_ok();
    drop(file);
    if prepared {
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Some(target);
        }
    }
    let _ = fs::remove_file(temp);
    complete().then_some(target)
}

// The sole writer fills the table completely before publishing its address.
pub(crate) static FORWARD_TABLE: std::sync::atomic::AtomicPtr<usize> =
    std::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
static FORWARD_READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub(crate) fn require_forwarding() {
    if !*FORWARD_READY.get_or_init(|| unsafe { load_forwarding() }) {
        // Arbitrary GL exports have different stack cleanup and return types.
        // Fail the process explicitly rather than jump to an invalid address.
        std::process::abort();
    }
}

// x86 system exports take arguments on the caller's stack. The generated
// slow path balances its own cdecl argument before tail-jumping to OpenGL.
pub(crate) extern "C" fn first_forward(index: usize) -> usize {
    require_forwarding();
    unsafe { *FORWARD_TABLE.load(Ordering::Acquire).add(index) }
}

unsafe fn load_forwarding() -> bool {
    unsafe {
        let Some(path) = real_dll_path() else {
            log("OpenGL forwarding: cannot prepare the system DLL");
            return false;
        };
        use std::os::windows::ffi::OsStrExt;
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let module = LoadLibraryExW(
            wide_path.as_ptr(),
            core::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if module.is_null() || module as usize == INSTANCE_BASE.load(Ordering::Acquire) {
            log(&format!(
                "OpenGL forwarding: cannot load {}",
                path.display()
            ));
            return false;
        }
        let mut table = Box::new([0; FORWARDED_COUNT]);
        for (slot, name) in table.iter_mut().zip(FORWARDED_NAMES) {
            *slot = resolve(module, name) as usize;
            if *slot == 0 {
                log(&format!("OpenGL forwarding: missing {name}"));
                FreeLibrary(module);
                return false;
            }
        }
        for name in INTERCEPTED_NAMES {
            if resolve(module, name).is_null() {
                log(&format!("OpenGL forwarding: missing {name}"));
                FreeLibrary(module);
                return false;
            }
        }

        REAL_WGL_CREATE_CONTEXT.store(
            resolve(module, "wglCreateContext") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_MAKE_CURRENT.store(
            resolve(module, "wglMakeCurrent") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_DELETE_CONTEXT.store(
            resolve(module, "wglDeleteContext") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_GET_PROC_ADDRESS.store(
            resolve(module, "wglGetProcAddress") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_SWAP_BUFFERS.store(
            resolve(module, "wglSwapBuffers") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_SHARE_LISTS.store(resolve(module, "wglShareLists") as usize, Ordering::Relaxed);
        REAL_WGL_GET_CURRENT_CONTEXT.store(
            resolve(module, "wglGetCurrentContext") as usize,
            Ordering::Relaxed,
        );
        REAL_WGL_GET_CURRENT_DC.store(
            resolve(module, "wglGetCurrentDC") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_GET_STRING.store(resolve(module, "glGetString") as usize, Ordering::Relaxed);
        REAL_GL_VIEWPORT.store(resolve(module, "glViewport") as usize, Ordering::Relaxed);
        REAL_GL_SCISSOR.store(resolve(module, "glScissor") as usize, Ordering::Relaxed);
        REAL_GL_RASTER_POS2I.store(resolve(module, "glRasterPos2i") as usize, Ordering::Relaxed);
        REAL_GL_RASTER_POS3F.store(resolve(module, "glRasterPos3f") as usize, Ordering::Relaxed);
        REAL_GL_READ_PIXELS.store(resolve(module, "glReadPixels") as usize, Ordering::Relaxed);
        REAL_GL_COPY_TEX_SUB_IMAGE2D.store(
            resolve(module, "glCopyTexSubImage2D") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_COPY_TEX_IMAGE2D.store(
            resolve(module, "glCopyTexImage2D") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_DRAW_PIXELS.store(resolve(module, "glDrawPixels") as usize, Ordering::Relaxed);
        REAL_GL_DISABLE.store(resolve(module, "glDisable") as usize, Ordering::Relaxed);
        REAL_GL_ENABLE.store(resolve(module, "glEnable") as usize, Ordering::Relaxed);
        // OpenGL 1.1 entry points live in opengl32.dll's export table and
        // wglGetProcAddress returns NULL for them, so they have to come from
        // GetProcAddress like every other forwarded function.
        REAL_GL_GET_BOOLEANV.store(resolve(module, "glGetBooleanv") as usize, Ordering::Relaxed);
        REAL_GL_GET_FLOATV.store(resolve(module, "glGetFloatv") as usize, Ordering::Relaxed);
        REAL_GL_GET_INTEGERV.store(resolve(module, "glGetIntegerv") as usize, Ordering::Relaxed);
        // Triangle overlay (cokelos and future mods).
        REAL_GL_BLEND_FUNC.store(resolve(module, "glBlendFunc") as usize, Ordering::Relaxed);
        REAL_GL_MATRIX_MODE.store(resolve(module, "glMatrixMode") as usize, Ordering::Relaxed);
        REAL_GL_LOAD_IDENTITY.store(
            resolve(module, "glLoadIdentity") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_PUSH_MATRIX.store(resolve(module, "glPushMatrix") as usize, Ordering::Relaxed);
        REAL_GL_POP_MATRIX.store(resolve(module, "glPopMatrix") as usize, Ordering::Relaxed);
        REAL_GL_ORTHO.store(resolve(module, "glOrtho") as usize, Ordering::Relaxed);
        REAL_GL_VERTEX_POINTER.store(
            resolve(module, "glVertexPointer") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_COLOR_POINTER.store(
            resolve(module, "glColorPointer") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_COLOR_3F.store(resolve(module, "glColor3f") as usize, Ordering::Relaxed);
        REAL_GL_DRAW_ARRAYS.store(resolve(module, "glDrawArrays") as usize, Ordering::Relaxed);
        REAL_GL_ENABLE_CLIENT_STATE.store(
            resolve(module, "glEnableClientState") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_DISABLE_CLIENT_STATE.store(
            resolve(module, "glDisableClientState") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_DEPTH_MASK.store(resolve(module, "glDepthMask") as usize, Ordering::Relaxed);
        REAL_GL_PUSH_ATTRIB.store(resolve(module, "glPushAttrib") as usize, Ordering::Relaxed);
        REAL_GL_POP_ATTRIB.store(resolve(module, "glPopAttrib") as usize, Ordering::Relaxed);
        REAL_GL_PUSH_CLIENT_ATTRIB.store(
            resolve(module, "glPushClientAttrib") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_POP_CLIENT_ATTRIB.store(
            resolve(module, "glPopClientAttrib") as usize,
            Ordering::Relaxed,
        );
        REAL_GL_PIXEL_STORE_I.store(resolve(module, "glPixelStorei") as usize, Ordering::Relaxed);
        // Profiled entry points. They are intercepted by Rust bodies that
        // call these real pointers, so resolve all of them even when the
        // user never runs a profiler mod.
        for (slot, name) in [
            (&REAL_GL_CALL_LIST, "glCallList"),
            (&REAL_GL_BIND_TEXTURE, "glBindTexture"),
            (&REAL_GL_BEGIN, "glBegin"),
            (&REAL_GL_END, "glEnd"),
            (&REAL_GL_DRAW_ELEMENTS, "glDrawElements"),
            (&REAL_GL_TEX_IMAGE2D, "glTexImage2D"),
            (&REAL_GL_TEX_SUB_IMAGE2D, "glTexSubImage2D"),
            (&REAL_GL_NEW_LIST, "glNewList"),
            (&REAL_GL_END_LIST, "glEndList"),
            (&REAL_GL_GEN_LISTS, "glGenLists"),
            (&REAL_GL_DELETE_LISTS, "glDeleteLists"),
            (&REAL_GL_CALL_LISTS, "glCallLists"),
        ] {
            slot.store(resolve(module, name) as usize, Ordering::Relaxed);
        }

        REAL_MODULE.store(module as usize, Ordering::Release);
        FORWARD_TABLE.store(Box::into_raw(table).cast(), Ordering::Release);
        true
    }
}

pub unsafe fn init(instance: Hmodule) {
    unsafe {
        LOG_T0.store(GetTickCount(), Ordering::Relaxed);
        let mut pinned = core::ptr::null_mut();
        // The watcher, callbacks, CBT hooks and exception handler live until exit.
        if GetModuleHandleExW(0x5, instance.cast(), &mut pinned) == 0 {
            // PIN | FROM_ADDRESS
            log(&format!(
                "init: could not pin the process-lifetime proxy: {}",
                GetLastError()
            ));
            return;
        }
        let veh = install_exception_logger();
        let pid = GetCurrentProcessId();
        let cfg = load_config();
        let _ = CONFIG.set(cfg.clone());
        hotkeys::init_host(&cfg);

        let (logical_w, logical_h) = compute_logical_size(&cfg);
        LOGICAL_W.store(logical_w, Ordering::Relaxed);
        LOGICAL_H.store(logical_h, Ordering::Relaxed);
        STARTUP_LOGICAL_W.store(logical_w, Ordering::Relaxed);
        STARTUP_LOGICAL_H.store(logical_h, Ordering::Relaxed);
        PHYS_W.store(logical_w, Ordering::Relaxed);
        PHYS_H.store(logical_h, Ordering::Relaxed);
        WINDOW_MODE.store(cfg.start_fullscreen as u32, Ordering::Relaxed);

        let user32 = LoadLibraryW(wide("user32.dll").as_ptr());
        if !user32.is_null() {
            REAL_CLOSE_WINDOW.store(resolve(user32, "CloseWindow") as usize, Ordering::Relaxed);
            REAL_METRICS.store(
                resolve(user32, "GetSystemMetrics") as usize,
                Ordering::Relaxed,
            );
            REAL_CHANGE_DISPLAY.store(
                resolve(user32, "ChangeDisplaySettingsA") as usize,
                Ordering::Relaxed,
            );
            REAL_GET_CURSOR_POS.store(resolve(user32, "GetCursorPos") as usize, Ordering::Relaxed);
            REAL_ASYNC_KEY_STATE.store(
                resolve(user32, "GetAsyncKeyState") as usize,
                Ordering::Relaxed,
            );
            REAL_SHOW_WINDOW.store(resolve(user32, "ShowWindow") as usize, Ordering::Relaxed);
            REAL_SHOW_WINDOW_ASYNC.store(
                resolve(user32, "ShowWindowAsync") as usize,
                Ordering::Relaxed,
            );
        }

        if cfg.hook_cbt {
            install_cbt_hooks();
        }

        // Drag-and-drop install: a PROFILES.zip next to the game is extracted
        // into profiles/ before discovery runs.
        install_profiles();

        // The profile is selected by build identity, never by address or name
        // alone. No match -> safe mode: window/F11/FPS/mods only, no engine writes.
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        let image_size = module_image_size(base);
        let timestamp = pe_timestamp(base);
        log(&format!(
            "[pid {pid}] module base=0x{base:x} image_size=0x{image_size:x} ts=0x{timestamp:x} \
         dll=0x{instance:p} veh=0x{veh:p}"
        ));
        let selection = profile::load(&cfg);
        match &selection {
            profile::Selection::Matched(p) => {
                log(&format!(
                    "profile: exe={} build={} status={} image=0x{:x} ts=0x{:x} base=0x{base:x} \
                 slots={} canvas={} scene={} rtti={} los={} terrain={} range={} source={}",
                    p.exe,
                    p.build,
                    p.status.tag(),
                    p.image_size,
                    p.pe_timestamp,
                    p.slots.any() as u8,
                    p.canvas.is_some() as u8,
                    p.scene.is_some() as u8,
                    p.rtti.len(),
                    p.los.is_some() as u8,
                    p.terrain.is_some() as u8,
                    p.range.is_some() as u8,
                    p.source
                ));
                validate_profile_slots();
            }
            profile::Selection::SafeMode(reason) => {
                log(&format!(
                    "profile: safe mode (no engine writes): {reason} \
                 [pid {pid}] module base=0x{base:x} image_size=0x{image_size:x}"
                ));
            }
        }
        if cfg.profile_dump {
            diagnose_slots(base, image_size);
            let (text_rva, text_size) = module_text_range(base).unwrap_or((0, 0));
            log(&format!(
                "profile dump: module base=0x{base:x} image=0x{image_size:x} ts=0x{:x} \
             text_rva=0x{text_rva:x} text_size=0x{text_size:x} text_fnv1a=0x{:x} \
             blocks={}",
                profile::run_timestamp(),
                profile::text_hash().unwrap_or(0),
                profile::profile()
                    .map(|p| p.blocks())
                    .unwrap_or_else(|| "none".to_string()),
            ));
            if profile::profile().is_none() {
                log(
                    "profile dump: no profile matched; the identity above (image, ts, text hash, \
                 slot RVAs from the IAT scan lines, RTTI lines) is what a new profile needs",
                );
            }
            for (rva, what) in [
                (slots().map(|s| s.close_window).unwrap_or(0), "close_window"),
                (
                    slots().map(|s| s.get_system_metrics).unwrap_or(0),
                    "get_system_metrics",
                ),
                (
                    slots().map(|s| s.change_display).unwrap_or(0),
                    "change_display",
                ),
                (
                    slots().map(|s| s.get_cursor_pos).unwrap_or(0),
                    "get_cursor_pos",
                ),
            ] {
                log(&format!(
                    "profile dump: slot {what} rva=0x{rva:x} addr={:?}",
                    profile_addr(rva)
                ));
            }
        }

        log(&format!(
            "[pid {pid}] config: logical={logical_w}x{logical_h} fullscreen={} window_scale={}% fullscreen_inset={} edge={} stretch={} \
fix_alt_tab={} limit_fps={} vsync={:?} priority={} affinity=0x{:x} scale_fixes={} promote_scene={} hook_slots={} canvas_patch={} stale_viewport_fix={} scene_rect_fix={} hook_cbt={} hook_window={} hook_wndproc={} hook_mode={}",
            cfg.start_fullscreen,
            cfg.window_scale,
            cfg.fullscreen_inset,
            cfg.fullscreen_inset_edge,
            cfg.stretch,
            cfg.fix_alt_tab,
            cfg.limit_fps,
            cfg.vsync,
            cfg.priority,
            cfg.affinity,
            cfg.scale_fixes,
            cfg.promote_scene,
            cfg.hook_slots,
            cfg.canvas_patch,
            cfg.stale_viewport_fix,
            cfg.scene_rect_fix,
            cfg.hook_cbt,
            cfg.hook_window,
            cfg.hook_wndproc,
            cfg.hook_mode
        ));

        // Install the slot hooks immediately; the watcher keeps them installed in
        // case the unpacker overwrites the slots later during startup.
        if cfg.hook_slots {
            install_slot_hooks();
        }

        let mut tid = 0u32;
        let worker = CreateThread(
            core::ptr::null_mut(),
            256 * 1024,
            watcher_thread,
            core::ptr::null_mut(),
            0,
            &mut tid,
        );
        if worker.is_null() {
            log(&format!(
                "[pid {pid}] watcher thread failed: {}",
                GetLastError()
            ));
        } else {
            CloseHandle(worker);
            log(&format!("[pid {pid}] watcher thread {tid}"));
        }
        log("[init] done");
    }
}
