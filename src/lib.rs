//! build the dll from all files.
//! Combat Mission Black Sea - drop-in `opengl32.dll` proxy.
//!
//! Layout: `ffi` (bindings) -> `state` (shared state) -> `config`/`util` -> `init`.
//! The rest are the individual hook surfaces the proxy installs.

#![allow(non_snake_case)]
// Lints that do not indicate defects in this codebase:
// - the hook/history tables are fixed-size arrays of atomics, so indexed loops
//   are the clearest way to read them;
// - the mod host's C ABI signatures and draw helpers are long by necessity;
// - `b"...\0"` is used next to the `wide()` UTF-16 helper, so mixing `c"..."`
//   would be less consistent.
#![allow(
    clippy::explicit_counter_loop,
    clippy::manual_c_str_literals,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

mod cbt;
mod coke;
mod config;
mod crash;
mod dialog;
mod engine;
mod ffi;
mod hooks;
mod hotkeys;
mod init;
mod install;
mod los;
mod opengl;
mod overlay;
mod perf;
mod profile;
mod rtti;
mod state;
mod tuning;
mod updates;
mod util;
mod watcher;
mod widgets;
mod window;
mod wtree;

// Every module is pulled into the crate root so the modules can reach each other
// through `use crate::*` without a wall of per-file import lists.
#[allow(unused_imports)]
pub(crate) use {
    cbt::*, coke::*, config::*, crash::*, dialog::*, engine::*, ffi::*, hooks::*, hotkeys::*,
    init::*, install::*, los::*, opengl::*, overlay::*, perf::*, profile::*, rtti::*, state::*,
    tuning::*, updates::*, util::*, watcher::*, widgets::*, window::*, wtree::*,
};

include!(concat!(env!("OUT_DIR"), "/names.rs"));
include!(concat!(env!("OUT_DIR"), "/thunks.rs"));

// ------------------------------------------------------------------ DllMain

// The signature is fixed by the Windows loader; every caller is the OS, which
// passes the module handle. The raw-pointer deref lint cannot apply here.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(instance: Hmodule, reason: u32, _reserved: usize) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { init(instance) };
    }
    1
}
