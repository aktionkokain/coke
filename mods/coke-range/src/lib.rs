use coke_host::{CokeHost, RANGE_ENABLE_KEYS};

/// # Safety
/// Only the Coke loader may call this, with its advertised host table.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn coke_init(host: *const CokeHost) -> i32 {
    let Some(host) = (unsafe { CokeHost::copy_from_raw(host) }) else {
        return 0;
    };
    let Some(control) = host.range_control else {
        return 0;
    };
    if host.api_version < 7 || host.range_read.is_none() {
        return 0;
    }
    unsafe { control(RANGE_ENABLE_KEYS, core::ptr::null()) }
}
