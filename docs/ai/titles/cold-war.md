# Cold War (GOG 32-bit): verified port

`profiles/CM Cold War.exe/gog-32.ini` is `status = verified`. Build identity,
the RTTI class table, `[canvas]` and `[scene]` were validated live: windowed
start, F11, repeated resizes in a campaign battle, HUD re-layout, and
click-to-unit picking. `[los]` is absent, so `cokelos` is unavailable.

The step-by-step derivation (window discovery without a class string, the app
object, the failed owner-global search) is in
[PORTING.md appendix B](../PORTING.md#appendix-b-worked-cold-war-example-gog-32-bit).
The engine model is in [FINDINGS.md](../FINDINGS.md); this page is only the
Cold War-specific evidence.

## Scene anchor

Cold War has no stable static scene-owner global. Searching the image for the
battle-screen `View` pointer found no dword, and the global that sometimes held
`GameView` changed across menu/battle transitions. The scene widget is a direct
child of the battle-screen `View` and is stable by RTTI, so the profile uses
the widget-tree fallback instead of `owner_ptr`:

```ini
[scene]
widget_class = GameView
top_inset = 155
```

The proxy walks the live tree from the `[canvas]` app/root object and resolves
the first `GameView`. `scene_rect` and the battle layout override use the same
resolver. Live evidence from the verified run:

```text
profile: ... canvas=1 scene=1 rtti=6
scene inset learned: 155
widget dump: summary ... scene=0x27a683e0 scene_rect=Some([0, 155, 2560, 1222])
widget relayout: 0x27a683e0 (0,155,2560,1222) -> (0,155,2560,1284) scene axis=S/E
```

Every resize kept the scene at `y=155` with a `GameView`-shaped rect, and
picking stayed aligned. Do not copy CMBS or SF2 anchors into the Cold War
profile; the `widget_class` fallback is the intended anchor for this title.

## Profile anchors

| Block | Value |
|---|---|
| `[canvas]` | `app_ptr = 0x00a73e24`, `app_hwnd_off = 0xbac`, `size_tolerance_pct = 0` |
| `[scene]` | `widget_class = GameView`, `top_inset = 155` |
| `[rtti]` | `View 0x00836a08`, `GameView 0x00826340`, `CommandView 0x008233a8`, `UnitView 0x00835b8c`, `SoldierView 0x00834488`, `CommandButtonPanel 0x008237d4` |
| `[slots]` | Omitted. Slot hooks depend on verified named imports. |
| `[los]` | omitted; `cokelos` stays unavailable |

## Identity

```text
image_size   0x00b2b000
pe_timestamp 0x68d02464   (live; the on-disk header reads 0x00555c00)
text_fnv1a   0x8b23ec42
```

The live timestamp comes from `profile_dump=1`; see
[PORTING.md section 2](../PORTING.md#2-identify-the-build). The on-disk value is
rewritten by the loader and must not be used.

## Verification

Windowed start, F11, repeated resizes in a campaign battle, HUD re-layout and
click-to-unit picking all passed. The scene stayed at `(0,155,w,h)` and picking
stayed aligned on every resize. The profile is `status = verified`.
