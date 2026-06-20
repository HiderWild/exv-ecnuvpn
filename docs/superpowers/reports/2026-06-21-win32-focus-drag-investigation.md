# Win32 Focus Drag Investigation

## Reproduction

- Active-window drag: log captured normal system move-loop entries.
- Inactive activation-only click: log captured `startDrag` calls that did not always enter a move loop.
- Inactive immediate drag: log captured at least one clear jump signature.
- Tray restore immediate drag: user reported no strange behavior; log does not point to tray restore as the primary path.

## Trace Summary

- Trace file: `%TEMP%\exv-win32-drag-trace.log`
- Copied workspace trace: `exv-win32-drag-trace.log`
- Total log lines at first analysis: 425; later file continued to 28 `start-drag-enter` events.
- `start-drag-enter`: 28
- `start-drag-after-foreground`: 0
- `WM_MOUSEACTIVATE`: 0
- `WM_ENTERSIZEMOVE`: 24 traced pairs (`wndproc-enter` plus `enter-size-move`)
- `WM_EXITSIZEMOVE`: 24 traced pairs (`wndproc-enter` plus `exit-size-move`)

## Key Evidence

The strongest jump signature is this sequence:

```text
63081593 WM_ACTIVATE fg=1 cursor=2030,35 rect=225,155,1440,859
63081593 WM_SETFOCUS fg=1 cursor=2030,35 rect=225,155,1440,859
63081593 WM_KILLFOCUS capture=465224 cursor=2030,35 rect=225,155,1440,859
63081593 start-drag-enter fg=1 capture=465224 cursor=2030,35 rect=225,155,1440,859
63081609 start-drag-before-send WM_NCLBUTTONDOWN cursor=2030,35 rect=225,155,1440,859 lparam=2295790
63081609 WM_ENTERSIZEMOVE cursor=2030,35 rect=225,155,1440,859
63081609 WM_CAPTURECHANGED cursor=2030,35 rect=-681,289,534,993
63081609 WM_EXITSIZEMOVE cursor=2030,35 rect=-681,289,534,993
```

The cursor at `2030,35` is outside the top-level window rect `225,155,1440,859`, yet the host sends a synthetic `WM_NCLBUTTONDOWN` with `HTCAPTION`. Windows then enters the move loop and immediately moves the window to `-681,289,534,993`.

## Current Root-Cause Hypothesis

Hypothesis: the main jump is caused by the renderer-driven synthetic drag path starting a system caption move with an invalid current cursor position after focus/activation transfer. The current trace does not support `SetForegroundWindow()` as the direct trigger because the `start-drag-after-foreground` branch did not execute.

Confidence: Medium-high for the invalid synthetic drag coordinate being the immediate cause; medium for the upstream cause being renderer/WebView focus/capture ordering.

Evidence:

- The bad sequence sends `WM_NCLBUTTONDOWN/HTCAPTION` while `GetCursorPos()` is outside the top-level titlebar/window rect.
- `start_window_drag()` was called while the WebView child still had capture (`capture=465224`).
- No `WM_MOUSEACTIVATE` was observed at the top-level host during the trace.
- No `start-drag-after-foreground` event was observed, so the explicit foreground-promotion branch was not hit in this run.

Unknowns:

- Whether the renderer `pointerdown` is stale, delayed, or valid from the WebView child's perspective.
- Whether native `WM_NCHITTEST -> HTCAPTION` alone is sufficient through the WebView child surface.
- Whether passing the original renderer pointer coordinates would be more reliable than using live `GetCursorPos()`.

## Next Probe

Probe A temporarily disabled renderer `window.startDrag()` on Windows.

Result:

- Dragging stopped working, so native `WM_NCHITTEST -> HTCAPTION` alone is not
  sufficient through the WebView child surface.
- The synthetic renderer-to-native drag path is required unless the shell moves
  to a deeper WebView2 non-client-region integration.

Next targeted probe:

- Re-enable renderer drag.
- Pass the original renderer `pointerdown` `screenX/screenY` to native.
- Start the system move loop from that original point instead of the delayed
  live `GetCursorPos()` point.
- Reject and log the drag if that original point is not in the native titlebar
  area.

Follow-up result:

- The first renderer-point build still could not drag. No trace file was
  produced from the running instance, but the behavior matches the likely issue
  that `screenX/screenY` is not a reliable native titlebar hit-test coordinate
  across WebView/DPI/window coordinate systems.
- The next build keeps `screenX/screenY` only for the system move-loop start
  coordinate and validates the drag using renderer `clientX/clientY/viewWidth`
  against the WebView titlebar geometry.
- The About repository link now calls `shell.openExternal(url)` so the URL opens
  in the OS default browser instead of navigating inside the WebView shell.

Trace evidence from the next run:

```text
rect=176,380,1391,1084
start-drag-renderer-point lparam=22020864 => screen=768,336
start-drag-renderer-client wparam=972 lparam=2097780 => client=628,32
start-drag-hit-test detail=2
WM_ENTERSIZEMOVE
rect jumps to 829,775,2044,1479
```

Interpretation:

- The renderer client point is valid titlebar input.
- The top-level window physical width is `1215`; the renderer view width is
  `972`, matching 125% DPI scaling.
- The renderer `screenY=336` is above the window top `380`, so
  `PointerEvent.screenX/screenY` is not a reliable physical screen coordinate
  at this WebView activation boundary.

Next probe:

- Keep renderer client coordinates for titlebar validation.
- Derive the `WM_NCLBUTTONDOWN` screen point in native code from
  `GetWindowRect(hwnd_) + MulDiv(clientX/clientY, dpi, 96)` instead of using
  WebView `screenX/screenY`.

Follow-up observation:

- Native-derived screen coordinates removed the WebView CSS/screen coordinate
  mismatch but did not remove the jump.
- The remaining difference is behavioral: holding the left button through the
  activation delay starts a smooth drag after the delay, while clicking and
  releasing before the delayed `startDrag` arrives causes the accumulated mouse
  movement to be applied to the window.
- Therefore the next guard rejects delayed synthetic drags unless
  `GetAsyncKeyState(VK_LBUTTON)` still reports the left button as down. This
  intentionally drops activation-only movement instead of replaying it.

Additional drag-path model:

- If the window is inactive at position `a`, the pointerdown begins at `b`, the
  cursor has already moved to `c` when native finally starts the system move
  loop, and the user releases at `d`, then using `b` as the synthetic
  `WM_NCLBUTTONDOWN` coordinate makes the window replay a parallel copy of
  `c -> d` anchored from `b`.
- The next probe keeps `b` only for titlebar validation and uses the current
  native cursor position `c` as the `WM_NCLBUTTONDOWN` coordinate. This should
  make the delayed drag start from the point where control is actually restored
  instead of replaying movement from the stale pointerdown point.
