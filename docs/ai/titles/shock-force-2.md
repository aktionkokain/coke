# Shock Force 2 (GOG 32-bit)

The shipped profile is `profiles/CM Shock Force 2.exe/gog-32.ini`,
`status = verified` (2026-11-16). That status means the proxy identity, window,
canvas, and scene integration described here were verified. It does not mean
every optional feature has an SF2 adapter. CokeRange has its own adapter
on this build. CokeLOS remains unavailable.

The engine model is in [FINDINGS.md](../FINDINGS.md): binary model in §3, widget
model in §5, battle scene in §6 and the safety rules in §12. This page is only
the SF2-specific evidence.

## Identity

| Property | Value |
|---|---|
| Executable | `CM Shock Force 2.exe` |
| Build | `gog-32` |
| `SizeOfImage` | `0x00b3d000` |
| PE timestamp | `0x696fead4` |
| On-disk preferred base | `0x00400000` |
| On-disk `.text` FNV-1a | `0x115cb589` |
| Live load base (example) | `0x00ef0000` |
| `.text` `HIGHLOW` relocations | `100833` |

The protector rewrites the in-memory `ImageBase` to the load base, so the live
`.text` hash differs from the on-disk hash. Match with the relocation-invariant
hash described in [FINDINGS.md §3](../FINDINGS.md#3-binary-and-process-model)
(`src/engine.rs`: `module_preferred_base`, `module_text_relocations`,
`fnv1a_canonical`, `disk_preferred_base`,
`module_text_fnv1a_with_preferred`). A raw live hash will never match.

Startup line on the verified build:

```text
identity: text base=0xef0000 preferred=0x400000 relocs=100833
profile: exe=CM Shock Force 2.exe build=gog-32 status=verified image=0xb3d000 \
  ts=0x696fead4 base=0xef0000 slots=1 canvas=1 scene=1 rtti=6 los=0
```

## Profile anchors

All engine addresses are RVAs (`VA - preferred_base`); object fields are plain
offsets. The profile accessors add the live module base, so the SF2 rebase does
not invalidate them.

`[slots]` (taken from the PE import table; all four pass
`slots: content check 4/4 passed`):

| Function | RVA |
|---|---|
| `close_window` | `0x005cc448` |
| `get_system_metrics` | `0x005cc430` |
| `change_display` | `0x005cc420` |
| `get_cursor_pos` | `0x005cc3ec` |

`[canvas]`: `FUN_006f8690` stores the app object at `[0xe53884]` (RVA
`0x00a53884`). Canvas width/height are the first two `i32` (`app+0x00`,
`app+0x04`); the main HWND is at `app+0xbac`; `size_tolerance_pct = 0`.

`[scene]`: `[0xe57af8]` (RVA `0x00a57af8`) is the battle/session object; the
scene widget is at `owner+0x20` (stored by `FUN_0079eda0` through
`FUN_005cb4a0`). `[0xe57b0c]` (RVA `0x00a57b0c`) is the blur quad, allocated
`0x5c` bytes by `FUN_0071c440`, with the scene widget's `i16` rect at `+0x28`,
an enable flag at `+0x50` and a widget backlink at `+0x54`. `top_inset = 155`.

`[rtti]` (class names remain the primary key):

| Class | RVA |
|---|---|
| `View` | `0x007aa230` |
| `GameView` | `0x00799b1c` |
| `CommandView` | `0x00796b64` |
| `UnitView` | `0x007a9590` |
| `SoldierView` | `0x007a7c24` |
| `CommandButtonPanel` | `0x00796f74` |

`[terrain]` records verified read-only anchors from the live setup battle on
2026-11-22. The world pointer is `[0xe57b00]` (RVA `0x00a57b00`); the two
ActionMap global slots are `[0xe52354]` and `[0xe52358]` (RVAs `0x00a52354` and
`0x00a52358`) and both held the same live map. The map's RTTI vtable matched
`ActionMap` (`0x007955a0` RVA); `TerrainMap` is also recorded as `0x007a9034`.
Live fields were `world size 512x512`, heightfield at `+0x9c`, and grid
`0x201x0x201` at `+0xa0/+0xa4`. The world command state was `0` in idle
battle setup and `2` while the stock Target preview was active. CokeRange
uses the idle state. The shared host `terrain` and `terrain_height` functions
can use this terrain-only profile.

`[los]` is intentionally absent: CokeLOS still lacks validated MapView/camera
and occlusion contracts. Keeping terrain anchors separate means they can be
reused by CokeRange without implying that the LOS feature is available.

`[range]` provides the native CokeRange adapter. A bounded OpenGL call-site
probe in a live SF2 battle observed the native Target strip
draw at runtime VA `0x00a23d83` (RVA `0x00123d83`, preferred-base VA
`0x00523d83`). Ghidra decompilation maps that call to `FUN_00523760`, a helper
that builds a textured strip from two world-space endpoints. One caller is
`FUN_0079b6c0`, which invokes the helper while `DAT_00e57b09 == 0`. This
identifies part of the native rendering path. The runtime call stack also
reaches a call at preferred VA `0x005cdc83` (RVA `0x001cdc83`) to
`FUN_007930f0` (RVA `0x003930f0`). Ghidra shows that function drains the native
render queue by invoking each queued object's virtual draw method at vtable
offset `+0x38`.

### Native range adapter, 2026-11-22

The [reference comparison](../RANGE-PORTING.md) ranked every code anchor in
the matching SF2 build. The stock Target renderer at VA `0x007a9310` calls
the styled ribbon builder at `0x007a88a0`. Its visibility path through
`0x007aa9b0` calls the trace routine at `0x0060d700` and branches on its
byte result. The point vtable resolved through MSVC RTTI for `Vector3`.

| Profile field | RVA |
|---|---|
| `draw_call` → `draw_flush` | `0x001cdc83` → `0x003930f0` |
| `trace` | `0x0020d700` |
| `ribbon` | `0x003a88a0` |
| `point_vtable` | `0x00794b8c` |
| `hud_call` → `hud_draw` | `0x001cde59` → `0x001cdf20` |
| `label_draw`, `label_width`, `label_background` | `0x00209870`, `0x00209260`, `0x003dba90` |

The battle renderer resets the ribbon pool through owner field `+0xc8c4`
on SF2. CMBS uses `+0xc99c`. The profile stores that difference as
`ribbon_pool_off`. The live SF2 call bytes before installation were
`e8 68 54 1c 00` (queue) and `e8 c2 00 00 00` (HUD). Both the range
update and HUD read the world via the shared terrain profile.

**Live validation.** On SF2 v2.11 (GOG x86), the loader logged
`coke: loaded coke_range.dll` and
`cokerange: native battle ribbon hook installed`. In an urban tutorial's
single player setup phase, unselected `T` started a measurement on a building
11.76 m above the local terrain height. A 161 m sample showed the blocked ribbon and
`No Line of Sight`. A 43 m ground sample showed the blue ribbon and
`Ground Range`. `T` cancelled and started a new measurement at the current
cursor. A live call probe confirmed the trace uses the owner context at
`+0x1c` and observed both AL verdicts. Over a full `H` cycle, the ribbon's
source Z advanced by 0.75, 1.0, and 1.5 m and then returned to its starting
height, with the same source X and Y.
The native Target preview was observed separately. The post-install pass
covered windowed setup with no unit selected. It did not cover the
selected-unit shortcut, sustained camera input, or other battle phases.

### Target cursor picking, 2026-11-22

The live setup battle was left in the native `T` Target phase after the user
restarted SF2. Its prompt, blue world ribbon, `Area Target` marker and `Range
32m` label were visible. A live call trace and Ghidra analysis isolated the
cursor picking path:

| Preferred VA | RVA | Observation |
|---|---|---|
| `0x005cc430` | `0x001cc430` | MapView mouse/cursor query; called from `FUN_00391bb8` |
| `0x007922d0` | `0x003922d0` | Iterates candidate scene objects and invokes their intersection virtuals; returns the nearest picked object/point |
| `0x00720900` | `0x00320900` | Generic scene-geometry segment query; used by the cursor query and other engine paths |
| `0x007172f0` | `0x003172f0` | TerrainMap heightfield ray query, vtable `+0x80` |
| `0x00717970` | `0x00317970` | TerrainMap heightfield ray query, vtable `+0x78` |
| `0x0061eeb0` | `0x0021eeb0` | Triangle segment intersection used by the TerrainMap queries |

The native world ribbon is queued as a `VisualLink` object. Its observed live
object had the vtable at preferred VA `0x00baa450`; the `+0x38` draw slot is
`FUN_0079b6c0`, which calls the textured-strip renderer. `FUN_007930f0`
drains the render queue and dispatches that slot. The generic factory
`FUN_007a82d0` was an early lead. The Target caller resolves to its styled
counterpart `FUN_007a88a0`, listed in the adapter table above.

The generic scene segment query identifies what is under the cursor.
CokeRange retains its shared depth-based surface picker and calls the
separate visibility routine for LOS. Its origin can be arbitrary, while
the stock Target command uses the selected unit and its posture.

### Why this port takes longer than CM:BS

CM:BS established CokeRange's native trace, ribbon, label and hook
contracts. For SF2, each address had to be matched to equivalent behavior
in the new executable. Each native call also needed the expected object
layout, calling convention and result semantics.

| Work area | CM:BS reference | SF2 port |
|---|---|---|
| Range adapter | Established the native interface | Matched the same interface against a different executable |
| Build identity | Existing profile and analysis baseline | Matched SF2's own identity and relocation behavior |
| Native drawing | Known ribbon, HUD and insertion hooks | Found the equivalent call sites and the shifted pool field |
| LOS | Known trace ABI | Followed the SF2 Target call chain to confirm its trace and byte verdict |

Most of the time went into following SF2's cursor and render paths from
scratch. Comparing instruction windows against the working CMBS adapter
quickly located the relevant routines. Keep that comparison as the first
step for future ports.

## Feature status

| Feature | State | Notes |
|---|---|---|
| Identity (image size / timestamp / `.text`) | verified | relocation-invariant hash |
| `[slots]` user32 IAT | verified | 4/4 content check |
| `[canvas]` adoption + patching | verified | client follows the window |
| `[scene]` anchors (viewport/blur) | verified in battle | viewport `(0,155,w,h-155)` |
| RTTI table | verified | 6 classes plus `MapView` |
| Widget re-layout | verified | menu and battle trees |
| Scene-rect repair | verified | feedback probe reaches the inset |
| FPS overlay | fixed | digit-glyph indexing |
| `[los]` | disabled | not validated |
| `[terrain]` heightfield access | verified anchors | live ActionMap and dimensions validated; read-only profile block |
| CokeRange | verified in setup | clear and blocked previews, elevated origin, cancellation and four height settings |

## Bugs fixed during the port

**Canvas patch rejected after the first transition.** Clicks/hover/picking
worked while maximized but not windowed or in F11. `validate_app_candidate`
accepts the cached canvas only if it equals the current logical size or an entry
in `canvas_history()`, but `patch_engine_canvas` recorded only the canvas being
left, never the one being written. After 1920x1080 -> 2560x1377 the history
still held only `{1920x1080}`, so returning to 1920x1080 logged
`canvas patch: app rejected` and left the engine's mouse/layout canvas stale.
Fix: also call `record_canvas(w, h)`. History is deduplicated and bounded to 8
entries; the widget-recenter scan skips the entry equal to the current logical
size, so the extra record is harmless there.

**FPS counter showed junk.** `rasterise_fps` indexed glyphs with `value / 100`
per position and clamped with `.min(10)`, which dropped decimal digits and
mapped overflow onto the `.` glyph (`50.3.1`, `.33.8`). Fix: `fps_glyphs(fps10)`
in `src/overlay.rs` extracts every decimal digit and draws the separator from
the same digit stream. Regression tests:
`fps_glyphs_keep_every_decimal_digit`,
`fps_rasteriser_only_uses_digits_and_the_separator`.

## Live evidence

Window and canvas:

```text
main window found: window 1920x1080 at (0,0), client 1920x1080, style=0x96000000
canvas patch: adopted app=0x1da8570 canvas=1920x1080 stored_hwnd=0x1260e5a \
  hwnd_source=profile checks=ok
window = framed 1936x1119 at (312,140) (logical 1920x1080)
SC_MAXIMIZE: maximising the framed window
logical follows client: 1920x1080 -> 2560x1377
canvas patch: engine canvas 1920x1080 -> 2560x1377 (app=0x1da8570)
```

Battle frame at canvas `2560x1377`:

```text
frame vps: (0,0,2560,1377) (0,155,2560,1222) (0,0,2048,2048) \
           (0,155,2560,1222) (0,155,2560,1222) (0,0,768,155) \
           (1792,0,768,155) (768,0,1024,164)
```

`(0,155,2560,1222)` matches `top_inset = 155` and `canvas_h - 155`; the
`2048x2048` target is the shadow FBO; the flanks and the `1024x164` bar match
the layout in [FINDINGS.md §5](../FINDINGS.md#5-ui-widget-model).

Widget re-layout:

```text
widget relayout: apply logical=2560x1377 nodes=24 changed=24 moved=24 \
  skipped=0 latency=31ms scene=0x0
```

Mouse mapping in follow-the-window mode is the identity, which is what keeps
picking exact once the canvas matches the client:

```text
mouse msg=0x0200 raw=(1109,658) -> logical=(1109,658) scale=1.0000x1.0000 origin=(320,171)
```

If `scale` is not 1.0 or `logical != raw` after a transition, the canvas write
did not take; look for `canvas patch: app rejected`.

## Verification matrix (the gate for `status = verified`)

1. Windowed: click units and HUD.
2. `F11`: click again.
3. Maximize -> un-maximize: click again (the regression above).
4. Mid-battle resize: click a unit and confirm picking.

For each step, require `mouse msg=... -> logical=...` with `scale=1.0000` and no
`canvas patch: app rejected` line in the same interval.
