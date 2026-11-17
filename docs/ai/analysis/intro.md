# CMBS intro background is grey instead of black

Status: open. Documented from a local run; not fixed yet.

## Observation

CMBS, SF2 and Cold War all play the same Battlefront intro video. The
`intro.wmv` files are byte-identical. With the proxy installed, the CMBS intro
shows the video in a grey rectangle on an otherwise black window. SF2 and Cold
War show the same video with a black background, so the rectangle is much less
visible there.

The grey region is the video itself, not the letterbox around it: the rest of
the proxy window is already pitch black.

## Video facts

`Data/intro.wmv` (CMBS install):

```text
video: wmv3 (WMV9 Main), 640x480, 4:3, yuv420p, 30 fps, 5.014 s
audio: wmav2, 44.1 kHz stereo
```

Decoding the file with ffmpeg and reading corner pixels gives `(0, 0, 0)` at
both 0.1 s and 2.0 s, so the encoded background is true black. The grey is
introduced by the playback/blit path, not baked into the file.

## Runtime evidence

A local CMBS window trace (`analysis/window-trace.log`, captured with the
analysis setup in [DECOMPILING.md](../DECOMPILING.md); it stays out of git)
shows the intro is a DirectShow graph before the engine window exists:

```text
[ 58 ms] CreateWindowExW class="FilterGraphWindow" title="ActiveMovie Window" ...
[249 ms] CreateWindowExW class="VideoRenderer"     title="ActiveMovie Window" ...
[384 ms] CreateWindowExW class="VideoRenderer"     title="ActiveMovie Window" ...
[5517 ms] DestroyWindow FilterGraphWindow
[5522 ms] CreateWindowExA class="CM Black Sea" ...
```

Near the end of the intro the trace counts GDI presentation calls:

```text
[5004 ms] CALLS glFlush=212 GetDC=15 ReleaseDC=20 SetDIBitsToDevice=2 BitBlt=2
```

So the intro is not drawn through the proxy's GL hooks. It is a DirectShow/GDI
path outside the engine's normal swap loop. That is also why changing the GL
clear colour or the canvas has no effect on it.

## Likely causes, in order

1. **DirectShow renderer background.** The CMBS graph uses the legacy
   `VideoRenderer`/`FilterGraphWindow` classes. Their window background or
   letterbox fill may be the system window colour (grey), while SF2/CW use a
   renderer path that fills black. Setting the video window class background
   brush to `BLACK_BRUSH` (or handling `WM_ERASEBKGND` with black) would prove
   this cheaply.
2. **YUV -> RGB range conversion.** WMV9 is normally studio range (16..235).
   A path that does not expand the range renders `Y=16` as dark grey. This
   matches the visible rectangle, but not the pure-black surround; check the
   actual RGB value at the video corner.
3. **Grey DIB padding during the GDI blit.** If the engine allocates a
   `CreateDIBSection` larger than the decoded frame and only blits the 640x480
   video into it, the untouched padding stays at the DIB's initial colour.
   Hook `SetDIBitsToDevice`/`BitBlt`/`StretchDIBits` and inspect the destination
   rect and the corner pixels.

## Next steps

1. Reproduce with the proxy and capture the intro window's class, style,
   client rect, background brush and the corner pixel value of the grey area.
2. Compare those values with SF2/CW on the same machine.
3. Try the cheap renderer fix first: set `GCLP_HBRBACKGROUND` for the
   `FilterGraphWindow`/`VideoRenderer`/`ActiveMovie Window` classes to
   `GetStockObject(BLACK_BRUSH)`, invalidate, and re-run the intro.
4. If the grey survives, hook the GDI blit used for the intro and either
   expand the YUV range or clear the destination DIB to black before the blit.
5. Keep the fix outside the engine profile: the intro runs before the engine
   window and must work in safe mode too.
