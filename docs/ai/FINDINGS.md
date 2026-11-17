# FINDINGS: Combat Mission engine notes

This file is the agent-facing map of the project for every Combat Mission
title: the engine model, the per-title anchors, and how to extend the mod
without breaking buying/clicking. Combat Mission: Black Sea (Steam 32-bit) is
the reference build where the model was first verified. Shock Force 2 (GOG
32-bit) is a second verified port. Cold War (GOG 32-bit) is a third verified
port that resolves its battle scene through `[scene] widget_class` rather than
a static owner global. Facts marked CMBS, SF2 or CW are specific to that build;
unmarked facts describe the CMBS reference build until a per-title note says
otherwise.

Read [INSTALL.md](../user/INSTALL.md) first for the player-facing install, [CONFIG.md](CONFIG.md) for every setting,
[DECOMPILING.md](DECOMPILING.md) for the legal/reproducible analysis workflow and
[PORTING.md](PORTING.md) for another title. Addresses below are for the verified
32-bit builds unless marked otherwise. When in doubt, verify in the running
process instead of guessing.

## Working on the series

- All three titles share one engine codebase and the same data formats. The
  object model, widget layout, scene/picking path and LOS data described below
  carry over; the addresses and a few constants do not.
- CMBS is packed, has no relocations and always loads at `0x00400000`. SF2 and
  Cold War are unpacked with ASLR, and SF2's protector rebases the live image.
  Build identity is profile data, not a constant.
- Every install is build-specific: one profile matches one executable. A patch
  changes the identity and the old profile falls back to safe mode. Add a
  profile per build; never reuse RVAs across builds.
- The launcher and the DirectShow intro are not the engine. If the title has
  them, start them manually, wait for the engine window, then attach or profile.
  Many installs (Cold War GOG, for example) start directly from the exe; use that
  when it works.
- Work through a profile and the safety rules in this file: validate every
  read, re-validate before every write, and prefer RTTI identity over geometry.
  Safe mode is the correct fallback when a fact is missing.
- [DECOMPILING.md](DECOMPILING.md) owns the tooling and the text/artifact split;
  [PORTING.md](PORTING.md) owns the per-build procedure. Everything below is the
  current map they are applied against.

## Decompiling the game

You need your own legitimate installation of the game, plus REA
(<https://github.com/morluto/rea>) and Ghidra 11.x/12.x. The full workflow, the
build-identity checks and the contribution rules are in
[DECOMPILING.md](DECOMPILING.md).

Two rules matter for this repository:

1. **Do not commit decompiled output.** Ghidra `.c` files, decompiler JSONL,
   memory dumps, Ghidra/IDA projects and extracted `.brz` assets are the game's
   copyrighted code/data, not "notes". Keep them outside the repo.
2. **Publish facts, not code bodies.** Offsets, RTTI names, struct layouts,
   function signatures, profile files and your own prose explanations are
   welcome. Short excerpts for a specific point are acceptable; a full function
   dump is not.

A local analysis tree can live in `analysis/` (already gitignored).

For supporting another title, see [PORTING.md](PORTING.md). The short version:
the generic IAT/RTTI paths work, but the cached canvas and scene rect need a
build-specific profile or a validated signature scan.


## Operating notes

- The Steam launcher ignores synthetic mouse input. Start it manually, press
  Play, and wait for the DirectShow intro (about 3 seconds) before attaching to
  the engine process. Do not attach to the launcher or the intro. If the install
  runs directly from the exe, start that instead; the launcher is optional when
  the engine starts on its own.
- Use REA plus Ghidra or another decompiler. Guessing addresses does not work;
  verify every pointer and offset in the live process.
- An OpenGL-only performance experiment yielded roughly +10 to +30 FPS, but it
  was unstable and machine-dependent. Multithreading the engine is the
  structural fix, not more GL tweaks.
- Deploy the built DLL and config to the game folder with a script. Manual
  copying is error-prone and easy to forget between builds.
- Keep the working tree in git. Layout, scene and rendering experiments break
  often; a clean revert point is safer than trying to unwind a failed attempt.
- When a change needs a specific keybind or edge case tested in-game, list the
  exact steps and expected result.

---

## 0. Project state

Implemented and working:

- Windowed mode, resizable window, maximize, force-quit close and the
  borderless F11 toggle.
- Alt-tab without the black flash (the engine's minimise on focus loss is
  suppressed).
- The engine canvas follows the client size (`canvas_patch`), including the
  engine's own mouse Y math.
- Native UI re-layout after a resize/F11/maximize (`widget_relayout`).
- Stale viewport and battle-scene rect repairs.
- FPS overlay, frame cap, forced VSync, process priority/affinity tuning.
- The consent-gated `coke/` native mod loader.
- API v4 two-phase overlay: world-space `coke_frame`, then 2D `coke_hud`, so a
  world overlay cannot cover the FPS/status text, plus the host-owned F1-F12
  hotkey bus and per-mod draw layers.
- `mods/coke-prof`: frame/thread/GL-call profiler plus optional EIP sampling.

Support matrix (one row per profile; see `profiles/README.md`):

| Build | Profile | Window/F11 | Canvas patch | UI re-layout | Scene rect | FPS/mods |
|---|---|---|---|---|---|---|
| CM Black Sea, Steam 32-bit | shipped `steam-32` (verified) | yes | yes | yes | yes | yes |
| CM Shock Force 2, GOG 32-bit | `gog-32` (verified) | yes | yes | yes | yes | yes |
| CM Cold War, GOG 32-bit | `gog-32` (verified) | yes | yes | yes | yes | yes |
| any other build | none -> safe mode | yes | no | no | no | yes |

A `partial` profile is a valid, honest result: the proxy logs
`profile: ... status=partial`, lists the missing blocks, and writes nothing to
the engine.

Scene rect repair is not tied to `[scene]`. `scene_rect_fix` scans the heap for
the render target and its linked widget and repairs the rect with a
write/observe probe, so it works with `[canvas]` alone. `[scene]` supplies a
named, stable scene widget for the battle layout override, the `scene_rect` mod
API and `cokelos`; a profile without it still gets correct resizing and picking.

Port notes for the two non-CMBS rows live in
[titles/shock-force-2.md](titles/shock-force-2.md) (Shock Force 2) and
[titles/cold-war.md](titles/cold-war.md) (Cold War). Those pages record the
title's identity fields, profile anchors, the bugs fixed during the port and the
live evidence behind each `yes`.

Current known problems, in priority order:

1. **Poor performance from the engine's single-thread lock.** The game's
   render/UI/simulation work is effectively serialized on one thread and the
   renderer is CPU-heavy. The proxy does not move rendering to the GPU.
2. **`cokelos` is broken.** It can produce stuck frames while the camera moves,
   can spill outside the world bounds, and its whole-map scan is expensive
   enough to contribute to crashes. It is disabled by default and needs
   additional RE (terrain handing, projection, object occluders).
3. Mid-battle resizes still have layout limits: the engine has no runtime
   battle re-layout, and some screen geometry is frozen at battle entry.
4. **DPI scaling is not handled.** The process inherits whatever DPI
   awareness the game manifest requests; the proxy neither sets nor queries it.
   On a display scaled above 100% the window is correct but the frame may be
   bitmap-stretched by Windows (blurry) or, if the game is per-monitor aware,
   the logical size and the client size can disagree after a monitor change.
   See §4.
5. **Force-quit on `WM_CLOSE`/`SC_CLOSE` skips the engine's shutdown path.**
   It fixes a hang the engine has when closing, but it also bypasses any
   exit-time save, dialog or cleanup. Treat unsaved progress as lost on close.

---

## 1. Agent workflow

### Build the proxy

```powershell
rustup target add i686-pc-windows-msvc
cargo build --release --target i686-pc-windows-msvc
# output: target\i686-pc-windows-msvc\release\opengl32.dll
```

Copy `opengl32.dll`, `coke_config.ini` and `PROFILES.zip` next to
`CM Black Sea.exe`; the proxy extracts the profiles on startup. Delete the DLL,
config, zip and `profiles\` to uninstall. The game files are never modified.

### Build a `coke/` mod

Each mod lives under `mods/` and is a `cdylib`. Build it for the same 32-bit
target, then install:

```text
coke\your_mod.dll
coke\user_consent_your_mod.txt
```

The consent file is a deliberate second step; only its existence is checked.
The loader never scans subfolders and never modifies the EXE.

### Inspect a run

Set `verbose=1` in `coke_config.ini`. The log is `coke.log` next to the
EXE. Useful lines:

- startup identity: `module base=... image_size=...`, `Coke <version> loaded`
- config echo: `config: logical=...`
- canvas changes: `canvas patch: ...`
- raw engine viewports: `frame vps: ...`
- layout pass: `widget recenter:`, `widget relayout:`
- scene repair: `scene rect fix: ...`
- input: `mouse ...`, `click ...`, `GetCursorPos ...`
- FPS: `fps: ...`

`F9` dumps the live widget tree. `Ctrl+Shift+D` in `cokelos` dumps LOS/map
data. Both dumps are read-only.

### Iteration rules

- Never dereference engine pointers directly from a background thread. Use
  `ReadProcessMemory` (the code has `rd_u32`/`rd_f32`/`rd_bytes`) so a freed
  page fails cleanly instead of crashing the game.
- Before writing engine memory, re-check that the page is committed and
  writable. `writable()` in `src/widgets.rs` is the reference implementation.
- Prefer object identity (RTTI class name, vtable, parent backlink) over
  geometry. Rectangles are a consequence of layout, not a stable identity.
- Start the proxy early. It is loaded before the packer builds the real import
  table, which is why a drop-in `opengl32.dll` can hook Win32 calls at all.
- Test both menu and battle. Many layout bugs only appear after entering a
  battle, and many picking bugs only appear after a resize.

### Re-finding addresses on another build

Fixed addresses are safe only when the module base and image size match. The
proxy already validates this and falls back to an IAT scan; new engine-object
work should do the same. Useful stable anchors:

- RTTI names: `View`, `GameView`, `CommandView`, `UnitView`, `SoldierView`,
  `CommandButtonPanel`, `MapView`, `Camera`.
- Window class `CM Black Sea`.
- The engine's `CreateWindowExA` call reads `GetSystemMetrics(0/1)` directly.
- The battle screen layout constants `0x400` (1024), `0x300` (768), `0x9b`
  (155) and `0xa4` (164).
- The canvas lives at the start of the app object; the app pointer is in a
  single global.

`src/rtti.rs` resolves any class vtable by scanning the unpacked image for the
mangled type name, so a build that only moves addresses keeps working as long
as the RTTI names survive.

To get the image for a static tool, set `dump_module=1` once. The proxy writes
`analysis\<exe name>-image-pe.bin`, a PE-layout copy of the live (unpacked)
module. See [DECOMPILING.md](DECOMPILING.md) for the import and analysis steps.

---

## 2. What ships where

- `CM Black Sea.exe` contains the entire game: engine, renderer, UI and
  simulation. Data files contain no executable code.
- `Data/**/*.brz` contains assets only: models, textures, sounds, scenarios and
  UI art.
- `Data/Z/**` is the user-mod overlay for images, sounds, models and scenarios.
- `opengl32.dll` + `coke_config.ini` are this proxy.
- `coke.log` is the proxy log.
- `display size.txt` is a small file with `0 0 0`; the proxy does not read it.

---

## 3. Binary and process model

- 32-bit x86, packed. The real user32/OpenGL import table is built in memory at
  runtime, so there is no useful static IAT before the packer runs.
- Preferred base `0x00400000`. The packed header advertises
  `SizeOfImage = 0xa5f000`, and there are no relocations, so the unpacked image
  always loads at that base.
- The live image is larger than the header says: writable `SECTION.2` spans
  `0xbf0000`–`0xdc0600`, so engine globals above `0xa5f000` are valid.
- `CM Black Sea.exe` is loaded first, then the packer resolves imports. Because
  `opengl32.dll` is already mapped, the proxy can patch the slots as they are
  filled.
- Startup creates a full-monitor window, destroys it, and creates the real one.
  Do not subclass until `GetWindowLongW(GWLP_WNDPROC)` returns an executable
  pointer; USER32 thunks (`0xffffxxxx`) appear first.

**Other titles.** SF2 and Cold War are not packed the same way as CMBS. They are
unpacked and ASLR-enabled, and SF2's protector rebases the live image to an
arbitrary base (for example `0x00ef0000`) and rewrites the in-memory
`ImageBase`. Static RVAs are still correct, but the live `.text` hash differs
from the on-disk hash. Match those builds with the relocation-invariant hash:
record the on-disk preferred base in the profile, read the loader base at
runtime, normalise every relocated dword back to preferred-base form, then hash
(`src/engine.rs`). Never weaken the profile by comparing a raw live hash.

Cold War also has its loader rewrite the in-memory PE `TimeDateStamp`: the
verified GOG build reads `0x00555c00` on disk and `0x68d02464` live, and the
profile matches the live value. Treat the on-disk PE header as untrustworthy for
all three identity fields (`image_size`, `pe_timestamp`, `text_fnv1a`); only the
`profile dump:` log line is authoritative. See `PORTING.md` section 2.

---

## 4. Window and display model

- The engine creates its OpenGL window in `FUN_00618D20` with
  `CreateWindowExA(..., 0x96000000, x, y, GetSystemMetrics(0), GetSystemMetrics(1), ...)`.
  That is the only place it reads the screen size for window creation.
- Window class: `CM Black Sea`. Window procedure: `LAB_00617AB0`, registered by
  `FUN_00618D20`.
- The engine handles only `WM_CLOSE`, `WM_ACTIVATEAPP`, `WM_KEYDOWN`, `WM_CHAR`,
  `WM_SYSCHAR`, the mouse buttons/wheel, and returns 0 for `WM_MOUSEMOVE`.
  Everything else goes to `DefWindowProcA`.
- The engine does **not** handle `WM_SIZE`, `WM_MOVE`, `WM_DISPLAYCHANGE`,
  `WM_WINDOWPOSCHANGED` or `WM_GETMINMAXINFO`. Posting `WM_SIZE` cannot relayout
  the UI.
- The engine imports no `SetWindowPos`, `MoveWindow`, `GetClientRect` or
  `GetWindowRect`. It cannot resize or query its window after creation.
- The engine calls `ChangeDisplaySettingsA`; the proxy suppresses it so the
  window owns the display mode.
- On focus loss the engine minimises its GL window (`CloseWindow`/
  `ShowWindow`). A minimised OpenGL front buffer is undefined, which is the
  black flash on alt-tab. The proxy suppresses only the engine's minimise.
- `WM_CLOSE`/`SC_CLOSE` do not terminate the game promptly; the proxy
  force-quits.
- A framed restore can report `GetWindowRect = (-8,-8)`. That is the DWM
  invisible resize border, not necessarily an off-screen window; the visible
  frame is effectively at `(0,0)`.

### DPI scaling

The proxy works in physical client pixels and never calls
`SetProcessDpiAwareness*`, so the result depends on the game's manifest:

- **System-DPI aware / unaware:** Windows virtualises coordinates. `GetClientRect`
  and the GL viewport agree, so the mod is consistent, but the whole window is
  bitmap-stretched and looks soft. Nothing breaks.
- **Per-monitor aware:** `GetClientRect` is real pixels and the frame is crisp.
  After dragging to a monitor with different scaling, the engine canvas must be
  re-adopted; the watcher does this through `keep_engine_canvas_in_sync`, but the
  first frame after the move can show a stale layout.

An opt-in DPI fix would call `SetProcessDpiAwarenessContext` early in `init()`
(`opengl32.dll` is loaded before the first window exists, so that is a valid
place). It is deliberately not enabled yet: forcing awareness changes the UI
scale the user sees and needs testing on 125/150/200% displays.

### Canvas cache

The canvas the engine lays its UI out in is a cached pair in the app object:

- `APP_PTR = 0x00d764b4` points at the app object (allocated by
  `FUN_00700F10`; stored at `0x701197`).
- `app + 0x00` = canvas width, `app + 0x04` = canvas height.
- They are written once from `GetSystemMetrics(0/1)` at `0x70119D`/`0x7011A3`
  and never re-read by the engine.
- `app + 0xbac` is the engine HWND; used as a sanity check before trusting the
  pointer.
- `canvas_patch` writes the new client size into `app[0]`/`app[4]`. This is the
  only way the engine's own layout and mouse math see a resize.

UI layout is constructed from this pair at screen-construction time:

- `FUN_005A63C0` at `0x5A6FE0`: `w = *app; if (w > 0x400) { extra = w - 0x400; ... }`.
- `FUN_00550430` at `0x55043E`: centres a dialog with `app[0]/2`, `app[1]/2`.
- `FUN_005865F0` at `0x5865F5`: `if (*app > 0x4ff) ...`.
- Existing widgets do not reflow on resize. Changing `app[0]/[4]` affects
  screens constructed after the write and code that re-reads the pair.

The intro is DirectShow 640×480 and is blitted at the centre of the engine
canvas outside GL. It is centred only when canvas == client. Do not expect a
canvas change to move it; hook the video path instead.

---

## 5. UI widget model

- The root widget is `app + 0x34`, created once by `FUN_00700F10` through
  `FUN_007990A0(0, 0, 0, *app, app[1], ...)`, so its rect is
  `(0, 0, startup_w, startup_h)`. The engine never updates it. This is the
  `(0,0,1920,1080)` full-screen pass.
- Widget layout, verified from the binary and the live tree:

  ```text
  +0x00  vtable
  +0x04  parent pointer
  +0x10  absolute rect: i16 x, y, w, h (top-down y)
  +0x18  flags
  +0x34  alpha
  +0x3c  children vector begin
  +0x40  children vector end
  +0x44  children vector capacity
  ```

  `FUN_007990A0` is thiscall (`ECX = this`, arguments at `[ebp+8..0x24]`).
  Negative x/y mean "centre on the parent"; negative w/h mean "inherit the
  parent's size"; `param_8` clamps into the parent. Rects are absolute pixels.
- `FUN_00799A30` / `FUN_00799BD0` draw a widget by calling
  `glViewport(widget rect)` and building `glOrtho(0..w, 0..h)` from the same
  rect. The same rect drives mouse hit-testing.
- `FUN_0054F4D0` -> `FUN_0054EE90` reads `glGetIntegerv(GL_VIEWPORT)` and
  computes `aspect = vp_w / vp_h` on every projection setup. A corrected scene
  rect therefore also corrects the 3D camera.
- C++ RTTI is intact. Resolve a class with three fault-safe reads:
  `vtable[-4]` -> complete object locator; `col + 0x0c` -> type descriptor;
  `td + 0x08` -> decorated name such as `.?AVCommandView@@`.

### Layout rules recovered from the binary

Battle screen `FUN_007A6B40`:

```text
screen = ctor(parent, 0, 0, -1, -1)                       // fill
scene  = FUN_005D18D0(screen, ..., canvas_w, canvas_h-0x9b) // y = 0x9b = 155
flank  = (canvas_w - 0x400) / 2                           // 0x400 = 1024
left   = ctor(screen, 0, 0, flank, 0x9b)
right  = ctor(screen, canvas_w - flank, 0, flank, 0x9b)
bar    = ctor(screen, flank, 0, 0x400, 0xa4)              // 0xa4 = 164
```

Menus and dialogs: `ctor(parent, -1, -1, 0x400, 0x300)` = centred 1024×768.

The native re-layout identifies roles by RTTI class, not by current geometry:
the screen child that owns a `CommandView` is the bar, the `GameView` is the
scene, and the remaining `View`s are the flanks. The bar's x is the signed
centre `(W - 1024) / 2`, so it cannot be pinned to a corner by a narrow resize.
Menu/dialog children are re-anchored relative to their parent.

`widget_relayout=2` logs the planned `old -> new` rects and writes nothing.
`widget_relayout=0` disables the pass. The pass is scheduled about
`relayout_delay_ms` after a canvas change.

### Reading the F9 widget dump

`widget_dump=1` (the default) and F9 write the live tree to `coke.log`, read-only.
One node per line:

```text
d=1 a=0x13dd6b80 vt=0x00936a08 cls=View      p=0x12907b0
d=2 a=0x28045a98 vt=0x00926340 cls=GameView  p=0x13dd6b80 rect=(0,155,2560,1284)
```

- `d` depth from the app/root object.
- `a` absolute address of the widget. Valid for that run only.
- `vt` vtable VA. Subtract the module base from the same log to get the RVA a
  `[rtti]` entry stores (`0x00936a08 - 0x100000 = 0x00836a08`).
- `cls` RTTI class name, resolved by the proxy.
- `p` parent pointer, the widget field at `+0x04`.
- `rect` absolute `x,y,w,h` in top-down pixels.

Use the dump to choose the `[scene]` anchor:

1. If the module image holds the scene widget's parent pointer, use the static
   chain `owner_ptr -> owner_widget_off -> widget`.
2. Otherwise use `widget_class = <class>`. It requires `[canvas]`, the class must
   own the 3D scene (not a parent `View`), and the name must exist in `[rtti]`.
3. If neither is available, omit `[scene]`. Resize repair still works; only the
   battle layout override's named scene, `scene_rect` and `cokelos` lose their
   anchor.

Before trusting a "current scene" global, dump the tree in a menu, in a battle
and after returning to the menu. A global that changes between those states is
not an anchor.

### `menu_recenter` scan

`src/widgets.rs` uses the same `{begin, end, capacity}` pair at
`+0x3c/+0x40/+0x44` as `src/wtree.rs`. It walks private heap regions, validates
candidates by vtable, parent backlink and children span, then shifts the whole
subtree. The shipped config leaves `menu_recenter=0`; enable it only after
testing the menu, dialogs and the battle HUD at several canvas sizes.

---

## 6. Battle scene and picking

- The battle scene is drawn through a render-target object created by
  `FUN_00724F30` (0x5c bytes, vtable `PTR_FUN_00BA8D64` on the verified build).
  Its rect is `u32 x/y/w/h` at `+0x28..+0x37`; it is copied once at creation
  from the scene widget's rect (`SCENE_OWNER_PTR = 0x00d7a6b4`, widget at
  `owner + 0x20`, i16 rect at widget `+0x10..+0x17`). Later runs only toggle
  `+0x50`. The object has a widget backlink at `+0x54`. It is recreated per
  battle entry.
- `BLUR_QUAD_PTR = 0x00d7a6c8` points at the same object family; the code calls
  it the blur quad.
- After a canvas change the engine has been observed laying the scene out at
  `y=0` (`(0,0,w,h-155)`) and, mid-battle, at intermediate y values such as
  `(0,98,1920,982)`. **Both the drawn viewport and click-to-unit picking derive
  from that rect.** Rewriting only the drawn viewport breaks picking: clicks
  stay correct against the UI but land `inset` pixels off against 3D units.
  The real fix must repair the engine's rect in memory.
- `scene_rect_fix` does that. It finds the render target and/or the scene widget
  in the heap, writes a candidate y, observes the next scene viewport, derives
  the y convention from the slope, then applies the exact correction. Two
  adjacent write/observe pairs are enough for either identity or bottom-up
  layout. A successful fix is propagated to the linked widget/RT pair so
  drawing and picking stay consistent. It does not require a `[scene]` block:
  the block only makes the scene widget resolvable by `scene_widget()` for the
  battle layout override, the `scene_rect` mod API and `cokelos`.
- The engine's y convention between widget rects and GL viewports is **not**
  fully known. Do not replace the feedback loop with a hard-coded transform
  without verifying it on a live battle.
- `scene_vp_rewrite` is the old drawn-viewport-only fix. It is intentionally
  off because it breaks picking.

---

## 7. Input

- Distances: the engine's click/picking path uses raw client pixels and builds
  its projection for the client area. In follow-window mode (scale 1) canvas
  coordinates equal client coordinates.
- Mouse Y is inverted against the cached canvas height in the window procedure:
  `y' = app[1] - (lParam >> 16) - 1` at `0x618182` and `0x6181E3`. X is used
  raw. This is why patching `app[1]` is required for clicks after a height
  change.
- `GetCursorPos` must return **canvas coordinates**. Serving screen coordinates
  breaks hover.
- Wheel messages carry screen coordinates and go through `ScreenToClient`
  first.
- Shift+drag draws the selection rectangle at the engine's idea of the cursor;
  it is a free probe for picking errors.
- The engine's `Data/hotkeys.txt` cannot bind modifiers or function keys. F1-F12
  are reserved for debug-style mods and go through the host hotkey bus (API v4,
  see §10); `cokelos` still polls its `Ctrl+L` combo through `host.key_down`.
  `Ctrl+L` produces `WM_CHAR 0x0C` and does not trigger the game's letter
  hotkeys.

---

## 8. Rendering and frame structure

Per frame, from the viewport log:

1. Bind an offscreen FBO (shadow map), set viewport `(0,0,canvas)`, render the
   3D scene.
2. Bind FBO 0, set viewport to the battle game rectangle, composite.
3. Draw small UI viewports inside that rectangle.

Consequences:

- The shadow FBO is bound only when a 3D scene is rendered. Menus never bind
  one. This is the reliable "in game" signal (`FBO_LAST_TICK`, 2 s window).
- OpenGL viewport coordinates are bottom-left. Widget rects are top-down. Do not
  mix the two conventions.
- The FPS overlay and all mod drawing happen in `wglSwapBuffers`, after the real
  frame is complete and before the swap. Do not draw from `glReadPixels` or
  `glCopyTex*` paths; that produced ghost frames.
- `glViewport`, `glScissor`, `glRasterPos*`, `glReadPixels`,
  `glCopyTexImage2D`, `glCopyTexSubImage2D` and the wgl entry points are
  implemented by the proxy. Everything else is a generated thunk forwarding to
  the real system `opengl32.dll`. `build.rs` and `src/opengl.rs` must stay in
  sync on the intercepted list.

---

## 9. World, terrain, camera and LOS data

Coordinates are `x` east, `y` north, `z` up, in metres.

```text
WORLD_PTR         0x00d7a6ac   global -> world object
world + 0x310                 -> ActionMap (a TerrainMap subclass)
SCENARIO_PTR      0x00d7a6b8   global -> scenario/battle object
MAP_GLOBALS       0x00d74ea4 / 0x00d74ea8 -> live ActionMap
```

`ActionMap` / `TerrainMap`:

```text
+0x04  world width  (f32, map units)
+0x08  world height (f32, map units)
+0x9c  height array pointer
+0xa0  vertex-grid width
+0xa4  vertex-grid height
```

`MapView` (RTTI `.?AVMapView@@`, vtable `0x00b9a5dc`) is the battle view. Look
for it at the view itself or at the model object `+0x48`:

```text
+0x48  map view model
+0x6c  selected-unit pointer vector begin
+0x70  selected-unit pointer vector end
+0xe0  cursor world x
+0xe4  cursor world y (north)
```

`Camera` (RTTI `.?AVCamera@@`, vtable `0x00b98b00`):

```text
+0x08  position
+0x18  direction
+0x28  up
+0x34  fov
+0x38  aspect (not a normal aspect on the verified build; use the viewport)
+0x3c  near
+0x40  far
```

`cokelos` uses the host's `los_scan_map`, which sweeps rays from the cursor
ground point and keeps a running horizon angle. It is terrain-only: tree and
building occluders are not wired in yet. That is why it is labelled `terrain
LOS` and why it must not be presented as the engine's spotting model. See
`analysis/SPOTTING.md` for the target/LOS data model and the `T` command.

Known LOS problems:

- Projection/orientation needs more RE; `los_flip_x`/`los_flip_y`,
  `los_scene_rect` and `los_clip_bottom_pct` are escape hatches, not fixes.
- Whole-map scans are expensive. The scan runs on the mod worker thread, but it
  still competes for memory bandwidth and can show as stuck frames.
- The fan can spill outside world bounds. Clamp against `world_w`/`world_h` and
  the terrain grid in any replacement.

---

## 10. Native mod host (`coke/`)

The host is in `src/coke.rs`. It creates `coke/`, loads top-level `*.dll`, and
only when `coke/user_consent_<stem>.txt` exists. Loading happens on the watcher
thread, never in `DllMain`, to avoid the loader lock.

A mod may export, all optional except that declining `coke_init` unloads it:

```c
int  coke_init(const CokeHost *host);               // 0 = decline
void coke_frame(int width, int height, float fps);  // GL thread, before swap
void coke_hud(int width, int height, float fps);    // GL thread, after all frame hooks
void coke_tick(void);                               // dedicated worker thread
```

API v3 adds `coke_hud`, called after every mod's `coke_frame` and before the
host FPS overlay. Put 2D text there; keep world-space overlays in `coke_frame`.
The host binds framebuffer 0 for text draws, so a world pass that leaves an
offscreen FBO bound cannot make the text vanish.

API v4 adds a host-owned F1-F12 hotkey bus and per-mod draw layers. A mod calls
`register_hotkey(vk, cb)` during `coke_init`; the host owns `GetAsyncKeyState`
polling, the foreground check and rising-edge de-duplication, so one press fires
each callback exactly once. Only F1-F12 are accepted; F9 and F11 are host-owned
and rejected. A mod calls `set_layer(n)` once from `coke_init` to request a draw
layer: higher layers draw later (on top), and mods that never call it stay at
layer 0. Both `coke_frame`
and `coke_hud` iterate in `(layer, load order)` order; the load order is the
case-insensitive DLL file name sort, so the default is deterministic. The key
map is documented in `CONFIG.md`.

The proxy exports `coke_prof_read`, `coke_prof_reset`, `coke_prof_set_timing`
and `coke_prof_version` for `mods/coke-prof` (`src/perf.rs`). `coke_prof_read`
fills a versioned `CokeProfStats`; `coke_prof_set_timing(1)` additionally times
`glCallList`/`glBindTexture` with QPC.

Threading contract:

- `coke_tick` runs on a worker thread pinned away from the game's main thread
  (highest allowed CPU, below-normal priority), roughly every 10 ms.
- `coke_frame` runs on the OpenGL thread before the host overlay and the swap.
  Do not block it; `try_lock`-style access keeps a stuck mod from stalling
  presentation.
- Hotkey callbacks run on the watcher thread, not the GL thread or a mod worker.
  Keep them short and set state for `coke_frame`/`coke_tick` to consume.
- `coke_init` must reject hosts with `api_version < 2` or a `size` smaller than
  the struct it was compiled against.

`CokeHost` v4 fields. v1 is the first block; v2, v2.1 and v4 are appended, so a
mod must check `api_version` and `size` before touching anything past the block
it was compiled against. The actual field order is:

| Group | Field | Purpose |
|---|---|---|
| v1 | `api_version` | ABI version (currently `4`) |
| v1 | `log`, `draw_text` | Host log and 2D text overlay |
| v1 | `get_width`, `get_height`, `get_fps` | Physical client size and cached FPS |
| v2 | `size` | Size of the host struct in bytes |
| v2 | `read_u32`, `read_f32`, `read_bytes` | Fault-safe process-memory reads |
| v2 | `module_base`, `module_size` | Main module identity |
| v2 | `class_name` | RTTI class name from a vtable |
| v2 | `scene_rect`, `camera` | Battle scene rect and camera |
| v2 | `world_to_screen` | Project a world point using the live camera |
| v2 | `cursor_world` | Engine cursor world point |
| v2 | `terrain`, `terrain_height` | ActionMap descriptor and height sampling |
| v2 | `los_scan`, `los_segment` | Terrain LOS scans |
| v2 | `selected_unit`, `unit_pos` | First selected unit and its world position |
| v2 | `draw_world_triangles` | Submit world-space triangles, per-vertex RGBA |
| v2 | `key_down`, `focused` | `GetAsyncKeyState` and foreground-process check |
| v2 | `debug_dump`, `time_ms` | Read-only engine dump and tick count |
| v2.1 | `draw_world_lines` | World-space line pairs at a screen-space width |
| v2.1 | `cursor_ground` | Mouse-ray ground hit (more stable than `cursor_world`) |
| v2.1 | `los_scan_map` | Whole-map visibility grid |
| v4 | `register_hotkey` | Register an F1-F12 callback with the host bus |
| v4 | `set_layer` | Per-mod draw layer, applied to `coke_frame` and `coke_hud` |

A v1 mod can read `api_version`, `log`, `draw_text`, `get_width`, `get_height`
and `get_fps` and nothing else. `mods/coke-fps` uses the v1 prefix, the v3 HUD
split and the v4 hotkey bus when present, and falls back to drawing without a
toggle on older hosts. `mods/coke-los` checks `api_version >= 2` and `size`
before using the v2/v2.1 tail. The authoritative definition is
`crates/coke-host/src/lib.rs`, shared by the proxy and every mod.

Overlay drawing is submitted at the swap point and is not composited into
in-game screenshots taken with `glReadPixels`.

The sample `mods/coke-fps` draws a red `FPS ...` in the top-left. `cokelos` is
the larger sample and the current broken one.

---

## 11. Address and RTTI cheat sheet

**These are not compiled into the proxy.** The CMBS values below are kept here
as the reference table; the code reads them from
`profiles/CM Black Sea.exe/steam-32.ini` as RVAs. See `profiles/README.md`.
The values belong to the live (unpacked) image: `SizeOfImage 0x00a5f000`,
timestamp `0x63d82c90`.

Fixed slots in the packed engine's in-memory user32 import table:

```text
CloseWindow             0x009d53e8
GetCursorPos            0x009d53dc
GetSystemMetrics        0x009d5400
ChangeDisplaySettingsA  0x009d5410
DefWindowProcA          0x009d541c
glViewport              0x009d5320
```

Engine globals and vtables:

```text
IMAGE_BASE              0x00400000
IMAGE_SIZE (header)     0x00a5f000
APP_PTR                 0x00d764b4
APP_HWND_OFF            0x00000bac
SCENE_OWNER_PTR         0x00d7a6b4
BLUR_QUAD_PTR           0x00d7a6c8
WORLD_PTR               0x00d7a6ac
SCENARIO_PTR            0x00d7a6b8

View vtable             0x00bac7b4
GameView vtable         0x00b9c05c
CommandView vtable      0x00b994a0
UnitView vtable         0x00babaa8
SoldierView vtable      0x00baa134
CommandButtonPanel      0x00b992d8
MapView vtable          0x00b9a5dc
Camera vtable           0x00b98b00
CameraPersp vtable      0x00b98b28
ActionMap vtable        0x00b979c8
TerrainMap vtable       0x00bab5d4
Unit vtable             0x00b97d68
```

Key functions:

```text
FUN_00700F10  creates the app object, stores APP_PTR and the HWND
FUN_00618D20  registers the window class and creates the window
LAB_00617AB0  engine window procedure
FUN_007990A0  widget constructor
FUN_00799A30  widget draw entry (viewport + ortho)
FUN_00799BD0  widget draw entry (viewport + ortho)
FUN_007A6B40  battle screen construction
FUN_005D18D0  battle scene widget construction
FUN_0054F4D0  camera update
FUN_0054EE90  projection setup (reads the live viewport)
FUN_005A63C0  width-dependent layout helper
FUN_00550430  dialog centring
FUN_005865F0  width-dependent UI check
FUN_00724F30  battle render-target object creation
```

Always confirm `IMAGE_BASE` and `IMAGE_SIZE` before trusting the fixed slots.
On an unknown build the proxy logs `profile mismatch` and uses the generic IAT
scanner. The RTTI helpers are the build-portable alternative for UI objects.

### Shock Force 2 (GOG 32-bit)

SF2 values live in `profiles/CM Shock Force 2.exe/gog-32.ini`; they are not
repeated here because they are not interchangeable with CMBS. Quick identity:

```text
SizeOfImage        0x00b3d000
PE timestamp       0x696fead4
preferred base     0x00400000   (on disk; the live image is rebased)
.text FNV-1a       0x115cb589   (on disk, relocation-normalised)
```

The profile has all four `[slots]`, `[canvas]`, `[scene]` and six `[rtti]`
entries and is `status=verified`. It has no `[los]` block, so `cokelos` stays
unavailable on that build. [titles/shock-force-2.md](titles/shock-force-2.md)
has the offsets, the evidence and the two bugs fixed during the port.

### Cold War (GOG 32-bit)

Cold War is a verified port. `[canvas]` uses its own app global, and `[scene]`
uses `widget_class = GameView` because the battle screen has no stable static
owner global. Windowed mode, F11, FPS, mods, live re-layout and battle-scene
repair all work. Do not copy the SF2 or CMBS canvas/scene addresses into it.
[titles/cold-war.md](titles/cold-war.md) has the current state.

The profiles are the source of truth for all three titles; the values in §11 are
the CMBS reference table only.

---

## 12. Common pitfalls

- **Do not post `WM_SIZE`.** The engine ignores it. Change `app[0]/[4]` (and
  repair widget rects) instead.
- **Do not rewrite only the drawn viewport.** The engine picks against its own
  rect; fix the rect source instead.
- **Do not read identity from the on-disk PE.** The loader rewrites the PE
  header in memory (Cold War: on-disk timestamp `0x00555c00`, live
  `0x68d02464`). Fill `image_size`, `pe_timestamp` and `text_fnv1a` only from the
  live `profile dump:` line; `PORTING.md` section 2 has the procedure.
- **Do not scan module data as widgets.** Restrict heap scans to committed,
  guard-free, `MEM_PRIVATE` writable regions, and read through
  `ReadProcessMemory`. The first version of the scan died on guard pages and
  could corrupt module globals.
- **Do not trust a stale HWND.** The engine destroys and recreates its window
  during startup. Compare process IDs when checking focus.
- **Do not subclass too early.** Wait for a real executable `GWLP_WNDPROC`.
- **Do not expect the intro to follow the canvas.** It is DirectShow, outside
  GL.
- **Do not expect a launcher to accept synthetic clicks.** If the title starts
  through a launcher, a real Play click may be required; script the engine
  process after it appears.
- **Do not assume other CM titles work unchanged.** The slot addresses, RTTI
  vtables and layout constants are per-build. Port the validation and fallback
  paths, then retest windowed mode, FPS and clicking.
- **Do not compare a raw live `.text` hash on SF2.** The protector rebases the
  image and rewrites `ImageBase`; use the relocation-invariant identity from
  §3 and [titles/shock-force-2.md](titles/shock-force-2.md).
- **Do not read one interleaved log as one run.** `coke.log` is append-only and
  shared by every process that loads the DLL, and two game launches can
  interleave by PID. Filter by the startup PID and timestamp before concluding
  a run is broken. A loaded DLL can be renamed but not overwritten, so deploy
  with a staged rename and relaunch.
- **Do not guess `[los]` anchors to make `cokelos` light up.** A guessed write
  is a crash; absent `[los]` is a valid, safe state. See §9 and
  [titles/shock-force-2.md](titles/shock-force-2.md).

---

## 13. Verification checklist for a change

Parameterised by the profile under test. Replace `<build>` with the row being
verified and `<profile>` with its `profile:` log line.

1. Build both the proxy and any changed mod for `i686-pc-windows-msvc`.
2. Start the game: directly from the executable if the install supports it,
   otherwise through the launcher. Start windowed at the default scale; confirm
   menu layout and clicking.
3. Maximize, then F11, then restore; confirm no black bands and exact picking.
4. Enter a battle; resize during the battle; confirm the scene stays below the
   bar and clicks land on the intended unit.
5. Switch away and back; confirm no black flash and no lost frames.
6. Check `coke.log`:
   - `profile: exe=<exe> build=<build> status=<status> image=... ts=...` is
     present and names the intended file;
   - `slots: content check 4/4 passed` when the profile has `[slots]`;
   - `rtti: class table resolved by name` (this works in safe mode too);
   - no unvalidated write lines and no crash lines.
7. A `partial` row may be missing `[canvas]`/`[scene]`. The acceptance is that
   the log says `canvas patch: profile has no [canvas] block` (or no
   `canvas patch:` line at all), the game does not crash, and all features
   marked `yes` in the row still work. Absent features must be listed in the
   matrix, not silently assumed.
8. If touching `cokelos`, test with the camera moving, at map edges, and with
   the command panel open. It is off unless `enabled=1` and the profile has a
   `[los]` block.
9. Confirm a profile install works: `opengl32.dll`, `coke_config.ini` and
   `profiles/<exe>/<build>.ini` next to the exe. With the profile removed the log
   must say `profile: safe mode` and the game must still start.

---

## 14. Per-title port notes

The detailed port write-ups live in their own files:

- [titles/shock-force-2.md](titles/shock-force-2.md) - identity, profile
  anchors, feature status, bugs fixed during the port, live evidence and the
  verification gate.
- [titles/cold-war.md](titles/cold-war.md) - `widget_class = GameView` scene
  anchor, identity, profile anchors and the test result.

Both profiles are `verified`. The support matrix in §0 and the profile files
remain the source of truth; this page keeps the shared engine model and safety
rules.