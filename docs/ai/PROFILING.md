# Profiling Combat Mission Black Sea

Two tools answer different questions:

| Tool | Where it runs | Answers |
|---|---|---|
| `mods/coke-prof` | inside the game, through the `coke/` loader | per-frame cost, GL call counts, per-thread CPU, hot engine RVA |
| ETW (`wpr` + WPA/WPAExporter) | outside the game, kernel sampler | real call stacks, present/driver/lock time, which module is busy |

Use `coke-prof` for the build → run → compare loop. Use ETW when you need to
know *why* a frame is expensive and the leaf-PC histogram is not enough.

This document assumes the Steam 32-bit build, `i686-pc-windows-msvc`, and the
verified CMBS profile. The proxy and the mod must be rebuilt together: the
profiler ABI is `CokeProfStats` version 2.

---

## 1. Build and install

```powershell
.\scripts\build.ps1
.\scripts\deploy.ps1 -GameDir 'D:\STEAM\steamapps\common\Combat Mission Black Sea' -WithMods
```

That copies `opengl32.dll`, `coke_config.ini`, the matching
`profiles\<exe>\` folder, `coke\coke_prof.dll` and `coke-prof.ini`. The loader
still needs consent:

```text
coke\user_consent_coke_prof.txt    (empty file; its existence is the consent)
```

Delete the consent file to stop the mod from loading. `coke-prof.ini` is read
once at process start; edit it before launching the game.

---

## 2. Launch the game for a measurement

Use the shortest launch path the install supports. If the executable starts on
its own (Cold War GOG does), start it directly and skip the launcher and intro
steps; they apply only to builds that ship them.

1. If the title has a launcher, start it and press **Play** with a real mouse
   click. The Steam launcher ignores synthetic clicks (`FINDINGS.md`, "Operating
   notes").
2. If a DirectShow intro plays, wait through it (about 3 s). It is not the
   engine and must not be profiled.
3. Wait until exactly one `CM Black Sea.exe` has a stable main window. Startup
   creates and destroys a temporary full-monitor window before the real one.
4. Only then attach a profiler or start a capture.

Check what is actually running before every run:

```powershell
Get-Process Launcher,'CM Black Sea' -ErrorAction SilentlyContinue |
    Select-Object Id,ProcessName,StartTime,MainWindowTitle,Responding |
    Format-Table -AutoSize
```

### One engine process, one stable window

The launcher, when present, is only the way to start the game; it is never part
of the measurement. Before capturing, confirm there is exactly one
`CM Black Sea.exe` with a stable main window. Kill a stale engine process from an
earlier run instead of profiling the wrong PID, and never attach to the launcher
or the DirectShow intro.

If you automate the Play click, send it once and then wait for the engine
window. Two engine processes would interleave CPU and GPU work and invalidate
the comparison.

---

## 3. Using `coke-prof`

### Configuration

`coke-prof.ini` next to the EXE (or in `coke\`):

```ini
enabled=1        ; 0 = decline to load
hud=1            ; draw the HUD; F8 toggles at runtime
sample=1         ; EIP sampling of the GL thread; F7 resets counters
sample_hz=25     ; 1..200
interval_ms=1000 ; HUD/CSV update period
top=3            ; hot functions shown
log=1            ; append coke-prof.csv next to the EXE
timing=0         ; QPC timing inside glCallList/glBindTexture
```

`timing=0` is the default and the right setting for comparing runs. `timing=1`
adds a QPC pair around every `glCallList`/`glBindTexture`; use it only for a
short run that needs per-call time.

For a low-distortion A/B run, also set `sample=0`. That keeps the always-on
counters and per-thread CPU and removes the 25 Hz thread suspension. Use
`sample=1` when you specifically want the `HOT` lines.

### Reading the HUD

```text
PROF 31.2 fps  frame 12.1 / p95 18.0 / p99 24.3 ms  cpu 87%  threads 17
GL  list 10123/f 4.1ms | tex 8450/f 1.2ms | begin 95/f | push 1460/f | draw 95/f | verts 1200/f
GL  upload 4 (0.8 MB) | read 0 | swap 3.8 ms
CPU 1234:61% 5678:18%
HOT FUN_00619220 42% | FUN_00702ba0 23%
```

- `frame` / `p95` / `p99` are **CPU-side** frame cost: from the previous swap
  returning to this frame reaching `wglSwapBuffers`. They exclude the driver's
  present/VSync wait, so they do not collapse to 16.7 ms under VSync.
- `swap` is the time spent inside the present. `frame + swap` is approximately
  the wall-clock frame interval (what FPS shows).
- `GL .../f` values are per frame for the interval. `list` is `glCallList`,
  `tex` is `glBindTexture`, `push` is matrix push/pop, `draw` is
  `glDrawArrays + glDrawElements`, `verts` is submitted vertices.
- `CPU` rows are `GetThreadTimes` deltas; `HOT` is the share of GL-thread
  instruction-pointer samples in the interval (not wall time).

`F7` resets counters, samples and the frame ring. `F8` toggles the HUD.

### The CSV

`coke-prof.csv` is appended next to the EXE, one row per interval:

```text
ms,fps,frame_ms,p95_ms,p99_ms,cpu_pct,threads,
gl_list,gl_list_ms,gl_tex,gl_tex_ms,gl_begin,gl_push,gl_draw,verts,
gl_tex_up,swap_ms,top_func,top_pct
```

Counts and GL times are per interval; divide by the frame count for per-frame
values. `frame_ms` is the CPU-side frame cost; `swap_ms` is present time.
`p95_ms`/`p99_ms` come from a rolling 256-frame window, so they are not strictly
interval-local.

### A/B protocol

1. Fix the scenario: same mission, camera, zoom, frame cap, VSync, window size,
   process priority and mod set.
2. Warm up for a few seconds after loading; discard the first interval.
3. Capture 3 × 30–60 s of the same behaviour. Compare the **median** interval,
   not a single row.
4. Primary metric: `frame_ms` (p95 for stutter). Secondary: `gl_list`/`gl_tex`
   per frame, `swap_ms`, `cpu_pct`.
5. Keep the CSV next to the build you measured. The historical baseline (~10k
   `glCallList`, ~8.4k `glBindTexture` per frame in a busy battle) comes from a
   local profiling run; the recipe to produce and read your own capture is in
   [DECOMPILING.md](DECOMPILING.md). Generated artifacts stay in `analysis/` and
   out of git.

### Symbols

`HOT` names come from `mods/coke-prof/src/symbols.rs`, generated by
`scripts/gen_prof_symbols.py` from the local decompiled analysis (RVA extraction
and the image dump are described in [DECOMPILING.md](DECOMPILING.md); keep the
generated tree in `analysis/`, never in git). It contains RVAs and Ghidra names
only. It belongs to the verified CMBS build; another patch or another title
needs a new table. The sampler only checks `eip >= module_base`, so a sample in
another module above the image is attributed to the nearest engine symbol. Treat
`HOT` as a hint, not proof.

---

## 4. Using ETW

ETW is a kernel sampler: roughly 1 kHz, no thread suspension in the target, and
it walks user stacks. It needs an elevated shell and does not require building
or installing anything into the game.

### Capture

```powershell
# elevated
wpr -start CPU -filemode
# press Play once, load a battle, play the same 30-60 s
wpr -stop "$PWD\cmbs-cpu.etl"
```

Low-level equivalent, if you prefer xperf:

```powershell
xperf -on PROC_THREAD+LOADER+PROFILE -stackwalk Profile
xperf -d "$PWD\cmbs-cpu.etl"
```

`wpr -cancel` aborts a capture. The ETL is tens to hundreds of MB; keep it out
of git and delete it after export.

### Inspect

Open the ETL in Windows Performance Analyzer:

```powershell
& "${env:ProgramFiles(x86)}\Windows Kits\10\Windows Performance Toolkit\wpa.exe" cmbs-cpu.etl
```

On this machine the Windows Kits 8.1 toolkit is installed under
`C:\Program Files (x86)\Windows Kits\8.1\Windows Performance Toolkit\`.
If that WPA cannot open a newer trace, install the current Windows ADK.

Useful views:

- **CPU Usage (Sampled)**: expand `Stack`, filter `Process` to
  `CM Black Sea.exe`. This is your hot-path data, with caller chains.
- **CPU Usage (Precise)**: shows waits and `ReadyThread`; use it for lock,
  present and scheduling questions.
- **GPU Usage**: if the capture includes the GPU profile, not the plain CPU
  profile.

Set symbols with `_NT_SYMBOL_PATH`, but the game has no PDB. Expect frames like
`CM Black Sea.exe+0x619220` (module + RVA). `opengl32.dll` will be the proxy
when it is installed; its offsets show the proxy/overlay cost. System DLLs can
resolve through the Microsoft symbol server.

### Export and map RVAs

`wpaexporter` needs a WPA profile containing the tables you want. Create one in
WPA (drag `CPU Usage (Sampled)` into the analysis, then save the profile), then:

```powershell
wpaexporter -i cmbs-cpu.etl -profile cmbs-cpu.wpaProfile -outputfolder out -symbols
```

`tracerpt cmbs-cpu.etl -o cmbs.csv -of CSV` is a raw-event fallback; it does not
aggregate stacks.

To turn `CM Black Sea.exe+0x...` into `FUN_...`, use the same table as the mod:
read `mods/coke-prof/src/symbols.rs`, sort by RVA, and pick the nearest entry at
or below the sampled RVA. Do not apply the table to `opengl32.dll` or system
modules.

### ETW limits

- Requires admin (kernel trace) and captures the whole system; do not publish
  ETLs containing unrelated processes.
- Stacks for a 32-bit WOW64 process are user-mode stacks; the kernel transition
  can truncate a chain, but the engine frames are still there.
- ETW has no notion of a game frame. Pair an ETW capture with the
  `coke-prof` `frame_ms` numbers from the same scenario; ETW tells you where the
  time went, `coke-prof` tells you what it cost per frame.

---

## 5. Quick alternatives

- `analysis/perf_sample.ps1 -Seconds 6` prints process CPU, per-thread CPU and
  GPU utilisation for the focused game. No stacks, no admin.
- `analysis/frida/run_profile.py --seconds 25` attaches out-of-process, samples
  the `wglSwapBuffers` thread and prints module+RVA histograms. It is the
  quickest way to get a stackless hot-RVA list without rebuilding.
- `analysis/frida/counts.js` counts GL entry points with CModule stubs, useful
  when you want per-call detail the proxy does not expose.

These live under `analysis/`, which is gitignored; they are local tools, not
part of the shipped proxy. [DECOMPILING.md](DECOMPILING.md) covers the tooling,
the image dump and the local analysis tree they run against; generate the
scripts and any baseline yourself, or take the numbers above as the starting
point.

---

## 6. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| No HUD, no CSV | consent file missing, `enabled=0`, or the mod was not built into `coke\` |
| `GL counters unavailable` | proxy and mod versions differ; rebuild both (`CokeProfStats` v2) |
| `frame_ms` equals the refresh interval | old proxy without `coke_prof_last_cpu_frame_ns`; rebuild both |
| Stutter while the HUD is on | set `sample=0` or lower `sample_hz`; sampling suspends the GL thread |
| `HOT` names look wrong | symbol table is for the verified CMBS build; regenerate it for another build |
| ETW has no stacks | capture with `PROC_THREAD+LOADER+PROFILE` / `wpr -start CPU`; enable stack walking and symbols |
| ETW shows no `CM Black Sea.exe` | captured the launcher or the intro; wait for the engine window and capture again |
