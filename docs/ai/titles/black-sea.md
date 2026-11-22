# Combat Mission: Black Sea

This page records CM:BS-specific engine evidence. The general engine model is
in [FINDINGS.md](../FINDINGS.md), the adapter matching process is in
[RANGE-PORTING.md](../RANGE-PORTING.md), and the current profile is
`profiles/CM Black Sea.exe/steam-32.ini`.

## Build identity

| Property | Value |
|---|---|
| Executable | `CM Black Sea.exe` |
| Build | Steam 32-bit |
| `SizeOfImage` | `0x00a5f000` |
| PE timestamp | `0x63d82c90` |
| Preferred base | `0x00400000` |
| `.text` FNV-1a | `0xf7b80d9a` |

All code locations below are RVAs relative to the preferred base. The overlay
prototype refuses to install unless the executable name and all three identity
values match this profile.

## Deployment tint and terrain overlay prototype

In the verified build, the native deployment-zone tint path includes the
terrain color-grid writer at RVA `0x003d03a0`. The renderer consumes an RGB
float color grid from the `TerrainMap` object (`+0x116f44` pointer, dimensions
at `+0x0c` and `+0x10`). The stock zones alter this grid with multiplicative
palette factors; live red deployment cells were observed at `(2.0, 0.5, 0.5)`.

The CokeRange painter hooks the two terrain draw calls at RVAs `0x0039a84f`
and `0x0039a961`. It supplies a separate RGB grid during the draw, leaving the
deployment colors intact. Near terrain uses that grid and refreshes its
per-chunk color cache when paint changes. The distant renderer draws 16 m
terrain chunks with a display list call at RVA `0x0032783c`. The hook sets the
color from the center of the chunk before calling the cached list. It restores
the OpenGL color and lighting state after the distant pass.

With `range_terrain_overlay_test=1`, press `Ctrl+Shift+T` while aiming at a
visible surface. The painter picks the observer from the battle depth buffer
and scans the map with the native range visibility routine, including building
and woodland blocking. It traces in time-bounded batches on the render thread
and submits the completed tile set to the terrain renderer. It starts with
16 m cells and subdivides them to 8 m within 192 m of the observer. Very
large maps use coarser cells to stay within the scan limit. A second press or a
scene change clears the paint. See
[CONFIG.md](../CONFIG.md#range_terrain_overlay_test) for the switch. The old
standalone CokeLOS DLL is removed from the release bundle; its source remains
as a development reference while this integration is validated.
