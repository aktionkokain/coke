# CokeRange

CokeRange provides a surface-to-surface rangefinder in Combat Mission Black Sea Steam x86. Pressing **T** starts its source on the nearest visible surface under the cursor, including building roofs; moving the cursor measures to the next visible surface. It calls the map-visibility routine used by Target and submits the ribbon through the native battle renderer. It requires the Coke v7 host and a matching profile with the `[range]` block.

- With the cursor over the battlefield and no unit selected, press **T** to set the origin. Move the cursor to measure to another point.
- With a unit selected, use **Shift+T**. Bare **T** still starts the unit's Target order.
- Press **H** while CokeRange is active to cycle the source height through **0.25 m**, **1 m**, **2 m**, and **3.5 m**. The lowest level is near ground height. CokeRange leaves **E** available to Coke Shot’s camera control. Press **T** or **Esc**, or right-click, to close CokeRange. Switching scenes, losing focus, or entering a native command clears the origin.

CokeRange reads the battle render target's depth at the cursor. Walls, roofs and depth-writing foliage can become the nearest surface. The origin uses the cursor position at the key press and waits for matching depth data. Pixels without a valid sample leave the preview waiting for a visible surface. Depth readback uses pixel-buffer objects and non-blocking fences. The range ribbon and visibility verdict use the native engine. A selected unit's Target verdict can also depend on its weapons, crew, optics and spotting state.

On the verified Black Sea build, `Ctrl+Shift+T` toggles the experimental terrain LOS painter. It picks an observer surface under the cursor and classifies the map with the same native visibility routine as the range preview, including the engine's building and woodland blocking rules. The scan runs in small batches on the battle render thread. It samples at 16 m across the map and 8 m near the observer, with coarser cells on very large maps. Visible cells are green and blocked cells are red. Both nearby terrain and distant meshes use the resulting colors. Toggle again to clear the paint. Set `range_terrain_overlay_test=1` in `coke_config.ini` and restart to enable it.

Build with `cargo build --release --locked` in this directory. Install `coke_range.dll` and `user_consent_coke_range.txt` under the game's `coke` directory. Update the host and the Black Sea profile together. The repository's packaging scripts include the submod.

## Ribbon settings

Edit `[range]` in `coke_config.ini` next to the game executable, then restart the game. These settings belong to the Coke host and apply to the shared range ribbon.

| Setting | Default | Effect |
| --- | ---: | --- |
| `range_line_width` | `0.65` | Native ribbon width at close range. |
| `range_line_scale_distance` | `100` | Distance in metres at which the ribbon starts to widen. Set to `0` for fixed width. |
| `range_line_max_width` | `12` | Cap on scaled width. The cap has a minimum of the base width. |

At twice the configured distance, the ribbon doubles in width until it reaches the cap. Width uses the distance from the camera to the farther endpoint, so clear and blocked portions of the same measurement stay the same thickness.

## Terrain tint experiment

On the verified Black Sea Steam x86 build, set `range_terrain_overlay_test=1` in the game's `coke_config.ini` and restart. Aim at the ground and press **Ctrl+Shift+Y** to place a red 24 m square. Aim again and press the chord to move it and turn it blue. A third press clears it. Each placement uses CokeRange's cursor ray and terrain intersection.

The deployment compositor starts at RVA `0x003D05C0`. Its call at `0x003D0A2A` invokes the zone color writer at `0x003D03A0`. The writer multiplies the float RGB grid at `TerrainMap + 0x116F44`; the terrain renderer at RVA `0x00326AD0` draws that grid. The experiment saves the original RGB values and applies the deployment palette to the chosen area. Clearing the patch restores those values and invalidates the affected terrain color caches.

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
