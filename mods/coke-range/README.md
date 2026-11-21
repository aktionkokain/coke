# CokeRange

CokeRange provides a ground-to-ground rangefinder in Combat Mission Black Sea Steam x86. It calls the map-visibility routine used by Target and submits the ribbon through the native battle renderer. It requires the Coke v7 host and a matching profile with the `[range]` block.

- With the cursor over the battlefield and no unit selected, press **T** to set the origin. Move the cursor to measure to another point.
- With a unit selected, use **Shift+T**. Bare **T** still starts the unit's Target order.
- Press **T** or **Esc**, or right-click, to close CokeRange. Switching scenes, losing focus, or entering a native command clears the origin.

CokeRange uses a standing observer for the ground origin. The game’s selected-unit verdict also depends on that unit’s weapons, crew, optics and spotting state. The native engine draws the ribbon and range label and supplies the visibility result. Coke projects the cursor through the live battle matrices and intersects the ground.

Build with `cargo build --release --locked` in this directory. Install `coke_range.dll` and `user_consent_coke_range.txt` under the game's `coke` directory. Update the host and the Black Sea profile together. The repository's packaging scripts include the submod.

## Host integration

Use `CokeHost::copy_from_raw` during initialization. Check for API version 7, the advertised table size and the service pointers before using them.

- Call `range_control(RANGE_FROM_CURSOR, NULL)` or `range_control(RANGE_SET_ORIGIN, &point)` to start the shared preview. A point is in world metres with x east, y north and z up. Accepted requests resolve on the battle render thread. Call `range_control(RANGE_CANCEL, NULL)` to clear the preview.
- Call `range_read(&result)` to copy the current status, endpoints, horizontal range in metres and visible fraction. A successful read can return `RANGE_WAITING` while the cursor is off the map. A return of 0 means idle, unavailable or busy. These functions can be called from any host callback.
- Pass `RANGE_ENABLE_KEYS` to opt in to the keyboard controls. Plugins with their own controls can omit it. All callers share one preview and should coordinate ownership.

On x86, the v7 table is 152 bytes. `range_control` is at byte 144 and `range_read` is at byte 148. Callbacks use `extern "system"` (`__stdcall` in C on x86). `CokeRangePoint` is three consecutive `float` values. `CokeRangeResult` is 44 bytes in the field order declared in [coke-host](../../crates/coke-host/src/lib.rs).

The host installs the native hook in process memory on the main render thread after it matches the profile and verifies the original call instruction. Porting requires verified addresses and compatible engine layouts.
