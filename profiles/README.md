# Build profiles

A profile is one data file per build of one executable. It holds every engine
address and engine-object offset the proxy needs; nothing build-specific is
compiled into `src/`.

```text
profiles/
  CM Black Sea.exe/
    steam-32.ini        # verified, shipped as a file
  CM Shock Force 2.exe/
    gog-32.ini
  CM Cold War.exe/
    gog-32.ini
```

Every install needs a profile, including CM Black Sea. The DLL does not embed a
copy, so a missing or renamed profile means safe mode.

Profiles are released as one `PROFILES.zip` containing every title's folder.
Copy the zip next to the game executable; the proxy creates `profiles/` and
extracts each `<exe name>/<build>.ini` folder into it on startup. The zip can
also be extracted manually. The folder above is the repository layout, not the
zip root.

## Addresses are RVAs

Every address in a profile is an **RVA**: `absolute_va - preferred_base`, where
`preferred_base` is the image's preferred base (usually `0x00400000`).

This is not cosmetic. The packed CMBS image never relocates, but the unpacked
Combat Mission titles have ASLR enabled and load at a different base on every
launch. The proxy resolves `runtime = module_base + rva` through the loader's
actual base, so one profile works on every machine running that build.

Object *field* offsets (`app_hwnd_off`, `owner_widget_off`, etc.) are offsets
from an object, not RVAs, and are used as-is.

## Identity

```ini
[profile]
exe = CM Black Sea.exe        ; must match the folder/file name for discovery
build = steam-32              ; unique within that folder
status = verified             ; verified | partial | experimental
preferred_base = 0x00400000   ; informational: RVA + this = the VA tools see
image_size = 0x00a5f000       ; live SizeOfImage
pe_timestamp = 0x63d82c90     ; live PE TimeDateStamp
text_fnv1a = 0xf7b80d9a       ; optional, 0/absent = skip
```

Selection requires `image_size` **and** `pe_timestamp` (and `text_fnv1a` when
present) to match the running image. No match -> safe mode: window/F11/FPS/mods
still work, but no engine memory is written. The executable name only decides
where to look, never whether to trust a file.

## Schema

```ini
[slots]                       ; optional: user32 IAT slots, RVAs
close_window = 0x005d53e8
get_system_metrics = 0x005d5400
change_display = 0x005d5410
get_cursor_pos = 0x005d53dc

[canvas]                      ; optional: engine's cached canvas
app_ptr = 0x009764b4
app_hwnd_off = 0xbac
size_tolerance_pct = 0

[scene]                       ; optional: battle-scene objects
owner_ptr = 0x0097a6b4        ; static owner global
widget_class = GameView       ; fallback when no stable owner global exists
blur_quad_ptr = 0x0097a6c8
top_inset = 155
owner_widget_off = 0x20
widget_rect_off = 0x10
blur_quad_rect_off = 0x28
blur_quad_backlink_off = 0x54
blur_quad_enable_off = 0x50

[rtti]                        ; optional fallback; RTTI names are the primary key
View = 0x007ac7b4
GameView = 0x0079c05c
CommandView = 0x007994a0
UnitView = 0x007abaa8
SoldierView = 0x007aa134
CommandButtonPanel = 0x007992d8

[los]                         ; optional: cokelos anchors
world_ptr = 0x0097a6ac
scenario_ptr = 0x0097a6b8
action_map_off = 0x310
map_globals = 0x00974ea4,0x00974ea8
vt_action_map = 0x007979c8
vt_terrain_map = 0x007ab5d4
vt_mapview = 0x0079a5dc
vt_camera = 0x00798b00
vt_camera_persp = 0x00798b28
vt_unit = 0x00797d68
height_off = 0x9c
grid_w_off = 0xa0
grid_h_off = 0xa4
world_w_off = 0x04
world_h_off = 0x08

[terrain]                     ; optional: shared read-only heightfield anchors
world_ptr = 0x0097a6ac
map_globals = 0x00974ea4,0x00974ea8
vt_action_map = 0x007979c8
vt_terrain_map = 0x007ab5d4
height_off = 0x9c
grid_w_off = 0xa0
grid_h_off = 0xa4
world_w_off = 0x04
world_h_off = 0x08
```

Every block except `[profile]` is optional. A profile that only identifies the
build is valid and deliberately produces safe mode with a `profile:` log line.
Absence of `[los]` means "cokelos unavailable", not an error.

`[terrain]` records only read-only world/heightfield anchors, independently of
the fuller `[los]` block. This lets a feature reuse verified terrain access
without making CokeLOS appear available. It enables the shared host ABI's
`terrain` and `terrain_height` functions; camera, unit and occlusion interfaces
still require `[los]`. `[range]` enables the CokeRange native
preview only when the complete adapter and its dependencies are verified for
that exact executable build. It requires the stable scene owner and terrain
anchors, plus native trace, ribbon, point-vtable, HUD, and label addresses. These
addresses are not portable between CM titles; never copy them from CMBS into
another profile. See the CokeRange port procedure in
[`docs/ai/PORTING.md`](../docs/ai/PORTING.md#porting-cokerange).

```ini
[range]                       ; optional; only after native validation
command_states = 0x1          ; bit N allows world command state N (default: 0)
draw_call = 0x001d4013        ; verified CMBS example, not a template value
draw_flush = 0x0039ad80
trace = 0x002145c0
ribbon = 0x003b0950
point_vtable = 0x00796f20
hud_call = 0x001d41e9
hud_draw = 0x001d42a0
label_draw = 0x00210740
label_width = 0x00210130
label_background = 0x003e44a0
```

The range host API is available from ABI v7. ABI v8 adds
`range_capabilities()`; use `RANGE_CAP_PROFILED` to check for a matched native
adapter and `RANGE_CAP_HOOK_INSTALLED` to check that both render hooks passed
runtime call-byte validation.

`[scene]` needs one way to reach the widget that owns the 3D scene. Prefer the
stable `owner_ptr -> owner_widget_off -> widget` chain. When a build keeps the
battle screen only in the heap (Cold War), use `widget_class = GameView`
instead: the proxy resolves that RTTI class from the live widget tree. The
fallback requires `[canvas]` so the app/root widget can be found, and the class
name must match an `[rtti]` class exactly.

## Discovery order

1. `profile=<name-or-path>` in `coke_config.ini` (explicit; `profile=none`
   forces safe mode).
2. `<exe_dir>/profiles/<exe name>/*.ini` (all matching builds).
3. `<exe_dir>/profiles/<exe name>.ini` (single-build shorthand).
4. Identity sweep across every profile folder (for a renamed executable).
5. Safe mode.

## Adding a build

1. Run the game once with `profile_dump=1` in `coke_config.ini`. The log then
   contains the live `SizeOfImage`, PE timestamp, `.text` FNV-1a, the module
   base, and the resolved RTTI vtables. (`profile_dump` works in safe mode too.)
   For `[canvas]`/`[scene]` work, also set `dump_module=1` once and follow
   [DECOMPILING.md](../docs/ai/DECOMPILING.md).
2. Create `profiles/<exe name>/<build>.ini` with `[profile]` and the `[rtti]`
   block.
3. Start again. The log must say `profile: ... status=...` instead of
   `profile: safe mode`.
4. Only add `[canvas]`/`[scene]` after finding and validating those objects with
   the live memory checks described in `docs/ai/PORTING.md`. A wrong address here is a
   crash, not a warning.
5. Mark the row in `docs/ai/FINDINGS.md` §0 honestly (`verified` only after §13 passes).

## Trust boundary

A profile is **trusted input that can redirect writes inside the game process**.
Only install profiles you obtained from this repository or wrote yourself.

The proxy limits the damage in four ways:

- every RVA must fall inside the live image (`0x1000 <= rva`, `rva + 4 <=
  SizeOfImage`), and every object offset must be `<= 0x10000`;
- slot addresses are only used when their current content still points at the
  real user32 function;
- the app object is only adopted when it carries this process's HWND and a
  canvas the engine has actually used;
- a write never happens without re-validating the target immediately before it.

A profile cannot cause code execution outside the game process and cannot load
a DLL; it can still make the game crash by naming the wrong object. That is why
`status = verified` requires the manual checklist in `docs/ai/FINDINGS.md` §13.
