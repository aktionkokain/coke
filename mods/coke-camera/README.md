# Coke Shot

Optional RTS camera controller for the `coke/` host. It is enabled by default
after installation, but is distributed separately from the base Coke package.
Download `Coke-Shot.zip`, then copy its DLL, INI, and consent file into the
game's `coke/` folder. The consent file opts into loading the mod.

It uses the host's validated camera API rather than CMBS addresses. WASD emits
constant-speed ground-relative pan commands, RMB emits cursor-delta yaw/pitch,
Q/E orbit, Shift boosts pan speed, Control slows it, and wheel input is
coalesced by the host before being turned into gentle zoom steps.

Set `enabled=0` in `coke-camera.ini` to return to vanilla camera input.

When enabled, the host claims the native camera input boundary: engine
`GetAsyncKeyState` sites see neutral WASD/RMB state, RMB button messages and
wheel messages are consumed by the proxy, and this mod receives the cursor and
coalesced wheel samples directly. Disabling the mod releases capture and
restores vanilla input. RMB down/up messages are forwarded to the game, so
right-click continues to cancel unit orders; only RMB-held mouse-move samples
are blocked from the native camera drag path. A brief missing render-camera
snapshot does not release input capture, avoiding one-frame native key leaks.
