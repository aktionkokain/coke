# Decompiling the game

This is the reproducible workflow behind the engine addresses in
[`FINDINGS.md`](FINDINGS.md) and the build profiles in `profiles/`. It is
written for an agent (or a person) who wants to answer one question about the
engine, record the answer as a fact, and move on.

You need your own legitimate copy of the game. The tooling never redistributes
it.

| You want to | Start here |
|---|---|
| support another Combat Mission title | [PORTING.md](PORTING.md) |
| understand a function or an object layout | this document, then `FINDINGS.md` |
| change engine-facing code | `FINDINGS.md` sections 11 and 12 first |

## What you may publish
- offsets, RVAs, struct layouts, RTTI class names and function signatures;
- your own prose explanation of how a subsystem works;
- scripts that reproduce the analysis;
- build profiles (`profiles/<exe>/<build>.ini`).

Do not commit decompiled function bodies, memory dumps, Ghidra projects,
extracted assets (`.brz`, models, textures, sounds, maps) or a patched
executable. `analysis/`, `*.bin`, `decompiled/` and `*.brz` are already in
`.gitignore`. The analysis tree is local, not part of the project.
---

## 1. Install the tools

Windows 10/11 x64, PowerShell, Node 22 or newer.

```powershell
# REA and its CLI
npm install --global rea-agents

# Ghidra 12.1.x and a full JDK 21, both unpacked by hand
$env:GHIDRA_INSTALL_DIR = "C:\tools\ghidra_12.1.4_PUBLIC"
$env:JAVA_HOME = "C:\tools\jdk-21"
$env:REA_ANALYSIS_PROVIDER = "ghidra"

rea doctor --json
```

`rea doctor` must report Ghidra as available, with the headless launcher and
the native decompiler found. Also install the Rust target used by the proxy:

```powershell
rustup target add i686-pc-windows-msvc
```

Constraints that matter on Windows:

- **32-bit PE support is recent.** Package releases through 5.0.0 only admitted
  x86-64 PE targets on Windows. Check `rea doctor` and use a current release.
- **Unset `REA_GHIDRA_PROJECT_PATH`.** Windows cannot open an existing Ghidra
  project; REA imports into an ephemeral one.
- **The Windows Ghidra provider is read-only.** Function and address
  annotations are rejected, and everything is discarded when the session
  closes. Facts belong in markdown and profile files, not in a project you
  think you saved.
- **A cold import takes minutes** and every new session re-imports. Raise
  `REA_GHIDRA_STARTUP_TIMEOUT_MS` for a large image instead of retrying.
- If Ghidra fails with `java_home.save (Access is denied)`, point `TMP` and
  `TEMP` at a directory the agent host allows child processes to write.

You do not need a Ghidra plugin, and REA does not install Ghidra or Java.

---

## 2. Identify the build

A profile is valid for exactly one build. Get the identity before you analyze
anything.

Start the game manually, directly from the executable when the install supports
it (Cold War GOG does). If a launcher and an intro video exist, use them only to
reach the engine, wait until the engine runs, and do not attach to either. Set
this in `coke_config.ini`:

```ini
[debug]
profile_dump=1
```

Restart, then read the first `profile dump:` line in `coke.log`:

```text
profile dump: module base=0x... image=0x... ts=0x... text_rva=0x... text_size=0x... text_fnv1a=0x... blocks=...
```

`image`, `ts` and `text_fnv1a` are the fields a profile matches on. Do not fill
them from the on-disk PE header: the loader rewrites the in-memory header, so a
static read can disagree with the running image. On the Cold War GOG build the
on-disk timestamp was `0x00555c00` and the live value was `0x68d02464`; the
profile has to carry the live one. The `profile dump:` line is authoritative.
Also record the executable:

```powershell
Get-FileHash "C:\...\CM Black Sea.exe" -Algorithm SHA256
```

The packed CMBS image loads at its preferred base with no relocations. Shock
Force 2 and Cold War are unpacked and have ASLR, so an absolute address from
your run is worthless on another machine. Profiles therefore store RVAs
(`VA - preferred_base`), never absolute addresses.

---

## 3. Dump the running image

The CMBS executable is packed: the code on disk is not the code that runs, and
its user32 import table is built at runtime. Analyze the mapped image.

The proxy can write it for you. Enable the debug switch once:

```ini
[debug]
dump_module=1
```

At startup, after the unpacker has exposed the real image, the proxy writes:

```text
analysis\<exe name>-image-pe.bin
```

and logs `dump: wrote ...`. Turn `dump_module` off again.

The dump is copied section by section through `ReadProcessMemory`, so gaps the
loader left uncommitted cannot fault the game. Section headers are rewritten
from the mapped layout to a file layout (`VirtualSize`/`VirtualAddress` ->
`SizeOfRawData`/`PointerToRawData`), so a decompiler imports it as a normal PE.
The file is the game's code: it stays in `analysis/` and out of git.

If you are on an unpacked title, or you want a dump without the proxy, a Frida
one-liner does the same. Attach to the engine PID (not the launcher) and read
`Process.enumerateModules()[0]` from its base for its size. Keep the result in
`analysis/`.

---

## 4. Import into Ghidra

1. Create a local Ghidra project outside this repository.
2. Import `analysis\<exe>-image-pe.bin` as a PE image (auto-detect). Ghidra
   reads the base and language from the header.
3. Run the default auto-analysis. Add the Demangler and the RTTI analyzers if
   your Ghidra build offers them.
4. Expect CMBS section names to be blank and the import to be big. That is
   normal: the packer zeroes the names, and the proxy falls back to the first
   executable section when it needs `.text`.

Ghidra's default names (`FUN_00618d20`, `LAB_00617ab0`, ...) are what
`FINDINGS.md` uses. They are **per build**: a patch moves them. Prefer RTTI
names, strings and cross-references over raw addresses.

Through REA, this is:

```text
open_binary("D:\cmbs_dll\analysis\CM-Black-Sea-image-pe.bin")
```

then the read-only tools: `search_strings`, `search_procedures`, `xrefs`,
`inspect_pe_imports`, `read_bytes`, `analyze_function`, `procedure_pseudo_code`,
`procedure_callers`, `procedure_callees`.

---

## 5. Work with an agent

A good question is one fact wide:

```text
Open analysis\CM-Black-Sea-image-pe.bin with Ghidra.
Find the function that stores GetSystemMetrics(SM_CXSCREEN) into the app object.
Report the global's RVA, the store instruction, and the object offsets.
Do not write pseudocode into the repository.
```

Rules that keep the output useful:

- Write down the **conclusion**: RVA, offset, class name, calling convention,
  layout, and the evidence (function address, string, xref).
- Keep generated pseudocode local.
- Do not trust a decompiler guess. Every write path in the proxy re-validates
  the target at runtime for a reason.
- Reads from the live process go through `ReadProcessMemory` (the code has
  `rd_u32`/`rd_f32`/`rd_bytes`), never through a raw pointer on a background
  thread.

**Ghidra does not attach to a running process.** It analyzes the dumped image.
For live values, use the Frida tools (`list_frida_processes`,
`start_frida_session`, `load_frida_script`), or write a throwaway `coke/` mod
that reads through the host API. The proxy is the safest live probe because it
already owns fault-safe reads and the class table.

---

## 6. Which facts you need

| Fact | Source | Ghidra? |
|---|---|---|
| Build identity (`image`, `ts`, `text_fnv1a`) | `profile_dump=1` | no |
| user32 IAT slot RVAs | log line `IAT scan: <name> slot=... rva=...` | no |
| RTTI vtables for the six profile classes | log line `rtti: class table resolved by name: ...`, minus the module base | no |
| Cached canvas (`app[0]`, `app[4]`) | the two `GetSystemMetrics` stores | yes |
| Battle scene owner/render target | battle screen constructor; F9 widget dump for the `widget_class` fallback | yes |
| `[los]` world/map/camera offsets | `FINDINGS.md` section 9 | yes |

Only the last three need real reverse engineering. That is why a new title can
get windowed mode, `F11`, FPS and mods in minutes, while full live re-layout is
a focused session of analysis plus live testing: the Cold War `[canvas]` and
`[scene]` work in `PORTING.md` appendix B was one run.

### RTTI without Ghidra

The proxy resolves all six profile classes by scanning the live image for the
mangled MSVC type name, so its log already contains what a `[rtti]` block needs.
The static recipe (used by `analysis/rtti_scan.py` locally) is:

1. Find `.?AV<Class>@@` in the image. That address is the type descriptor name.
2. The type descriptor starts 8 bytes before the name.
3. Find `u32` references to the type descriptor. Each points at a complete
   object locator (COL) field; the COL starts 12 bytes before that pointer.
4. Validate the COL: signature `0`, a small relative offset, and a type-info
   pointer inside the image.
5. Find a `u32` reference to the COL. The vtable starts 4 bytes after the
   reference.
6. Confirm by reading the vtable's class name back through
   `vtable[-4] -> COL -> TypeDescriptor -> name`.

Example from the verified build: `TargetReferencePoint` has type descriptor
`0xd72744`, name `0xd7274c`, vtable `0xbacd30`, object size `0x164`.

### Canvas and scene

Those two are the hard part and are already documented step by step:
[PORTING.md](PORTING.md) sections 5 and 6. Do not guess an address because a
previous build had something there. A wrong write in a 32-bit engine is a
crash, not a warning.

---

## 7. Write the profile

1. Copy an existing profile and rename it for the build:
   `profiles/<exe name>/<build>.ini`.
2. Fill `[profile]` from `profile_dump=1` and `[slots]`/`[rtti]` from the log
   (subtract the module base from any absolute address).
3. Add `[canvas]` and `[scene]` only after the live checks pass.
4. Start the game with `verbose=1`. The `profile:` line must name your build
   and say `status=verified`; then look for `canvas patch` lines.
5. Set `profile=none` once and confirm the game still starts in safe mode.
6. Run the checklist in [`FINDINGS.md`](FINDINGS.md) section 13.

The schema and the trust boundary are in
[`profiles/README.md`](../../profiles/README.md).

---

## 8. Validate and contribute

```powershell
cargo fmt --check
cargo clippy --all-targets --release -- -D warnings
cargo test
cargo build --release
```

Reject your own commit if `git status` shows:

- `analysis/`, `decompiled/`, `ghidra-project/`
- `*.bin`, `*.brz`, `*.exe`, `*.pdb`, `*.idb`, `*.i64`, `*.gpr`
- large `.c` or `.jsonl` files you did not write
- extracted game art, maps or sounds

A pull request should say what you verified (menu, battle, resize, alt-tab),
which build it targets, and which facts are new. Profiles and prose are the
most valuable contributions.

## Links

- [REA](https://github.com/morluto/rea) and the
  [Windows Ghidra setup guide](https://github.com/morluto/rea/blob/main/docs/providers/windows-ghidra-p0.md)
- [Ghidra](https://ghidra-sre.org/)
- [Rust target setup](https://rustup.rs/)
- [FINDINGS.md](FINDINGS.md): the verified map and the safety rules
- [PORTING.md](PORTING.md): canvas, scene and the port checklist
- [profiles/README.md](../../profiles/README.md): profile schema
