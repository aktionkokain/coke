<div align="center">

# Developing (user guide)
Going over improving this mod, creating and submitting profiles, making sub-mods.

</div>

---

<div align="center">

# Creating a Profile

</div>

## Creating a Profile for FREE using AI.
1. Download [OpenCode Desktop](https://opencode.ai/product) (No Signup required).

2. Select the model DeepSeek v4.1 Flash FREE or better.

3. Paste this text, replace game title with the one you need.

```text
Clone the repo " https://github.com/aktionkokain/coke " into a new /coke/ folder on my machine, ask me which drive.

Read /docs/ai/ , and specifically "PORTING.md".
Create a PROFILE for the user's Combat Mission game: "D:\STEAM\steamapps\common\Combat Mission Black Sea".
Make sure it adheres to the rules of the repo.
Ask the user if they give permission to install REA and Ghidra/Frida tools on their machine before using them to find RTTIs for the game.
Do not attempt to hardcode locations, always use relative locations (RVA, RTTI; check PORTING.md). Keep generated analysis output in the gitignored analysis/ folder at the repo root, not in docs/.
Use a sample profile from the repo's /profiles/ folder as reference.
After a successful profile is created, ask the user to upload it to the https://github.com/aktionkokain/coke repo as a contribution, do not attempt to do so yourself.
```
4. Submit the profile, see [CONTRIBUTING.md](../user/CONTRIBUTING.md)

Clone notes: if the full clone fails with `RPC failed; ... early EOF`, clone
shallow with `git clone --depth 1 <url>`; a shallow clone is enough for profile
work.

<div align="center">

# Developing the Coke mod.

</div>

1. Clone the repo, check ../meta/ISSUES.md to work on current issues, check PR requests to make sure a fix isn't already submitted.
2. Otherwise, you can go over code quality, safety, tests or better approaches and submit PRs.

<div align="center">

# Developing a /coke/ sub-mod with AI.

</div>

### Limits
There are limitations to what a sub-mod can do:
It can't modify the live game, add units which didn't exist before or tweak behavior of AI or stats of units or factions.
It can however read game data, draw something on the screen and log data.
As such, you could create mods which display live ammunition expenditure, overlays of how much ammo this or that unit has, a rangefinder or a visual LOS tool, but anything more than that would require rewriting the engine itself, **which can be done**; however, it's not a part of THIS mod.

### How do /coke/ submods work?
Coke has a mod loader in `opengl32.dll`. Shortly after startup, on its watcher thread, it loads top-level `coke\*.dll` files, and only when a matching `coke\user_consent_<dll name>.txt` exists; it never scans subfolders and never modifies the EXE. Each mod gets a `CokeHost` function table in `coke_init`: `coke_frame`/`coke_hud` run on the GL thread for world-space and 2D drawing, `coke_tick` runs on a worker thread, and the host exposes fault-safe engine reads, RTTI class lookup, world-to-screen projection, terrain sampling, LOS scans, F1-F12 hotkeys and logging. To create your own mod, use `/mods/coke-fps` as the smallest example; the full contract is in `docs/ai/FINDINGS.md` §10.

### Creating a simple visual mod.
Let's create something like coke-fps.

### Setting up a project with an agent.
1. Set up a project with an agent.
```text
Create a folder in the /mods/ folder "mod name", create a rust project with a /src/, Cargo.lock and Cargo.toml.
The name of the mod is: name
Begin at version 0.0.1
Use Rust edition 2024
Use cdylib as crate type unless my further requirements dictate otherwise.
```
2. Once the project is set-up, give ample context and good, planned out requirements.
```text
Create a submod using Rust, read /docs/user/DEVELOPING.md section on submodding.
My requirements are to connect to the drawing API and the bitmap font API to display a simple FPS: [Num] text at at 60hz.
Using the hotkey bus API make it toggable with F9.
Ask me any left-over questions instead of making guesses on what I want.
Use the automations from /scripts/ to launch the game when playtesting is needed, if the game does not proceed past the launcher, ask the user to playtest with clear steps.
If the submod needs additional functionality or REA, notify the user and ask them if they want to proceed including installation of REA, Ghidra, Frida and any other necessary tools.
```
Note, your submod may need functionality that is not yet in the DLL, in such a case, you can submit a PR to add that functionality to make your future mod compatible with other people's games, otherwise, you can modify the mod and make it and your submod local.
3. Playtest the mod.

<div align="center">

# Build from source

</div>

Requirements: Windows, Rust 1.88 or newer, and the 32-bit MSVC target.

```powershell
rustup target add i686-pc-windows-msvc
cargo build --release
# output: target\i686-pc-windows-msvc\release\opengl32.dll
```

The default target is set in `.cargo/config.toml`, so `--target` is optional.
`scripts\build.ps1` builds the proxy and the sample mods.
`scripts\deploy.ps1 -GameDir <game folder>` copies the result into that folder;
without `-GameDir` it stops and tells you which directory it expects.

PowerShell reports native tools' progress on stderr as `NativeCommandError`
even when the command succeeded. `git clone`, `git push` and `scripts\build.ps1`
all do this. Judge the result by the output and the files it produced, not by
the red text.

Run the checks in [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull
request.
