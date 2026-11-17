<div align="center">

# Changelog

Release notes for the Coke proxy.

</div>

---

## 0.1.0

Initial public release.

- `[scene]` accepts `widget_class` (alias `widget_rtti`) when a build has no
  stable scene-owner global. The proxy resolves that RTTI class from the live
  widget tree instead of following `owner_ptr -> owner_widget_off -> widget`.
  The fallback needs `[canvas]`, and `scene_rect`, the battle layout override
  and `cokelos` all use the same resolver.
- Cold War GOG 32-bit profile is now `verified`: added `[canvas]`
  (`app_ptr = 0x00a73e24`, `app_hwnd_off = 0xbac`) and `[scene]`
  (`widget_class = GameView`, `top_inset = 155`), validated in a live campaign
  battle with repeated resizes and click-to-unit picking.
- Documented the no-owner-global case in `PORTING.md`, `profiles/README.md`
  and `FINDINGS.md`, including the Cold War worked example and the warning
  against unstable "current scene" globals.
- Profiles now ship as one drag-and-drop `PROFILES.zip`. The proxy checks for
  the zip next to the executable on startup, creates `profiles\`, and extracts
  every profile folder into it. The old per-title zips (`CMBS.zip`, `CW.zip`,
  `SF2.zip`) are gone.
- Renamed the project to Coke before the first tagged release.
- Renamed the config to `coke_config.ini` and the log to `coke.log`. The old
  config names are still read; the log is written under the new name only.
- Moved every user-visible name into `names.ini`. `build.rs` embeds the names in
  the DLL and the scripts read the same file.
- Rewrote [DECOMPILING.md](../ai/DECOMPILING.md): REA and Ghidra setup on Windows, the
  image dump, the RTTI recipe, and the profile-authoring loop.
- Added `dump_module=1`: writes the live (unpacked) main module to
  `analysis/<exe name>-image-pe.bin` with file-layout section headers.
- Added `scripts/package.ps1` and a tag-triggered release workflow.
- Fixed broken documentation links and the profile-install instructions.
- Added `LICENSE` (MIT), `CONTRIBUTING.md` and this changelog.
- Added the host-owned F1-F12 hotkey bus (`CokeHost` v4) and the `fps_hotkey`
  setting; the FPS counter and `coke-fps` toggle at runtime (F10 by default).
  `coke-prof` now uses the bus for F7/F8.
- Added per-mod draw layers (`CokeHost.set_layer`) for deterministic ordering in
  both `coke_frame` and `coke_hud`.
- Every title now needs a profile. The embedded CMBS profile was removed; CMBS
  ships as a profile like the other titles, and a missing profile means safe
  mode.
- Removed the unimplemented `scan_diag` setting.
- Initial private build: windowed mode, borderless F11, live UI re-layout,
  alt-tab handling, FPS counter and frame cap, process tuning, profile-driven
  engine anchors, and the consent-gated `coke/` native mod loader.
