# SPOTTING: targeting (`T`), range and line of sight in CM Black Sea

Status: working notes from the shipped 32-bit Steam build. Everything in
**Verified** was read from the binary or its data files. **Inferred** means the
behaviour is consistent with the code/data but the exact function has not been
pinned down yet. Keep decompiled bodies out of the repository; see
[DECOMPILING.md](../DECOMPILING.md).

## 0. Short version

`T` is the engine's **Target** command, not a dedicated rangefinder. With a
unit selected it enters target-pick mode. The engine then:

1. builds a **target reference point** from the cursor / hovered object,
2. computes the horizontal range from the shooter to that point,
3. evaluates the engine's **LOS categories** and **spotting contacts**,
4. draws the target line and its status (range + whether the target is in
   sight / spotted / blocked).

The line is not a terrain-only ray. Trees, buildings, vehicles, trenches,
minefields, smoke and unit silhouettes participate through the engine's
spotting/LOS data. `cokelos` only marches the terrain heightfield, which is why
it can show green through a tree line that the game correctly reports blocked.

**Open question:** the code that fills `TargetReferencePoint` is now pinned
(`FUN_00685cf0`, case 7; see section 2), but the LOS evaluator it feeds has not
been reduced to one address. The command path, data types, UI widget and terrain
anchors below are verified; the LOS call graph is the next RE step.

## 1. Input path: how `T` becomes a command

`Data/hotkeys.txt` is the authoritative key map. The relevant block is:

```text
Command Keys Direct
//FAST
<E>B
...
//TARGET
<E>T
//TARGET LIGHT
<E>Y
//TARGET ARC
<E>K
```

**Verified:** `T` maps to the `TARGET` command in `Data/hotkeys.txt`. The
parser reads command names as comments and the next line as the key, so the
key is data-driven; editing `hotkeys.txt` moves the command without touching
the exe.

**Verified:** the engine has a `TargetReferencePoint` RTTI type
(`.?AVTargetReferencePoint@@`, VA `0xd7274c`) and target-specific cursor
resources (`cursor target`, `cursor arrow 3d`, `cursor arrow 2d` in the cursor
theme table at `0x748fd0`).

**Inferred / operational:** the command is dispatched like every other unit
order. With a unit selected, the engine enters a pick mode; the next click
commits the target order. `TARGET LIGHT`, `TARGET ARC`, `TARGET ARMOR ARC`,
`TARGET BRIEFLY`, `TARGET SMOKE` and `CLEAR TARGET` are sibling commands that
change the same target reference data (light/arc/brief/smoke flags) rather than
the LOS maths.

### Target mode vs. a plain cursor probe

- `T` targets a **unit/point for the selected unit's weapons**. It creates or
  updates the unit's target reference.
- The range/LOS readout is a side effect of that target reference. There is no
  separate "measure" command in `hotkeys.txt`; the same line is used to judge
  a fire order.
- `Y` (target light) and `K`/`M` (arcs) reuse the target reference with
  different engagement constraints.

## 2. The target reference point and the UI arrow

`TargetReferencePoint` is the engine object that stores what the player is
aiming at. At the gameplay level it contains at least:

- the world position of the point (x east, y north, z up),
- the target object/unit if the cursor is over one,
- the selected shooter,
- the target type/flags (area, light, arc, smoke, brief),
- the range and LOS/spotting result used by the HUD and the AI.

**Verified:** the type exists and is distinct from `Spottable`/`SpotContact`.

RTTI and allocation anchors, read from the unpacked image
(`analysis/cmbs-image-pe.bin`, identical in `cmbs-mainmodule.bin`; produce both
with `dump_module=1` as described in [DECOMPILING.md](../DECOMPILING.md)):

```text
TargetReferencePoint type descriptor   0xd72744
TargetReferencePoint name string       0xd7274c  ".?AVTargetReferencePoint@@"
TargetReferencePoint primary vtable    0xbacd30
SpotContact type descriptor            0xd70674
SpotContact name string                0xd7067c
SpotContact primary vtable             0xba7bf8
```

The image contains one `TargetReferencePoint` type descriptor. Two
complete-object locators point at it, but only one of them (`0xbc26e0`) is
attached to the vtable (`0xbacd30`) that code references.

**Verified allocation path:** `FUN_00685cf0` is a switch on a record type at
`+0xa8`; case `7` allocates `0x164` bytes, writes vtable `0xbacd30`, copies the
world position from the order into the new object at `+0x30`, `+0x34` and
`+0x38`, and registers it with `FUN_007c2af0`. It also installs secondary vtable
pointers at `+0x90` and `+0x140`. `FUN_007c3930` walks a pending list and calls
`FUN_00685cf0` for every entry, which identifies this as the conversion from an
order or packet to a live target reference. `FUN_007d4da0` also references the
same vtable, so it is a second construction or copy path.

This closes the older "which function fills `TargetReferencePoint`" question.
The LOS evaluator that consumes the point is still not pinned to one address.

**Verified UI detail:** `FUN_0057f600` constructs a normal engine widget from
the asset name `specialist target arrow` (the constructor calls the widget base
`FUN_007990A0` and stores the widget vtable `PTR_FUN_00b99e38`). The target line
is therefore a 2D UI widget fed by the target reference data, not a 3D line
drawn by the renderer. The cursor theme table at `FUN_00748fd0` also loads
`cursor target`, `cursor arrow 3d` and `cursor arrow 2d`, so target mode is a
distinct cursor/command state.

The engine's world coordinates are metres, x east / y north / z up (see
[FINDINGS.md](../FINDINGS.md) §9). The target point is taken from the cursor's ground hit; for
a hovered unit the engine substitutes the unit's position/centre.

## 3. Range

Range is the horizontal distance between the shooter and the target reference
point:

```text
range_m = sqrt((tx - sx)^2 + (ty - sy)^2)
```

It is shown in metres. The vertical component is not part of the displayed
range; it is handled separately by the LOS/ballistic checks. The same distance
is used by the weapon range checks (can this weapon reach?), the AI's target
selection, and the target line's length.

**Inferred detail:** for a unit target the engine measures to the unit's
reference point/centre, not to the cursor pixel; for an area target it measures
to the ground point under the cursor. This is why the displayed range can
change slightly when the cursor moves from the ground next to a vehicle onto
the vehicle itself.

## 4. LOS: what the engine actually tests

The engine separates **line of sight** (geometry: is there a clear line?) from
**spotting** (detection: can this unit actually see/identify the target?). The
`T` readout exposes both.

### 4.1 Terrain

- The map is an `ActionMap` / `TerrainMap` (see [FINDINGS.md](../FINDINGS.md) §9). The height
  field is a grid of vertex heights; the gameplay grid is the **action spot**,
  8 m on the shipped build (`cokelos` uses `cell=8` because that is the game's
  action spot).
- The LOS ray samples the terrain between the eye point and the target point.
  A crest that rises above the ray blocks it. `cokelos` uses a 1 m step and a
  20 cm clearance margin to approximate this; the engine's own sampling is at
  action-spot resolution with its own clearance rules.
- The eye point is the shooter's optics/eye height above the ground, not the
  unit's origin. Target height is the target's centre/aim point.

### 4.2 Objects and occluders

Terrain alone is not enough. The engine's world contains objects that can block
or attenuate LOS. Verified RTTI names include:

```text
.?AVSpottable@@
.?AVSpotContact@@
.?AV?$FastMemory@VLOSCategories@@@@
.?AV?$MemoryBlock@VLOSCategories@@@@
.?AVPacket_TrenchSpotting@@
.?AVPacket_FlavorObjectSpotting@@
.?AVPacket_MinefieldSpotting@@
.?AVPacket_TreeDamage@@
.?AVPacket_BuildingState@@
.?AVPacket_BuildingWallDamage@@
.?AVPacket_BuildingWallBlowout@@
.?AVPacket_SmokeBlack@@
.?AVPacket_SmokeGray@@
.?AVPacket_SmokePuff@@
.?AVPacket_ShowExplosion@@
```

The important split:

- **`Spottable`**: something that can be seen or can contribute a spotting
  contact (units, weapons, objects).
- **`SpotContact`**: a record that a spotter has seen a target. These are the
  things the AI/UI consume ("this unit is spotted by that observer").
- **`LOSCategories`**: the category/bitmask the LOS evaluator returns. It lets
  callers distinguish "clear", "blocked by terrain", "blocked by object",
  "smoke", "not spotted", etc., instead of a single boolean.
- **`Packet_*`**: state changes that invalidate or update spotting:
  trees damaged/falling, building walls damaged/blown out, smoke puffs, trench
  occupation, minefield spotting.

Operationally, the LOS test walks the target line and asks the map/objects for
the highest blocking surface along it. Trees and buildings are not terrain
height samples; they are objects with their own height/extent and damage state.
Smoke adds an obscurant volume. A building wall that has been blown out changes
the result, which is why `Packet_BuildingWallBlowout` exists.

### 4.3 Spotting / detection

LOS is necessary but not sufficient. The engine also evaluates whether the
target is **spotted**:

- the shooter's optics/sensors and experience,
- target size, posture, concealment and movement,
- smoke/dust/weather,
- suppression and visibility of the target's signature,
- the target's own `Spottable` state and any existing `SpotContact`.

The `T` HUD therefore can show a line that is geometrically clear but still
"not spotted", or a target that is spotted but temporarily blocked. The exact
UI wording is localized in the game data, but the underlying distinction is
present in the RTTI/data model above.

### 4.4 What the categories are for

`LOSCategories` is used beyond the player's target line:

- the AI's target acquisition and fire permission,
- the artillery "fire mission los" editor option (verified string at
  `0xb99d3c`),
- spotting contacts shared between units,
- scenario/quick-battle setup and debugging.

This is why the target line can disagree with a naive ray: it is the same
evaluator the AI uses, including object state and spotting history.

## 5. Why `cokelos` differs from the game's `T`

`cokelos` (`src/los.rs`) is deliberately terrain-only:

- it copies the height field and marches a ray at 1 m steps,
- it uses a 20 cm clearance margin to approximate the engine's crest rule,
- it has no tree, building, vehicle, trench, minefield or smoke occluders,
- it has no spotter optics/concealment model and no `SpotContact` history.

Consequences:

- A terrain crest is usually close to the engine's answer.
- A tree line, building, vehicle or smoke screen is **not** represented. The
  mod can report "visible" where the game reports "blocked".
- The mod's whole-map scan is expensive; it runs on the worker thread but
  competes for memory bandwidth and can cause frame-time spikes (observed
  ~1.5 s scans in `coke.log`).

The label "terrain LOS" is therefore accurate; do not present it as the
engine's spotting model.

## 6. Data model and addresses

World/terrain anchors (verified; see [FINDINGS.md](../FINDINGS.md) §9 and the profile):

```text
WORLD_PTR          0x00d7a6ac   global -> world object
SCENARIO_PTR       0x00d7a6b8   global -> scenario/battle
MAP_GLOBALS        0x00d74ea4 / 0x00d74ea8 -> live ActionMap
world + 0x310                  -> ActionMap (TerrainMap subclass)

ActionMap/TerrainMap:
  +0x04  world width  (f32, metres)
  +0x08  world height (f32, metres)
  +0x9c  height array pointer
  +0xa0  vertex-grid width
  +0xa4  vertex-grid height
```

RTTI names (verified strings; addresses are VAs on the shipped image):

```text
.?AVSpottable@@                      0xd70068
.?AVSpotContact@@                    0xd7067c
.?AV?$FastMemory@VLOSCategories@@    0xd705b4
.?AVTargetReferencePoint@@           0xd7274c
.?AVPacket_TrenchSpotting@@          0xd6ea10
.?AVPacket_FlavorObjectSpotting@@    0xd702e8
.?AVPacket_MinefieldSpotting@@       0xd70610
.?AVPacket_TreeDamage@@              0xd708a4
.?AVPacket_BuildingState@@           0xd6f608
.?AVPacket_SmokeGray@@               0xd710e8
```

Resolved vtables and the target reference layout (verified):

```text
TargetReferencePoint vtable           0xbacd30
SpotContact vtable                    0xba7bf8
TargetReferencePoint object size      0x164 bytes
  +0x00  primary vtable
  +0x30  world x (f32)
  +0x34  world y (f32)
  +0x38  world z (f32)
  +0x90  secondary vtable
  +0x140 secondary vtable
```

`0xd6ff30`/`0xd7274c` etc. are string addresses, not function addresses. The
game loads at `0x00400000` and does not relocate, so VA = image base + RVA.
Vtables are addresses in `.rdata`; their first entry is the scalar deleting
destructor and the entries after it are the class virtual methods.

Known engine loop anchors from the analysis (for profiler symbols, not LOS):

```text
FUN_00619220   main loop
FUN_00702ba0   render dispatch
```

## 7. How to reproduce the `T` result in a mod

To match the game rather than approximate it, a mod needs four layers:

1. **Target point**: ground hit under the cursor, or the hovered unit's
   reference point. The host already exposes `cursor_ground` and
   `selected_unit`/`unit_pos`. The engine's own object is now identified
   (`FUN_00685cf0` case 7, vtable `0xbacd30`, position at `+0x30`), but a mod
   still needs a validated way to read the live instance.
2. **Terrain LOS**: current `los_segment`/`los_scan` is a good first pass.
3. **Object occluders**: walk the objects whose bounds intersect the segment
   and use their height/extent and damage state. This is the missing piece in
   `cokelos`; it needs the object list, tree/building extents and the wall
   damage flags.
4. **Spotting**: optics, concealment, smoke, contacts. This is not a geometry
   test and cannot be derived from terrain alone.

A practical order for the next iteration:

- add a debug dump of every object whose bounds cross the target line,
- compare the engine's `T` colour/status with the mod's terrain result on the
  same frame,
- add tree/building occluders first (largest visual disagreement),
- then smoke,
- then spotting (requires the `SpotContact`/`Spottable` data).

## 8. Verification checklist

1. Select a unit, press `T`, hover flat ground: range is the horizontal
   distance and the line is clear.
2. Hover a hull-down vehicle behind a crest: the line is blocked or the target
   is unspotted even though the range is short.
3. Hover a unit behind a tree line/building: the game blocks; `cokelos` may
   show visible. This is the known occluder gap.
4. Blow a hole in a building wall and repeat: the result changes, proving the
   wall-damage packets feed the LOS state.
5. Pop smoke and repeat: smoke changes the result without changing terrain.
6. Compare the target line with an artillery "fire mission los" check: both
   should agree on the same geometry.

## 9. References

- `Data/hotkeys.txt`: `TARGET` key binding.
- [DECOMPILING.md](../DECOMPILING.md): tooling, `dump_module`, the image dump and
  the RTTI recipe. Use it to generate the local Ghidra output, the function
  listing and any profiling capture; those artifacts stay in `analysis/` and
  out of git.
- [FINDINGS.md](../FINDINGS.md) §9: world/terrain/camera anchors.
- `src/los.rs`: terrain-only approximation and its limits.
