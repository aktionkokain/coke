# Porting to another build or title

**This is a doc only useful for agents, don't bother reading as a human, too much context about the game needed.**

A port adds support for one build of one Combat Mission executable. The output
is a profile file, `profiles/<exe name>/<build>.ini`, plus the test result for
that profile. No game address is compiled into `src/`; if the procedure needs a
code change, it is a generic validation or fallback path, not a new hard-coded
address.

Read this with:

- [DECOMPILING.md](DECOMPILING.md): tooling, image dump and the RTTI recipe;
- [profiles/README.md](../../profiles/README.md): schema, discovery and the
  trust boundary;
- [FINDINGS.md](FINDINGS.md): the verified reference build, the object layouts
  and the safety rules;
- [CONFIG.md](CONFIG.md): the switches used during a port.

The engine paths are generic already: IAT scanning, RTTI class lookup, window
discovery, frame cap, VSync, priority and the mod loader. A build-specific
profile supplies only the engine anchors that those generic paths cannot derive.

---

## 0. What each profile block buys

| Blocks present | Features enabled | Engine memory written |
|---|---|---|
| `[profile]` only | window, `F11`, frame cap, VSync, priority, FPS, mods, safe reads | none |
| `[profile]` + `[rtti]` | the above plus read-only UI class resolution | none |
| + `[slots]` | logical resolution, cursor mapping, alt-tab fix, display-mode suppression | import slots only |
| + `[canvas]` | live canvas patch, native UI re-layout after resize/F11/maximize | app canvas and widget rects |
| + `[scene]` | a stable scene widget/rect for the battle layout override, `scene_rect` API consumers and `cokelos` | scene rect |
| + `[los]` | `cokelos` terrain overlay (opt-in) | none |

The first row is a complete, useful port. A profile that only identifies the
build is valid and performs no engine writes. Add the optional blocks one at a
time, in the order above, and test after each one.

`[scene]` is not what makes resize-time scene repair work. `scene_rect_fix`
probes the heap for the render target and its linked widget on its own (see
section 6), so a profile without `[scene]` still repairs the battle rect and
keeps picking aligned. What `[scene]` adds is a named, stable scene widget: the
battle layout override can target it, `scene_rect` returns a rect to mods, and
`cokelos` can project against it. Add `[scene]` for those, not to make resizing
work.

---

## 1. Prerequisites

- A legitimate install of the target game, 32-bit.
- The proxy built for `i686-pc-windows-msvc` (see
  [DEVELOPING.md](../user/DEVELOPING.md#build-from-source)).
- `REA` plus Ghidra for static analysis, and a way to inspect live memory (the
  proxy's fault-safe reads or Frida). Setup and Windows caveats are in
  [DECOMPILING.md](DECOMPILING.md) section 1.
- A local analysis tree. Decompiled output, dumps and projects stay outside the
  repository; `analysis/` is already gitignored.

Start the game manually. Try the executable directly first: several installs
(Cold War GOG, for example) run fine without their launcher. If the title only
starts through a launcher, the launcher is just the way to start the engine;
never attach to it or to the intro video.

---

## 2. Identify the build

A profile matches exactly one build. Get the identity before analyzing
anything.

1. Put the proxy next to the executable with `coke_config.ini`.
2. Set:

   ```ini
   [debug]
   profile_dump=1
   verbose=1
   ```

   `profile_dump` ships in `coke_config.ini` with the default `0`; set it to
   `1` (it is also documented in [CONFIG.md](CONFIG.md#profile_dump)). It works
   in safe mode, so a build with no profile yet still reports its identity.

3. Start the game and read the first `profile dump:` line in `coke.log`:

   ```text
   profile dump: module base=0x... image=0x... ts=0x... text_rva=0x... text_size=0x... text_fnv1a=0x... blocks=...
   ```

   `image`, `ts` and `text_fnv1a` are the match fields. Record the module base
   for this run; it is needed to convert addresses to RVAs.

   **The live dump is authoritative; the file on disk is not.** The game loader
   rewrites the PE header in memory, including `TimeDateStamp`. On the Cold War
   GOG build the on-disk timestamp read `0x00555c00` while the live value (and
   the value the profile must match) was `0x68d02464`. Do not fill
   `pe_timestamp` from a static PE reader, `dumpbin`, or a decompiler import -
   only from this log line. The same applies to `image_size` and
   `text_fnv1a`: use the live values, or the profile will look correct and never
   match.

4. Record the executable and its location:

   ```powershell
   Get-FileHash "D:\...\CM <title>.exe" -Algorithm SHA256
   ```

   The hash is for your notes and for identifying the install; it is not a
   profile match field.

5. Record whether the image is packed (no relocations, fixed base) or unpacked
   (ASLR). Packed CMBS always loads at its preferred base; SF2 and Cold War are
   unpacked and load at a different base per launch.

**Everything in a profile is an RVA**, never an absolute VA:

```text
rva = absolute_va - preferred_base
```

`preferred_base` in `[profile]` is informational. The proxy resolves
`runtime = module_base + rva` from the loader's actual base. This is what makes
one profile work on every machine running that build.

Turn `profile_dump` off after this step.

---

## 3. Write the profile skeleton

Create `profiles/<exe name>/<build>.ini`, copying the shape from an existing
profile in `profiles/` and renaming it for this build and title:

```ini
[profile]
exe = CM <title>.exe
build = <steam-32 | gog-32 | other>
status = experimental
preferred_base = 0x00400000
image_size = 0x...
pe_timestamp = 0x...
text_fnv1a = 0x...
```

Rules:

- `exe` must match the executable/file name used for discovery.
- `build` must be unique within that folder.
- `image_size`, `pe_timestamp` and `text_fnv1a` are copied from the live
  `profile dump:` line. Never take them from the on-disk PE header: the loader
  rewrites the header in memory, so a static read can store a value the proxy
  will never match.
- `status` starts at `experimental` or `partial`; use `verified` only after the
  checklist in section 10 passes on a clean install.
- Leave every optional block out until its values are found and validated.

Start the game. The log must change from `profile: safe mode` to:

```text
profile: exe=... build=... status=...
```

Discovery order, for reference:

1. `profile=<name-or-path>` in `coke_config.ini` (`profile=none` forces safe
   mode);
2. `<exe_dir>/profiles/<exe name>/*.ini`;
3. `<exe_dir>/profiles/<exe name>.ini`;
4. an identity sweep over every profile folder;
5. safe mode.

---

## 4. Find the user32 import slots (`[slots]`)

The packed CMBS image builds its import table at runtime, so a static IAT is not
available. The proxy scans for the real user32 pointers; with `verbose=1` the
log gives the slot RVAs directly:

```text
IAT scan: <name> slot=... rva=...
```

Relevant names: `close_window`, `get_system_metrics`, `change_display` and
`get_cursor_pos`.

Write only the RVAs you have observed:

```ini
[slots]
close_window = 0x...
get_system_metrics = 0x...
change_display = 0x...
get_cursor_pos = 0x...
```

Before a slot is hooked, the proxy checks that the slot's current content still
points at the real user32 function. Never copy a slot from another build; a
stale slot is an unsafe patch, not a warning. Test the result with
`hook_slots=1` and confirm the log's content check passes.

If a title has no useful slots, omit `[slots]`. Window and F11 still work; only
the imported calls stay untouched.

---

## 5. Find the cached canvas (`[canvas]`)

This is the highest-value step: without it, the engine keeps laying out for its
startup resolution after a resize.

1. Find the function that creates the engine window. There are two routes. Try
   A first and switch to B as soon as it stops paying off; do not spend an hour
   enumerating class-name variants.

   **A - window class string.** Search the image for a `CM <title>`-style class
   name, then the `RegisterClassA` / `RegisterClassExA` XREF. The class is
   registered by the function that also calls `CreateWindowExA`. This is the
   route that works on CMBS (`CM Black Sea`), where the class name is a plain
   string in the image.

   **B - USER32 import table.** If no class string exists, the class name is
   probably built on the stack. That is normal, not a sign of a stripped build:
   Cold War does it. Read the USER32 imports from the dumped image
   (`inspect_pe_imports`), then xref these IAT slots:

   ```text
   RegisterClassExA
   CreateWindowExA
   GetSystemMetrics
   ```

   The `CreateWindowExA` xref lands in the window-creating function. On Cold War
   that is `FUN_00319110`, which fills a `WNDCLASSEXA` on the stack, registers
   it, reads `GetSystemMetrics(0/1)` and calls `CreateWindowExA`.

   Known dead ends on Cold War, so you recognise them: `Cold War` matched only
   version tags (`cold war v100a` … `v110`); `CM Cold War`, `CMCW` and
   `CombatMission` had no hits; `CM ` matched an unrelated `cipher not gcm mode`
   string.

2. Immediately before `CreateWindowExA`, find the two reads
   `GetSystemMetrics(SM_CXSCREEN)` and `GetSystemMetrics(SM_CYSCREEN)`. This is
   the only place the engine reads the screen size for window creation.
3. Follow the two results to the store. They are written into an engine global
   object: the app object, or a constructor called with it. On Cold War the
   window function passes the HWND to `FUN_004000f0` (reached through
   `FUN_00319950`), which allocates the app object and stores the global. Read
   the constructor, not only the window function, when the canvas stores are not
   in the same function.
4. In that object, the first two 32-bit integers are canvas width and height:

   ```text
   app + 0x00  canvas width
   app + 0x04  canvas height
   app + 0xbac engine HWND        (verify the offset on this build)
   ```

5. Confirm the HWND field by comparing it with the window handle after creation.
   Do not trust the pointer because the pattern looked similar.
6. Confirm the engine re-reads those ints for layout and for the mouse Y
   inversion in the window procedure:

   ```text
   y' = app[1] - (lParam >> 16) - 1
   ```

7. Write the block:

   ```ini
   [canvas]
   app_ptr = 0x...          ; RVA of the global that holds the app object
   app_hwnd_off = 0xbac     ; offset of the HWND field, from the app object
   size_tolerance_pct = 0
   ```

Validation the proxy performs before adopting the object:

- the app pointer is committed and readable;
- the HWND belongs to this process and is the game window;
- the cached canvas is plausible for the current client size (or within
  `size_tolerance_pct`).

Patch the canvas and the mouse transform together. A canvas patch without the
mouse transform makes clicks land in the wrong place; a mouse transform without
the canvas patch leaves the UI stale.

Do not post `WM_SIZE`: the engine ignores it and has no resize path. The cached
pair is the only input its layout math re-reads.

---

## 6. Find the battle scene rect (`[scene]`)

Both the drawn battle viewport and click-to-unit picking derive from a
render-target rect, not from the GL viewport. Repairing only the viewport breaks
picking.

1. Find the battle screen constructor and the render-target allocation. On CMBS
   the render target is created by `FUN_00724F30` (size `0x5c`).
2. The object layout on the reference build:

   ```text
   scene owner global -> +0x20 -> scene widget -> +0x10 i16 x/y/w/h
   render target      -> +0x28 u32 x/y/w/h
                         +0x50 enable flag
                         +0x54 widget backlink
   ```

3. The rect is copied once at creation from the scene widget and only partly
   updated later, so the owner, the render target and the widget must stay
   consistent.
4. Write the block, using the offsets you verified:

   ```ini
   [scene]
   owner_ptr = 0x...              ; RVA of the scene owner global
   blur_quad_ptr = 0x...          ; RVA of the companion global, if present
   top_inset = 155                ; command bar height; verify on this build
   owner_widget_off = 0x20
   widget_rect_off = 0x10
   blur_quad_rect_off = 0x28
   blur_quad_backlink_off = 0x54
   blur_quad_enable_off = 0x50
   ```

   **No stable owner global?** Some builds keep the battle screen only in the
   heap. To tell the cases apart: dump the widget tree with F9 in a battle, take
   the scene widget's parent pointer, then search the module image for that
   dword. No hit means there is no static anchor. If you do find a candidate
   global, test it across a menu -> battle -> menu transition; a global that
   merely happens to hold the widget during one battle will be stale after the
   transition. In that case name the scene widget's RTTI class instead:

   ```ini
   [scene]
   widget_class = GameView
   top_inset = 155
   ```

   The proxy walks the live widget tree from the `[canvas]` app/root object and
   picks the first node of that class. RTTI is the primary key, so this survives
   ASLR and needs no address. It requires `[canvas]`, and the class must be the
   one that owns the 3D scene, not a parent `View`. The name must match an
   `[rtti]` class exactly.

5. Test in a battle: the scene must sit below the command bar, and a click on a
   unit must select that unit. Resize mid-battle and repeat. With `[scene]`,
   check `widget relayout: ... scene=0x...` for a non-zero scene address. Without
   it, check `scene rect fix: verified y=...` and confirm picking by hand.

`scene_rect_fix` learns the y convention from the engine's own frames: it writes
a candidate y, observes the next scene viewport, and applies the correction to
both the render target and the linked widget. Do not replace that feedback loop
with a hard-coded transform unless you verify it live. `scene_vp_rewrite` is the
legacy viewport-only path and is deliberately off because it breaks picking.

If the new title has no equivalent object, omit `[scene]` but keep
`scene_rect_fix=1`. The repair probes the heap by itself; with `[canvas]` alone
it still fixes the battle rect and keeps picking aligned. What you lose is the
named scene widget for the battle layout override, the `scene_rect` mod API and
`cokelos`. Record that in the profile status and the feature matrix; do not
disable `scene_rect_fix` merely because the block is absent.

---

## 7. RTTI class table and UI re-layout

Native re-layout identifies widget roles by RTTI class, never by current
geometry:

- the screen child that owns a `CommandView` is the command bar;
- `GameView` is the battle scene;
- the remaining `View` classes are the flanks;
- menus and dialogs are centred relative to their parent.

The proxy resolves RTTI by scanning the live image for the mangled MSVC type
name, so its log already contains the values a `[rtti]` block needs:

```text
rtti: class table resolved by name: ...
```

Copy those vtables, minus the module base, into:

```ini
[rtti]
View = 0x...
GameView = 0x...
CommandView = 0x...
UnitView = 0x...
SoldierView = 0x...
CommandButtonPanel = 0x...
```

`[rtti]` is an optional fallback: the class name remains the primary key, so a
build whose addresses move still works when the RTTI names survive. The full
static recipe (type descriptor, complete object locator, vtable, read-back
check) is in [DECOMPILING.md](DECOMPILING.md) section 6.

Layout constants to confirm on the new build: `1024` (`0x400`) battle-bar width,
`768` (`0x300`) dialog height, `155` (`0x9b`) scene inset and `164` (`0xa4`)
command-bar height.

Test the re-layout in dry-run mode first:

```ini
[compat]
widget_relayout=2
```

That logs the planned `old -> new` rects and writes nothing. When the planned
rects are correct, set `widget_relayout=1` and retest the menu, dialogs and the
battle HUD at several canvas sizes. Keep `menu_recenter=0` until those tests
pass; it walks the heap and shifts subtrees.

If the class names differ, add them to the class table. Keep the geometry-based
fallback off.

---

## 8. LOS anchors (`[los]`, optional)

`cokelos` needs a `[los]` block; without it the mod reports "unavailable" and
stays off. The anchors and offsets are the world/map/camera fields documented in
[FINDINGS.md](FINDINGS.md) section 9 and in
[profiles/README.md](../../profiles/README.md):

```ini
[los]
world_ptr = 0x...
scenario_ptr = 0x...
action_map_off = 0x310
map_globals = 0x...,0x...
vt_action_map = 0x...
vt_terrain_map = 0x...
vt_mapview = 0x...
vt_camera = 0x...
vt_camera_persp = 0x...
vt_unit = 0x...
height_off = 0x9c
grid_w_off = 0xa0
grid_h_off = 0xa4
world_w_off = 0x04
world_h_off = 0x08
```

Only add `[los]` after the object model is verified. The shipped overlay is
terrain-only: tree and building occluders are not wired in, and the current scan
is expensive. See `analysis/SPOTTING.md` for the spotting data model. A
profile without `[los]` is valid and is not an error.

---

## 9. Validate the port

Build and deploy the proxy and config to the game folder:

```powershell
.\scripts\build.ps1
.\scripts\deploy.ps1 -GameDir 'D:\...\Combat Mission <title>'
```

Run with `verbose=1` and check `coke.log` for:

- startup identity: `module base=... image_size=...`;
- `profile: exe=... build=... status=...` naming the intended file;
- `slots: content check 4/4 passed` when `[slots]` is present;
- `rtti: class table resolved by name` (works in safe mode too);
- `canvas patch: ...` after a resize, F11 or maximize, when `[canvas]` is
  present;
- `widget relayout: ...` and `scene rect fix: ...` where applicable;
- no unvalidated-write lines and no crash lines.

Then:

1. Start the game directly from the executable if the install supports it;
   otherwise use the launcher. Start windowed at the default scale; check menu
   layout and clicking.
2. Maximize, F11, restore; no black bands, exact picking.
3. Enter a battle; resize during the battle; the scene stays below the bar and
   clicks land on the intended unit.
4. Alt-tab away and back; no black flash and no lost frames.
5. Remove the profile and confirm the log says `profile: safe mode` and the game
   still starts.
6. Confirm mods load: a consent-gated sample draws, or is skipped without
   consent.

For a `partial` profile, the missing blocks must fail closed: the log names the
missing block, the game does not crash, and every feature marked as working
still works.

---

## 10. Acceptance

A port is complete when:

- one profile file matches the installed build and survives a restart;
- every block in the profile has been validated on that build, not copied from
  another one;
- the test list in section 9 passes on a clean install;
- the profile's `status` is `verified` only if the full checklist passed;
- the support matrix in [FINDINGS.md](FINDINGS.md) section 0 and the profile
  status agree;
- no decompiled output, memory dump, Ghidra project or game asset is in git.

A `partial` port is a valid deliverable. Mark the missing features honestly;
never guess an address because an older build had something at that location. A
wrong write in a 32-bit engine is a crash, not a warning.

---

## Appendix A: worked RVA example

The reference CMBS profile in `profiles/CM Black Sea.exe/steam-32.ini` stores
these values:

```text
app_ptr         0x009764b4   <- VA 0x00d764b4 - base 0x00400000
scene owner     0x0097a6b4   <- VA 0x00d7a6b4 - base 0x00400000
world           0x0097a6ac   <- VA 0x00d7a6ac - base 0x00400000
get_system_metrics slot 0x005d5400 <- VA 0x009d5400 - base 0x00400000
```

The absolute VAs and the function names for that build are in
[FINDINGS.md](FINDINGS.md) section 11. Treat them as evidence for the reference
build only; derive the target build's values with the procedure above.

---

## Appendix B: worked Cold War example (GOG 32-bit)

This is the run that added `[canvas]` and `[scene]` to
`profiles/CM Cold War.exe/gog-32.ini`, starting from a `partial` profile that
already had identity and `[rtti]`. Use it as the example to copy when the class
string route in section 5 fails. Addresses and function names are for this build
only. The resulting profile and the live evidence are also in
[titles/cold-war.md](titles/cold-war.md).

### Identity

Live log (the on-disk value is shown for contrast):

```text
module base=0x100000 image_size=0xb2b000 ts=0x68d02464
identity: text base=0x100000 preferred=0x400000 relocs=101247
profile dump: ... text_rva=0x1000 text_size=0x5d2e4c text_fnv1a=0x8b23ec42
```

On disk the PE timestamp read `0x00555c00`. The profile carries the live
`0x68d02464`; this is the trap described in section 2. The shipped profile
matched immediately:

```text
profile: exe=CM Cold War.exe build=gog-32 status=partial
         image=0xb2b000 ts=0x68d02464 base=0x100000
         slots=0 canvas=0 scene=0 rtti=6 los=0
```

So identity and RTTI were already correct; only `[canvas]` and `[scene]` were
missing.

### Window and app object

No window class string existed, so section 5 branch B applies. The image was
dumped with `dump_module=1` and the USER32 imports read from it (runtime image
base `0x100000`):

| Import | IAT RVA | IAT VA |
|---|---|---|
| `RegisterClassExA` | `0x5d43fc` | `0x6d43fc` |
| `CreateWindowExA` | `0x5d4410` | `0x6d4410` |
| `GetSystemMetrics` | `0x5d4430` | `0x6d4430` |
| `GetCursorPos` | `0x5d43ec` | `0x6d43ec` |
| `CloseWindow` | `0x5d4448` | `0x6d4448` |
| `ChangeDisplaySettingsA` | `0x5d4420` | `0x6d4420` |

XREFs from those slots:

```text
CreateWindowExA  -> 0x3193b2  in FUN_00319110
RegisterClassExA -> 0x31937a  in FUN_00319110
GetSystemMetrics -> 0x319385  in FUN_00319110
GetSystemMetrics -> 0x400369  in FUN_004000f0 (app constructor)
```

`FUN_00319110` fills a `WNDCLASSEXA` on the stack, registers the class, reads
`GetSystemMetrics(SM_CYSCREEN/SM_CXSCREEN)` and calls:

```c
CreateWindowExA(0x40000, className, className, 0x96000000,
                0, 0, nWidth, iVar5, 0, 0, hInstance, 0);
```

`FUN_00319950` allocates `0xbb0` bytes and calls `FUN_004000f0(hwnd)`. The
constructor shows the layout:

```text
GetSystemMetrics(0) -> this[0]      // canvas width  at app + 0x00
GetSystemMetrics(1) -> this[1]      // canvas height at app + 0x04
this[0x2eb] = hwnd                  // HWND          at app + 0xbac
DAT_00b73e24 = this                 // app global
```

RVA: `0x00b73e24 - 0x00100000 = 0x00a73e24`.

```ini
[canvas]
app_ptr = 0x00a73e24
app_hwnd_off = 0xbac
size_tolerance_pct = 0
```

Live validation:

```text
canvas patch: adopted app=0xddc8340 canvas=1920x1080
              stored_hwnd=0x4c112d8 hwnd_source=profile checks=ok
canvas patch: engine canvas 2560x1377 -> 1384x761 (app=0xddc8340)
widget relayout: apply logical=1384x761 nodes=120 changed=119 moved=119 skipped=0 scene=0x0
```

The `app_hwnd_off` happens to be the same `0xbac` as Black Sea and SF2, but the
app global is not: derive it from this build's constructor.

### Scene: no stable owner global

The `GameView` vtable is at RVA `0x826340`; its constructor is `FUN_002d1c40` and
calls `FUN_00498800(..., 0x9b, ...)` (155 = scene inset). An F9 dump during a
battle shows:

```text
d=1 a=0x13dd6b80 vt=0x00936a08 cls=View      p=0x12907b0
d=2 a=0x28045a98 vt=0x00926340 cls=GameView  p=0x13dd6b80 rect=(0,155,2560,1284)
```

`GameView` is a direct child of the battle-screen `View`, and the parent pointer
is at `GameView + 0x04`. What did not work:

- searching the module image for the parent (`0x13dd6b80`) returned no dword;
  the battle-screen object exists only in the heap;
- a global at RVA `0xabcb58` sometimes held the current `GameView`, but changed
  across menu/battle transitions, so it is not a safe anchor;
- `app + 0x110` is not the scene owner: it points to a list object whose `+0x20`
  points back into the app allocation (`app+0xbb8`).

The static chain `global -> owner -> widget` cannot be expressed, so use the
widget-tree fallback from section 6:

```ini
[scene]
widget_class = GameView
top_inset = 155
```

Live evidence:

```text
profile: ... canvas=1 scene=1 rtti=6
scene inset learned: 155
widget dump: summary ... scene=0x27a683e0 scene_rect=Some([0, 155, 2560, 1222])
widget relayout: 0x27a683e0 (0,155,2560,1222) -> (0,155,2560,1284) scene axis=S/E
```

Play test: windowed start, F11, repeated resizes in a campaign battle, HUD
re-layout and click-to-unit picking all passed. `[slots]` is omitted because the
generic IAT scanner validates the four user32 slots on this build; `[los]` is
omitted, so `cokelos` stays unavailable. The profile is `status = verified`.
