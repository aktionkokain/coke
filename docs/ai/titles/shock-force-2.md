# Shock Force 2 (GOG 32-bit): verified port

SF2 is the second fully verified build. It reuses the CMBS engine model and the
same proxy paths; only the anchors and the identity rule differ. The shipped
file is `profiles/CM Shock Force 2.exe/gog-32.ini`, `status = verified`
(2026-11-16).

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

`[los]` is intentionally absent: the `cokelos` world/scenario/action-map
anchors are not validated live on SF2, so the mod reports itself unavailable and
stays off. That is a valid profile state, not an error.

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
