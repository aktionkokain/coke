# Porting CokeRange

CokeRange shares its controls, render hook wrappers, surface picking, and
submod API across titles. Each executable build provides its own native
addresses in `[range]` and battle objects in `[scene]` and `[terrain]`.
Start with [general porting](PORTING.md) for profile identity and
[SF2](titles/shock-force-2.md) for a worked example.

## Start from the working build

1. Capture the target build's identity from `profile_dump=1`. Use the live
   image size, timestamp, and canonical text hash in the profile.
2. Open the working CMBS profile beside both binaries in a disassembler. Use
   a rebuilt PE from [DECOMPILING.md](DECOMPILING.md) if a disk executable
   contains a packer stub instead of the engine code.
3. Rank equivalent code in the target image with this helper.

```powershell
python -m pip install -r scripts/requirements-dev.txt
python scripts/match-range-rvas.py 'D:/analysis/cmbs-image-pe.bin' 'profiles/CM Black Sea.exe/steam-32.ini' 'D:/Games/Target Game/CM Target Game.exe'
```

The script compares overlapping 28-byte windows from each working RVA
against the target's executable sections. It masks four-byte x86 immediates
and displacements so a changed global address or call displacement has less
effect. Counts rank candidates for one anchor. Overlapping windows share
evidence, and the search can miss a function whose code was reorganized.
Check top hits against their callers, callees and live behavior.

## Check each profile field

| Field | What to verify |
|---|---|
| `draw_call`, `draw_flush` | The battle render function can drain the same queue several times. Match the exact CMBS call's surrounding instructions and confirm the world viewport, matrices and depth buffer at that point. |
| `hud_call`, `hud_draw` | Confirm the call's target and the view in ECX. Verify the original UI and the font helpers draw in the same pass. |
| `trace` | Follow a native `T` preview caller. Compare the ECX context, the ten x86 stack arguments, callee cleanup, the result in AL, and clear versus blocked cases. A cursor ray answers which surface is under the pointer. Trace visibility separately. |
| `ribbon` | Follow the Target renderer to the styled, pooled ribbon builder. Confirm XMM2, the thirteen stack words, callee cleanup, and the clear and blocked color indices. |
| `point_vtable` | Resolve MSVC RTTI for `Vector3` and confirm the vtable is written to point objects at the trace and ribbon call sites. Points carry the vtable followed by three floats. |
| `ribbon_pool_off` | Find the battle render code that resets the pool. Compare its owner-relative access with the working build and check it in a live battle. Defaults to CMBS's `0xc99c`. |
| `label_draw`, `label_width`, `label_background` | Compare register and stack arguments, the native short-string layout and the world font field. Check placement in the live viewport. |

Check the implicit object fields in `src/range.rs` too. These include the
world's command and selected unit fields (`+0x38`, `+0x8a0`), the trace
context on the scene owner (`+0x1c`), the world font (`+0x40`), and the HUD
view extent (`+0x14`). Confirm the map type and its heightfield dimensions in
a live battle. Convert an on-disk VA using the preferred base and a live
address using the current module base. Keep member offsets relative to their
objects.

The installer checks that both sites contain the original five-byte x86
calls to the supplied destinations. It installs them together within one
page. If a future title splits the sites across pages or changes an ABI,
extend the adapter and verify the new contract before enabling its profile.

## Exercise the result in game

1. Install the rebuilt x86 proxy, matching profile, and CokeRange DLL with its
   consent file. Restart. Confirm the log shows the intended identity,
   `coke: loaded coke_range.dll` and the native hook installation.
2. Open a disposable battle. In idle mode with no unit selected, put the
   cursor on a surface and press `T`. Move to another ground point and check
   the ribbon and range label. Aim across solid cover and check that the
   label and colored segments change when the trace is blocked.
3. Start on both an open tile and a roof. Hover a wall, roof edge, and map
   edge. Check that the line tracks the visible surface and the scene keeps
   its terrain tiles. If the label appears without a ribbon, inspect the
   queue, pool, and hook timing. If neither appears, check the idle command
   state, the battle owner, and the range world and terrain lookups.
4. Cycle all four source heights with `H`. While the preview is active, check
   `E` still moves the camera. Cancel, select a unit, and compare plain `T`
   (game Target) and `Shift+T` (CokeRange). Leave and reopen the battle to
   check stale state and repeat after any supported window mode change.
5. Record the exact build, the map and game phase, observations, and any
   untested cases in the title notes. Keep local binaries, dumps and
   decompiled output outside version control.
