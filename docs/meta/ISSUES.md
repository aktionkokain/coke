<div align="center">

# Issues

Known problems and research topics, in no particular order.

</div>

---

- In CM:BS, the intro looks gray compared to the same intro in SF2/CW.
- Need a fast and idiot-proof way for myself and others to quickly create profiles for any titles. Perhaps, point users to OpenCode's freetier DeepSeek V4.1 Flash to build profiles for free?
- Need to add more API surface to the DLL, to make mod creation easier, unified schema of hotkeys: i.e if your mod is not touching the game, but is interacting with it in a debug way like coke-prof, use F1-12 keys for your hotkeys. <- partial, hotkeys added but api is small.

- Need to rewrite the game in Rust, there are like 6k functions, probably impossible without community support. <- actually 15-16k

<div align="center">

## Research

</div>

- Still researching whether a 3d LOS mod is possible, cokelos is bad, stutters, drifts compared to camera, created SPOTTING.md, researching the spotting mechanics in the decompiled game, in order to learn more to implement the mod.
- Need to research batching opengl calls in the DLL. <- plain attempts fail because the game dies on itself?
- Need to research SF2 and CW more, they seem to have more functions from the engine. <- true, 25k and 26k functions vs 16k for cmbs.
- Need to research whether a giga-CM is possible, since we know .brz files are unencrypted, we could decompile the "best" engine out of all CM games, and make a CM game that can play any mission or unit collection from said .brz files. <- would need a rewrite of at least 2 games to verify and merge possibility, but rewriting CMBS is gonna speed up that.

<div align="center">

## Done

</div>

- Need to add hotkey toggling to the FPS counter AND cokefps instead of a config value. (F1-12)
- Need to add z-index managing authority api to the mod, currently it's alphabetical and breaks.
