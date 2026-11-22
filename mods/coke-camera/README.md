# Coke Shot

RTS camera controller for the `coke/` host. Extract `coke-mods.zip` into the
game's `coke/` folder and create an empty
`coke/user_consent_coke_camera.txt` to enable it. Camera settings are in
`coke-camera.ini`.

It uses the host's validated camera API rather than CMBS addresses. In RTS mode,
WASD emits constant-speed ground-relative pan commands, RMB emits cursor-delta
yaw/pitch, Q/E orbit, and holding MMB while dragging translates the camera
along its 3D view plane. Shift boosts movement, Control slows it, and wheel
input is coalesced by the host before being turned into gentle zoom steps.

Press F6 to toggle free-camera mode. WASD flies along the camera's current view
and side axes, R/F moves vertically, and RMB drag looks in any direction. Shift
and Control adjust flight speed; the camera stays above terrain while its
altitude is otherwise unrestricted. Set freecam_noclip=1 to disable terrain
and map-boundary collision while free-camera mode is active. Change
freecam_fkey to another available F1-F12 key (F9 and F11 are reserved by the
host).

RMB rotation slows as FOV narrows: zoom_reference_fov sets the unzoomed
reference and zoom_drag_floor sets the minimum sensitivity ratio.

`mmb_sensitivity` controls the view-plane drag distance in world units per
screen pixel (default `0.36`). Movement is applied to the engine's scene-owner
camera controller, including native rollback and terrain-relative height
fields, so tile visibility follows the moved camera.

Set `enabled=0` in `coke-camera.ini` to return to vanilla camera input.

When enabled, the host claims the native camera input boundary: engine
`GetAsyncKeyState` sites see neutral camera input, RMB drag messages and wheel
messages are consumed by the proxy, and this mod receives cursor and coalesced
wheel samples directly. A right-click without a drag is replayed on release so
native unit-order cancellation still works. In free-camera mode, R/F also stop
reaching the engine's elevation bindings. Disabling the mod releases capture
and restores vanilla input. A brief missing render-camera snapshot does not
release input capture, avoiding one-frame native key leaks.
