# CokeRange

CokeRange provides a surface-to-surface rangefinder in Combat Mission Black Sea Steam x86. Pressing **T** starts its source on the nearest visible surface under the cursor, including building roofs; moving the cursor measures to the next visible surface. It calls the map-visibility routine used by Target and submits the ribbon through the native battle renderer. It requires the Coke v7 host and a matching profile with the `[range]` block.

- With the cursor over the battlefield and no unit selected, press **T** to set the origin. Move the cursor to measure to another point.
- With a unit selected, use **Shift+T**. Bare **T** still starts the unit's Target order.
- Press **H** while CokeRange is active to cycle the source height through **0.25 m**, **1 m**, **2 m**, and **3.5 m**. The lowest level is near ground height. CokeRange leaves **E** available to Coke Shot’s camera control. Press **T** or **Esc**, or right-click, to close CokeRange. Switching scenes, losing focus, or entering a native command clears the origin.

CokeRange picks the nearest rendered surface under the cursor from the battle depth buffer, so house walls and roofs can become the source or endpoint instead of being skipped by a terrain-only ray. On startup it waits for a depth sample at the current cursor before fixing the source; if depth sampling is unavailable, it falls back to a terrain intersection. Depth readback uses pixel-buffer objects and non-blocking fences. Visibility classification is refreshed at most every 100 ms while the measured endpoint continues to follow the cursor. The source-height cycle adjusts the observer ray relative to the picked source surface. The game’s selected-unit verdict also depends on that unit’s weapons, crew, optics and spotting state. The native engine draws the ribbon and range label and supplies the visibility result.

Build with `cargo build --release --locked` in this directory. Install `coke_range.dll` and `user_consent_coke_range.txt` under the game's `coke` directory. Update the host and the Black Sea profile together. The repository's packaging scripts include the submod.

## Host integration

Use `CokeHost::copy_from_raw` during initialization. Check for API version 7, the advertised table size and the service pointers before using them.

- Call `range_control(RANGE_FROM_CURSOR, NULL)` or `range_control(RANGE_SET_ORIGIN, &point)` to start the shared preview. `RANGE_FROM_CURSOR` waits for the nearest surface under the cursor; an explicit point is in world metres with x east, y north and z up. Accepted requests resolve on the battle render thread. Call `range_control(RANGE_CANCEL, NULL)` to clear the preview.
- Call `range_read(&result)` to copy the current status, endpoints, horizontal range in metres and visible fraction. A successful read can return `RANGE_WAITING` while the cursor is off the map. A return of 0 means idle, unavailable or busy. These functions can be called from any host callback.
- Pass `RANGE_ENABLE_KEYS` to opt in to the keyboard controls. Plugins with their own controls can omit it. All callers share one preview and should coordinate ownership.
- While the shared preview is active, **H** cycles the source through 0.25 m, 1 m, 2 m, and 3.5 m. Call `range_control(RANGE_SET_SOURCE_HEIGHT, &point)` to set a source height in metres (0.25–6.0); pass it in `point.z` (`x` and `y` are ignored). This action does not change the v7 table layout. All callers share this setting and should coordinate ownership.

```rust
let height = CokeRangePoint { x: 0.0, y: 0.0, z: 3.5 };
unsafe { range_control(RANGE_SET_SOURCE_HEIGHT, &height) };
```

On x86, the v7 table is 152 bytes. `range_control` is at byte 144 and `range_read` is at byte 148. Callbacks use `extern "system"` (`__stdcall` in C on x86). `CokeRangePoint` is three consecutive `float` values. `CokeRangeResult` is 44 bytes in the field order declared in [coke-host](../../crates/coke-host/src/lib.rs).

The host installs the native hook in process memory on the main render thread after it matches the profile and verifies the original call instruction. Porting requires verified addresses and compatible engine layouts.
