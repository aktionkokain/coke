# coke-prof

In-process profiler for Combat Mission, loaded through the proxy's `coke/`
loader. It answers "what is actually taking the frame?" without Frida or an
external debugger.

## Install

Build it with `scripts/build.ps1` (or `cargo build --release` in this folder),
then copy next to the game executable:

```text
coke\coke_prof.dll
coke\user_consent_coke_prof.txt   (empty file; consent gate)
coke-prof.ini                     (next to the EXE or in coke\)
```

The proxy must be the version with host API v4 (the `coke_hud` pass, the F1-F12
hotkey bus and the `coke_prof_*` exports). Older proxies reject the mod because
its struct is larger than theirs.

## What it measures

- **Frame**: CPU-side cost of producing a frame (engine work plus proxy
  overlays, measured from the previous swap returning to the next
  `wglSwapBuffers`), with average, p95 and p99. The driver's present/VSync wait
  is reported separately as swap time, so `frame_ms` does not collapse to the
  refresh interval when VSync is on.
- **Per-thread CPU**: `GetThreadTimes` deltas for every game thread. The GL
  thread id is captured in `coke_frame`; worker/audio threads are labelled by
  tid. Process CPU is shown on a 100%-per-core scale, so a multi-threaded
  process can exceed 100%.
- **GL load** (from the proxy): `glCallList`, `glBindTexture`, begin/end,
  matrix push/pop, draw calls, vertices, texture upload bytes, readbacks.
  `timing=1` also accumulates QPC time inside `glCallList`/`glBindTexture`.
- **Hot functions**: samples the GL thread's EIP at `sample_hz` and maps it to
  the nearest function start in `symbols.rs` (generated from a local decompiled
  image per [DECOMPILING.md](../../docs/ai/DECOMPILING.md); addresses only).

## HUD

Drawn at the top-left below the FPS/LOS lines:

```text
PROF 31.2 fps  frame 32.1 / p95 41.0 / p99 48.3 ms  cpu 87%  threads 17
GL  list 10123 12.4ms | tex 8450 3.1ms | begin 95 | push 1460 | draw 95 | verts 1200
GL  upload 4 (0.8 MB) | read 0 | swap 9.8 ms | timed
CPU 1234:61% 5678:18%
HOT FUN_00619220 42% | FUN_00702ba0 23%
```

`F7` resets counters and samples. `F8` toggles the HUD. Both keys are registered
with the host hotkey bus.

## CSV

`coke-prof.csv` next to the EXE gets one row per interval. Counts and GL times
in the CSV are per interval (divide by the frame count or multiply by the
interval to compare runs). `frame_ms`/`p95_ms`/`p99_ms` exclude the
present/VSync wait; `swap_ms` is the average time spent inside the swap. The
columns are:

```text
ms,fps,frame_ms,p95_ms,p99_ms,cpu_pct,threads,
gl_list,gl_list_ms,gl_tex,gl_tex_ms,gl_begin,gl_push,gl_draw,verts,
gl_tex_up,swap_ms,top_func,top_pct
```

Use it to compare an optimisation (display-list batching, texture sorting,
render-scale changes) against the same mission/camera/frame-cap. The historical
baseline is ~10k `glCallList` + ~8.4k `glBindTexture` per frame in a busy
battle; reproduce your own capture with the workflow in
[DECOMPILING.md](../../docs/ai/DECOMPILING.md). Generated artifacts stay in
`analysis/` and out of git.

## Caveats

- Sampling briefly suspends the GL thread. It is off in `sample=0`; if you see
  stutter or instability, disable it first.
- `timing=1` adds QPC overhead to every `glCallList`/`glBindTexture`. Use
  `timing=0` for a pure count run.
- The symbol table is generated from the verified CMBS build. A different build
  needs a new table (`scripts/gen_prof_symbols.py`) and will otherwise show
  wrong names.
- Like every `coke/` mod, the DLL runs with the game's privileges. Only install
  a build you trust.
