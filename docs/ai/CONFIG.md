# Configuration

All settings live in `coke_config.ini` next to `CM Black Sea.exe`. The file is
read once at process start; restart the game after editing. Lines starting with
`#` or `;` are comments, and `key value` is accepted as well as `key=value`.

The defaults below are the compiled-in defaults. The shipped
`coke_config.ini` overrides a few of them; where that matters the shipped
value is called out.

## Common settings

```ini
[display]
width=0            ; follow the window (exact picking)
height=0
fullscreen=0       ; start framed; F11 toggles at runtime
window_scale=75    ; framed window = 75% of the monitor

[performance]
limit_fps=0
vsync=off
priority=above_normal

[debug]
show_fps=1
verbose=1
```

With `verbose=1`, `coke.log` records startup identity, canvas changes, layout
passes, mouse transforms and FPS. `F9` dumps the live widget tree, read-only.

---

## Profiles

Engine addresses live in `profiles/<exe name>/<build>.ini`, not in the code.
Every install needs a profile, including Combat Mission: Black Sea; the DLL does
not carry one. The proxy picks a profile by build identity (`SizeOfImage` + PE
timestamp + optional `.text` hash). No match means **safe mode**: window, F11,
FPS and mods work, but nothing in the engine is written. Profiles ship in one
drag-and-drop `PROFILES.zip`, which the proxy extracts to `profiles/<exe name>/`
on startup.

### `profile`

- **Default:** unset (discover by executable name)
- **Aliases:** `build_profile`, `game_profile`

Explicitly select a profile file or a folder under `profiles/`.

```ini
profile=CM Cold War.exe/gog-32.ini
profile=none          ; force safe mode (useful for bug reports)
```

A bare name is looked up as `profiles/<name>/` first, then
`profiles/<name>.ini`. A value containing a path separator is opened directly.
Build identity is still checked; the name only decides which file to read.

### `profile_dump`

- **Default:** `0`
- **Aliases:** `dump_profile`

Log the full build identity once at startup: module base, live `SizeOfImage`,
PE timestamp, `.text` FNV-1a, the selected profile and its blocks, slot RVAs and
the resolved RTTI vtables. Use this to author a profile for a new build. Works in
safe mode.

These live values are the match fields, and they are not the same as the on-disk
PE header: the loader rewrites the header in memory. On the Cold War GOG build
the on-disk timestamp is `0x00555c00`, the live value is `0x68d02464`, and the
profile must carry the live one. See
[PORTING.md](PORTING.md#2-identify-the-build) section 2.

```ini
profile_dump=1
```

### `dump_module`

- **Default:** `0`
- **Aliases:** `dump_image`

Write the live main module to `analysis\<exe name>-image-pe.bin` once, after the
packed game image has unpacked. The section headers are rewritten to a file
layout, so a decompiler imports the result as a normal PE. The file contains the
game's code: keep it out of the repository. See
[DECOMPILING.md](DECOMPILING.md).

```ini
dump_module=1
```

---

## Hotkeys

The engine's `Data/hotkeys.txt` cannot bind Function keys or modifiers, so the
proxy polls `GetAsyncKeyState` and owns the key map. F1-F12 are reserved for
debug-style mods; the game's letter hotkeys are untouched.

| Key | Owner | Setting |
|---|---|---|
| F1-F6 | free for debug-style mods | - |
| F7 | `coke-prof`: reset counters | - |
| F8 | `coke-prof`: toggle HUD | - |
| F9 | proxy: dump the live widget tree | `widget_dump` |
| F10 | proxy FPS counter and `coke-fps` | `fps_hotkey` |
| F11 | proxy: framed <-> borderless | `hook_mode` |
| F12 | free | - |

A mod registers an F1-F12 key through `CokeHost.register_hotkey` (API v4). The
host handles polling, focus checks and rising-edge de-duplication, so one press
fires each registered callback exactly once. F9 and F11 are host-owned and are
rejected by the bus. Several mods may share a key; the FPS counter and
`coke-fps` both default to F10 by design. Set `fps_hotkey=off` to disable only
the core counter's key.

---

## `[display]`

### `width` / `height`

- **Default:** `0` / `0`
- **Aliases:** `resolution_x`, `resolution_y`

The logical resolution: the size the engine believes its canvas is.

`0` means **follow the window**. The engine is told the real client size, renders
natively at that size, and no scaling is applied. This is the configuration
where unit picking stays exact at any window size.

Set both to pin a fixed internal resolution. The engine then always renders at
that size and the image is scaled to the window. This is cheaper on the GPU but
softer, and clicks can drift because the engine canvas no longer matches the
real client surface.

```ini
width=0
height=0
```

### `fullscreen`

- **Default:** `0`
- **Aliases:** `fullscreen_at_start`; the old ready-made key `windowed` is the
  inverse

Start borderless fullscreen instead of a framed window. F11 toggles between the
two modes at runtime regardless of this setting.

```ini
fullscreen=0
```

### `window_scale`

- **Default:** `75`
- **Aliases:** `window_size`, `windowed_scale`
- **Range:** 25–100 (percent; a trailing `%` is accepted)

Framed window size as a percentage of the **monitor**, not of the logical
resolution. `75` gives a 1920×1080 window on a 2560×1440 monitor. The framed
window may be clamped by the work area or taskbar. Only applies when the window
is first created; afterwards the last framed size is remembered across F11
toggles. Ignored in fullscreen mode.

```ini
window_scale=75
```

### `fullscreen_inset`

- **Default:** `1`
- **Aliases:** `fullscreen_border`
- **Range:** 0–16 pixels

Pixels removed from the borderless fullscreen window. A window that covers the
monitor exactly is classified by Windows as a fullscreen app, and switching in
or out can blank some displays for a moment. The gap is not visible.

```ini
fullscreen_inset=1
```

### `fullscreen_inset_edge`

- **Default:** `top`
- **Aliases:** `fullscreen_inset_side`, `fullscreen_border_edge`
- **Values:** `top`, `bottom`, `left`, `right`, `right_bottom` (`both` and
  `bottom_right` are accepted aliases)

Which edge gives up the inset pixels. `top` keeps the bottom edge flush with
the screen, so a click at the very bottom cannot fall through to the taskbar.

```ini
fullscreen_inset_edge=top
```

### `stretch`

- **Built-in default:** `1` (fill)
- **Shipped config:** `0` (keep aspect)
- **Aliases:** `fill`; the inverse key `keep_aspect`

Only applies when `width`/`height` pin a fixed logical resolution. With the
default follow-window mode there is nothing to scale.

- `1`: fill the window completely (may distort)
- `0`: keep the aspect ratio and add black bars if needed

```ini
stretch=0
```

---

## `[performance]`

### `limit_fps`

- **Default:** `0` (uncapped)
- **Aliases:** `fps`, `fps_limit`, `max_fps`

Cap the frame rate. The engine otherwise renders menus at several hundred fps.
Common values are `60`, `120`, `144` and `240`.

The software limiter runs only while `vsync` is left at `auto`. Forcing
`vsync=on` or `vsync=off` disables the limiter.

```ini
limit_fps=0
```

### `vsync`

- **Built-in default:** `auto`
- **Shipped config:** `off`
- **Values:** `on`, `off`, `auto`

Force VSync on or off, or leave the engine's own setting alone with `auto`
(the engine normally asks for off).

```ini
vsync=off
```

### `priority`

- **Built-in default:** leave alone (`unset`)
- **Shipped config:** `above_normal`
- **Aliases:** `process_priority`
- **Values:** `idle`, `below_normal` (`below`), `normal`, `above_normal`
  (`above`), `high`, `unset` (leave the process class alone)

Windows process priority class. The engine already asks for `above_normal`.
`high` can reduce micro-stutters when other applications compete for CPU time.
`unset` is the built-in state: the proxy does not change the class at all.

```ini
priority=above_normal
```

### `affinity`

- **Default:** `all`
- **Aliases:** `cpu_affinity`
- **Values:** `all` (also `*`), a hex mask (`0x3`), a core index (`2`), a range
  (`0-3,6`) or a comma-separated list

Pin the game process to specific CPU cores. `all` leaves Windows scheduler
decisions alone. This is almost never needed; use it only to reserve cores for
other workloads.

```ini
affinity=all
```

### `boost_workers`

- **Default:** `0`
- **Aliases:** `boost_threads`

Raise background threads that the engine left at `IDLE` priority to
`BELOW_NORMAL`. On the verified CMBS build the engine already sets its threads
to `ABOVE_NORMAL`/`TIME_CRITICAL`, so this has no measured effect.

```ini
boost_workers=0
```

---

## `[compat]`

### `fix_alt_tab`

- **Default:** `1`

Suppress the engine's minimise-on-focus-loss. Without it the OpenGL front
buffer becomes undefined while minimised and alt-tabbing back shows a black
flash. Deliberate taskbar minimises still work.

```ini
fix_alt_tab=1
```

### `freeze_pan_while_dragging`

- **Default:** `1`
- **Aliases:** `drag_pan_fix`, `freeze_pan_on_drag`

The engine polls the cursor every frame and pans the camera when it reaches the
client edge. Moving or resizing the window puts the real cursor on the window
frame, which is the client edge, so the battle camera pans for the whole
gesture. While `WM_ENTERSIZEMOVE` is active this setting reports the last cursor
position seen inside the client area instead, clamped away from the edges, so
the camera stays still until the drag ends. `0` restores the old behaviour; the
engine's own edge-scrolling toggle (ALT+E in CM) is unaffected.

```ini
freeze_pan_while_dragging=1
```

### `resize_border`

- **Default:** `10`
- **Aliases:** `drag_border`, `resize_grip`

Pixels claimed for resizing just **inside** each client edge. The proxy answers
`WM_NCHITTEST` with the matching `HTLEFT`/`HTTOPLEFT`/… code for those pixels, so
Windows runs its own resize loop (live resize, minimum size, resize cursor) and
the effective border is much fatter than the thin native frame. `0` leaves only
the native frame. Framed windows only: borderless F11 input is untouched.

Combat Mission's battle HUD reaches the window edges, so the outermost 10 px of
the HUD resize instead of clicking. Lower the value (or use `0`) if that
interferes with the command panel.

```ini
resize_border=10
```

### `resize_modifier`

- **Default:** `ctrl_shift`
- **Aliases:** `resize_key`
- **Values:** `ctrl_shift`, `ctrl`, `shift`, `alt`, `none`

While this modifier is held, the whole game view acts as a resize handle: the
left/right/top/bottom third resizes that edge (corners resize both), and the
centre uses the nearest edge. The normal resize cursor appears and Windows does
the resize; it never moves the window. `none` disables the whole-view gesture
and leaves only `resize_border`.

The default avoids Alt because Alt is a Windows menu key (Alt+Space, Alt+F4,
Alt+Tab) and its key-up opens the window menu. Ctrl+Shift has no Windows
binding. The gesture does not run in borderless fullscreen.

```ini
resize_modifier=ctrl_shift
```

### `check_updates`

- **Default:** `ask`
- **Aliases:** `update_check`
- **Values:** `ask`, `on`, `off`

Controls the release check. `ask` shows a yes/no prompt on the first launch
(**No** preselected) and remembers the answer in the `[state]` section of
`coke_config.ini`; declining means no network traffic until the setting changes.
`on` checks without asking. `off` disables the check entirely.

The prompt is a separate top-level window with its own taskbar button and
Alt+Tab entry. Clicking the game leaves it open instead of discarding it, so you
can come back to it. Only the close box or Escape dismisses it; that records
nothing, so the question returns on the next launch.

With consent, a background thread requests the latest public GitHub release
once per game launch, using WinHTTP and the system certificate store. When
automatic discovery has no matching profile, Coke also checks that release's
`PROFILES.zip` in memory. It compares the executable name and build identity
locally. **Download profiles** opens the archive in your browser if it matches
the game and belongs to your current Coke version.

For a newer version, **Open release** opens that version's page in your default
browser, **Ignore this version** records the tag, and **Never check again**
disables future checks. Installation is manual. Dismissing the notice leaves
the choice for later.

The `[state]` section is written by the proxy and safe to delete; deleting it
makes the consent prompt appear again. It holds only `update_consent`,
`update_disabled`, `ignored_update` and `notified_update`.
`deploy.ps1` preserves the section when it installs a new config, so a
deployment never resets your answer. An old standalone `coke_state.ini` is
migrated into the section once and then removed.

```ini
check_updates=ask
```

### `scale_fixes`

- **Default:** `1`
- **Aliases:** `scale`

Master switch for the logical→physical coordinate mapping. Set to `0` only to
debug the mod itself: the image then appears in the bottom-left corner at the
logical size with no scaling.

```ini
scale_fixes=1
```

### `promote_scene`

- **Default:** `0`
- **Aliases:** `promote`, `scene_promote`

Stretch the in-game 3D scene sub-rectangle to the full logical canvas. Leave it
off: it changes what is drawn but not what the engine hit-tests, so unit
selection lands away from the cursor. The correct fix is to make
`width`/`height` match the engine's canvas.

```ini
promote_scene=0
```

### `canvas_patch`

- **Default:** `1`
- **Aliases:** `engine_canvas`, `patch_canvas`

Write the new client size into the engine's cached canvas after a resize, F11
toggle or maximize. The engine reads that cached pair for UI layout and for its
mouse Y inversion; its window procedure ignores `WM_SIZE`, so posting
`WM_SIZE` cannot relayout it. This is what makes maximize and F11 use the new
size instead of drawing the startup size in the corner.

Requires a profile with a `[canvas]` block. Without one (or without a matching
build identity) the proxy logs `canvas patch: profile has no [canvas] block` or
`profile: safe mode` and keeps the old behaviour. Ignored when
`width`/`height` pin a fixed canvas.

```ini
canvas_patch=1
```

### `stale_viewport_fix`

- **Default:** `1`
- **Aliases:** `fix_stale_viewport`

Expand a full-screen viewport that still uses the startup canvas (for example
`(0,0,1920,1080)` after the window grows). Removes black areas outside the old
canvas. Set to `0` if a menu or background pass looks stretched.

```ini
stale_viewport_fix=1
```

### `scene_rect_fix`

- **Default:** `1`
- **Aliases:** `fix_scene_rect`, `scene_offset_fix`

Repair the engine's battle-scene rect after a canvas change. The engine lays the
scene out at `y=0` instead of below the command bar and derives both the drawn
viewport and click-to-unit picking from that rect. The repair finds the render
target and its scene widget in memory and corrects their `y` to the inset, so
drawing and picking move together.

This does not require a `[scene]` block: the repair locates the render target and
its linked widget by a heap scan and a write/observe probe. A profile with
`[canvas]` alone still gets scene repair and correct picking. `[scene]` only adds
a stable scene widget for the battle layout override, the `scene_rect` mod API
and `cokelos`.

Set to `0` if the scene is drawn in the wrong place.

```ini
scene_rect_fix=1
```

### `scene_vp_rewrite`

- **Default:** `0`
- **Aliases:** `rewrite_scene_vp`

Legacy fallback: rewrite only the drawn viewport
(`(0,0,w,h-inset)` → `(0,inset,w,h-inset)`). The engine keeps picking with its
own un-rewritten rect, so every unit click lands `inset` pixels off. Keep this
at `0`; it exists only to compare the two approaches.

```ini
scene_vp_rewrite=0
```

### `scene_inset`

- **Default:** `0` (learn at runtime)
- **Aliases:** `scene_top_inset`
- **Range:** 0–480 logical pixels

Top inset reserved above the battle scene for the command bar. `0` learns it
from a battle frame drawn at the startup canvas and falls back to the engine's
`155`. Override only if the scene band is wrong on a build whose constant
differs.

```ini
scene_inset=0
```

### `menu_recenter`

- **Built-in default:** `1`
- **Shipped config:** `0`
- **Aliases:** `recenter_ui`, `widget_recenter`

Re-center UI widgets that were laid out for the startup or previous canvas
after the canvas changes (menu panel, dialogs, the alt+Q quit dialog). This
repairs the engine's own widget rects, so drawing and mouse hit-testing move
together. It runs incrementally on the watcher thread.

```ini
menu_recenter=0
```

### `widget_dump`

- **Default:** `1`
- **Aliases:** `dump_widgets`

Diagnostic: dump the live widget tree, read-only, to `coke.log`. Runs about
0.9 s after each canvas change, on the first battle frame after a change, and on
F9. Set to `0` to keep the log small.

```ini
widget_dump=1
```

### `widget_relayout`

- **Default:** `1`
- **Aliases:** `relayout_ui`, `native_relayout`

Native UI re-layout after a resize, F11 or maximize: replay the engine's own
widget rects against the new canvas using the layout rules recovered from the
binary (scene inset 155, battle bar flanks `(canvas_w - 1024) / 2`, menus
centred).

- `0`: off (the picture stays at the startup layout)
- `1`: apply
- `2`: dry-run: log the planned rects, write nothing (`dry`, `dry_run` and
  `dry-run` are accepted aliases)

```ini
widget_relayout=1
```

### `relayout_delay_ms`

- **Default:** `30`
- **Aliases:** `relayout_delay`
- **Range:** 0–2000 ms

Delay between the canvas change and the re-layout pass. A couple of frames is
enough because the engine does not rebuild a screen on resize. The watcher polls
every 2 ms while a re-layout is armed.

```ini
relayout_delay_ms=30
```

### `hook_slots`

- **Default:** `1`

Patch the engine's in-memory user32 import slots for `GetSystemMetrics`,
`GetCursorPos`, `ChangeDisplaySettingsA` and `CloseWindow`. These patches make
the logical resolution, cursor mapping, alt-tab fix and display-mode
suppression work. Disable only to isolate a crash.

```ini
hook_slots=1
```

### `hook_cbt`

- **Default:** `1`
- **Aliases:** `cbt_hook`

Install a `WH_CBT` hook on the engine's threads so `HCBT_CREATEWND` can be
rewritten. This creates the game window at its final size instead of resizing
it after the fact. Without it the window still gets the right size, just a
moment later.

```ini
hook_cbt=1
```

### `hook_window`

- **Default:** `1`

Find the game's main window, apply the initial window size and mode, and enable
F11 toggling. If off, the window stays exactly as the engine creates it and F11
does nothing.

```ini
hook_window=1
```

### `hook_wndproc`

- **Default:** `1`

Subclass the engine's window procedure to intercept maximize, close, restore,
minimize and mouse messages. Required for the working close and maximize
buttons and for correct cursor placement when the window size differs from the
logical resolution.

```ini
hook_wndproc=1
```

### `forced_exit`

Controls how the subclassed window procedure handles the close box and system
close command. `forced_exit=1` preserves Coke's legacy immediate termination;
`forced_exit=0` forwards the close message to the game's normal shutdown path,
which allows orderly cleanup and persistence.

```ini
forced_exit=0
```

### `hook_mode`

- **Default:** `1`

Apply the initial window size from `window_scale`/`fullscreen` when the game
window first appears, and enable F11 toggling.

```ini
hook_mode=1
```

---

## `[debug]`

### `verbose`

- **Default:** `1`

Log the GL renderer string, new viewport rectangles, FPS every 2 seconds and
window geometry changes to `coke.log` next to the EXE.

```ini
verbose=1
```

### `show_fps`

- **Default:** `1`
- **Aliases:** `fps_overlay`, `fps_counter`

Draw a yellow FPS counter in the top-right corner. The value updates every
2 seconds so it does not flicker every frame.

```ini
show_fps=1
```

### `fps_hotkey`

- **Default:** `F10`
- **Aliases:** `fps_key`, `fps_toggle`
- **Values:** `F1`–`F12` (or `1`–`12`), `off` (also `none` or `0`)

Function key that toggles the core FPS counter at runtime. It works without a
restart or a config edit. Only F1-F12 are accepted; F9 and F11 are host-owned
and rejected, and the key map is in [Hotkeys](#hotkeys). Set `off` to leave the
counter under `show_fps` control only.

```ini
fps_hotkey=F10
```

### `range_terrain_overlay_test`

- **Default:** `1` in the shipped config (`0` in the compiled fallback)
- **Values:** `0` or `1`
- **Restart required:** yes

Enable the CM:BS terrain painter in CokeRange. With the matching Black Sea
profile, aim at a visible surface and press `Ctrl+Shift+T`. The painter uses
the battle depth buffer to pick an observer and queries native visibility for
the entire playable map in 16 m cells, with 8 m detail near the observer.
Very large maps use coarser cells to stay within the scan limit.
Visible ground is green and blocked ground is red at both terrain detail
levels. Press the combination again to clear the paint. See the
[CM:BS title note](titles/black-sea.md#deployment-tint-and-terrain-overlay-prototype).

### `los_flip_x` / `los_flip_y`

- **Default:** `0` / `0`
- **Aliases:** `cokelos_flip_x`, `cokelos_flip_y`

Mirror the `cokelos` world-to-screen projection on the given axis. Only useful
if a build or modded camera uses the opposite handedness.

```ini
los_flip_x=0
los_flip_y=0
```

### `los_scene_rect`

- **Default:** `0`
- **Aliases:** `cokelos_scene_rect`

Project `cokelos` against the scene widget rect instead of the full window. On
the verified build the 3D viewport is the whole window, so keep this off.

```ini
los_scene_rect=0
```

### `los_clip_bottom_pct`

- **Default:** `17`
- **Aliases:** `los_clip_bottom`
- **Range:** 0–40 percent

Bottom percentage of the window treated as HUD and clipped out of the
`cokelos` overlay, so it never paints over the command panel. `0` disables the
clip.

```ini
los_clip_bottom_pct=17
```

---

## CokeRange LOS paint

LOS terrain painting is integrated into `coke_range.dll`; the release bundle
no longer contains a separate `cokelos.dll`. The CokeRange consent file gates
both the range preview and painter. `range_terrain_overlay_test` enables the
matched-build terrain draw hook; `Ctrl+Shift+T` toggles the combined cursor
terrain paint. The scan uses the game's observer and target postures. It
samples at 16 m across the map and 8 m within 192 m of the observer, subject
to the scan limit on large maps.
