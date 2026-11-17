
<div align="center">

# Contributing

</div>

## Useful contributions

In order of value:

- a build profile for another Combat Mission title, with the test results from
  [FINDINGS.md](../ai/FINDINGS.md) section 13;
- fixes for the single-thread performance limit;
- `cokelos` occluders and projection, or a replacement LOS overlay;
- removing dead config and dead code paths;
- reproducible bug reports with `coke.log` (set `verbose=1`).

Read [FINDINGS.md](../ai/FINDINGS.md) before changing engine-facing code. It
records what has been verified, which pieces are build-specific, and the safety
rules (fault-safe reads, re-validate before writing, prefer RTTI over geometry).

## Build Profiles.
Build profiles take the shape of a folder named like the corresponding executable of the game eg. /CM Black Sea.exe,
with a vendor tag eg. steam-32.ini, the .ini is populated according to the [README.md](/profiles/README.md)

To upload a profile, either make a simple pull request with the profile, or an issue, or email me directly on aktionkokain@gmail.com

To build a profile for FREE, read [DEVELOPING.md](/docs/user/DEVELOPING.md)


## Checks

Run these before opening a pull request:

```powershell
cargo fmt --check
cargo clippy --all-targets --release -- -D warnings
cargo test
cargo build --release
```

Build the sample mods too if you touched the host API:

```powershell
scripts\build.ps1
```

## Names

Every user-visible name (product, config file, log, mod folder, profile folder,
temp folder, DLL names) lives in [`names.ini`](../../names.ini). `build.rs` embeds it
into the DLL and the scripts read the same file. Do not hard-code a new name in
`src/` or `scripts/`; add it to `names.ini`.

## Pull requests

Say:

- which build(s) you tested (menu, battle, resize, alt-tab, F11);
- which new facts the change relies on, and how you verified them;
- which files a user has to add or replace.

Do not include decompiled code, memory dumps, Ghidra projects or extracted game
assets. `git status` is the last check before you push.

## Security

A mod DLL, a profile or a config runs with the game's privileges. Report
security issues privately; see [SECURITY.md](SECURITY.md).
