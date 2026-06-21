# Win32 WebView2 Focus/Input Resolution Archive

Date: 2026-06-21

Branch: `codex/ui-framework-webview-shell`

Primary commits:

- `9836dd5 chore: checkpoint desktop shell work`
- `15da6a7 fix: enable WebView2 host input processing`

Related investigation artifacts:

- `docs/superpowers/plans/2026-06-21-win32-focus-drag-investigation-plan.md`
- `docs/superpowers/reports/2026-06-21-win32-focus-drag-investigation.md`
- `exv-win32-drag-trace.log`

## Executive Summary

The Windows WebView2 shell had three related but distinct issues:

1. When the EXV window was inactive, immediately dragging the custom titlebar could freeze briefly and then jump.
2. After moving titlebar dragging into WebView2 non-client regions, the close/minimize buttons were mostly responsive but could occasionally miss inactive clicks and leave hover state stuck.
3. Even after drag was fixed, the first inactive click anywhere in normal WebView content still had the same strong delay.

The final shape is:

- The blank titlebar drag area is system-managed through WebView2 non-client region support and CSS `app-region: drag`.
- The close/minimize controls are not drag regions; they are `app-region: no-drag` and should be treated as ordinary interactive controls.
- The WebView2 controller is created with `ICoreWebView2ControllerOptions4::put_AllowHostInputProcessing(TRUE)` so input messages pass through the host app window before WebView2 consumes them.

The important conclusion is that the problem was not WebView rendering speed. It was Windows activation/input routing across a top-level host HWND and WebView2 child windows, made worse by a renderer-driven synthetic drag path.

## User-Visible Symptoms

The original symptom looked like:

- EXV inactive.
- User presses or clicks the custom titlebar immediately.
- The window does not respond for a short period.
- After the delay, either dragging starts late or the window jumps.

The user's slow-motion observation was important:

- EXV appeared to try to come forward first.
- Another app surface, including the Codex left sidebar, visibly reacted before EXV fully settled in front.
- EXV then paused briefly and only later became interactive.

That ruled out a simple "Vue is slow to render" explanation. The visible ordering pointed at OS activation, foreground arbitration, WebView2 child HWND focus, and delayed input forwarding.

The user later split the behavior into two precise cases:

- Direct press-and-hold drag: movement before EXV "woke up" did not move the window. Once control returned, dragging became smooth from the then-current cursor position.
- Click/release then move during the delay: movement during the delay could be applied later as a window jump even though the user was no longer holding a drag.

That distinction was the clue that stale or delayed drag startup was being replayed after the activation boundary.

## Initial Architecture

The custom frame had two competing drag concepts:

- Renderer path: Vue `AppWindowFrame` listened for `pointerdown` and called `window.exv.window.startDrag(...)`.
- Native path: Win32 hit testing returned `HTCAPTION` for the custom titlebar area.

The native renderer-driven drag implementation called:

- `ReleaseCapture()`
- `SendMessageW(hwnd_, WM_NCLBUTTONDOWN, HTCAPTION, MAKELPARAM(...))`

That is a standard way to ask Windows to enter its caption move loop, but it is fragile when the call is delayed by WebView2 activation and the starting coordinate no longer describes the actual current interaction.

## Diagnostic Logging

Temporary Win32 tracing was added behind `EXV_WIN32_DRAG_TRACE` and the `%TEMP%\exv-win32-drag-trace.enabled` sentinel. The trace records:

- Message name, including `WM_ACTIVATE`, `WM_SETFOCUS`, `WM_KILLFOCUS`, `WM_NCHITTEST`, `WM_NCLBUTTONDOWN`, `WM_ENTERSIZEMOVE`, `WM_EXITSIZEMOVE`, `WM_CAPTURECHANGED`, `WM_SYSCOMMAND`.
- Current foreground window.
- Current mouse capture window.
- Current cursor screen position.
- EXV top-level window rect.
- `wparam`, `lparam`, and per-stage detail.

The most important early trace pattern was:

```text
WM_ACTIVATE fg=1 cursor=2030,35 rect=225,155,1440,859
WM_SETFOCUS fg=1 cursor=2030,35 rect=225,155,1440,859
WM_KILLFOCUS capture=465224 cursor=2030,35 rect=225,155,1440,859
start-drag-enter fg=1 capture=465224 cursor=2030,35 rect=225,155,1440,859
start-drag-before-send WM_NCLBUTTONDOWN cursor=2030,35 rect=225,155,1440,859
WM_ENTERSIZEMOVE cursor=2030,35 rect=225,155,1440,859
WM_CAPTURECHANGED cursor=2030,35 rect=-681,289,534,993
WM_EXITSIZEMOVE cursor=2030,35 rect=-681,289,534,993
```

The cursor was outside the EXV top-level window rect when the synthetic `WM_NCLBUTTONDOWN/HTCAPTION` was sent. Windows then entered the move loop and produced the visible jump.

## Early Hypotheses And What They Proved

### Foreground Promotion

The native drag path contained foreground-promotion code:

- `SetWindowPos(hwnd_, HWND_TOP, ...)`
- `SetForegroundWindow(hwnd_)`

The first trace did not show the `start-drag-after-foreground` branch. That made direct foreground promotion less likely as the primary trigger for the observed jump.

Conclusion: foreground promotion could still be suspicious architecture, but the captured jump was not caused by that branch in the analyzed run.

### Native HTCAPTION Alone

Probe: disable renderer `window.startDrag()` on Windows and rely on native `WM_NCHITTEST -> HTCAPTION`.

Result: dragging stopped working through the WebView child surface.

Conclusion: ordinary top-level `WM_NCHITTEST` was not sufficient while WebView2 owned the child surface. Some WebView2-supported non-client integration was needed.

### Renderer Screen Coordinates

Probe: pass `PointerEvent.screenX/screenY` from Vue to native and use that as the synthetic caption drag point.

Result: the renderer screen coordinate did not reliably match native physical screen coordinates across WebView2/DPI/activation.

Example from the report:

```text
rect=176,380,1391,1084
renderer screen=768,336
renderer client=628,32
renderer view width=972
window physical width=1215
```

At 125% DPI, `972 * 1.25 = 1215`, so renderer client metrics made sense, but renderer `screenY=336` was above the physical window top `380`.

Conclusion: use renderer client coordinates only for titlebar validation, not as authoritative physical screen coordinates.

### Native-Derived Coordinates

Probe: derive screen coordinates from `GetWindowRect(hwnd_) + MulDiv(clientX/clientY, dpi, 96)`.

Result: this removed the obvious coordinate-space mismatch but did not remove the deeper delayed-start behavior.

Conclusion: the upstream issue was not just coordinate conversion. The drag start itself was arriving after the activation boundary.

### Button-Up Guard

Probe: reject delayed `startDrag` if `GetAsyncKeyState(VK_LBUTTON)` no longer shows the left button as down.

Result: this prevented activation-only movement from being replayed after mouse-up.

Conclusion: stale delayed drags must be dropped. This fixed one failure mode, but it did not make renderer-driven drag the right long-term architecture.

### Current-Cursor Move Loop

Probe: validate titlebar with the original renderer client point, but start the Windows move loop from the current native cursor position after activation has settled.

Result: this made direct press-and-hold drag smooth because the move loop started from the cursor position where control was actually restored, not from the stale pointerdown anchor.

Conclusion: this described the user's "parallel cloned path" observation: starting the move loop from stale point `b` after the cursor has reached `c` causes the later `c -> d` movement to replay from the wrong anchor.

## Why The Drag Fix Changed Direction

The synthetic drag path could be made less wrong, but it was still fundamentally doing application-managed caption behavior through a WebView2 child window during activation.

The user asked whether the visually custom titlebar could be backed by a system-managed transparent/non-client mechanism. That was the right architecture.

WebView2 provides non-client region support:

- `ICoreWebView2Settings9::put_IsNonClientRegionSupportEnabled(TRUE)`
- CSS `app-region: drag`
- CSS `app-region: no-drag`

After enabling that support and marking only the blank titlebar as `app-region: drag`, Windows/WebView2 owned the actual drag operation. The renderer no longer needed to synthesize `WM_NCLBUTTONDOWN` on Windows for that area.

User result: titlebar dragging was "perfectly solved".

## Close/Minimize Button Phase

After drag was solved, the inactive close/minimize buttons still had two issues:

- About one in several inactive close clicks could do nothing.
- Hover state could stick:
  - close button stayed red after mouse left.
  - minimize button stayed hovered after minimizing and restoring from the taskbar.

An attempted native-control approach routed control hit testing through:

- `control_button_hit_test(...)`
- `HTMINBUTTON`
- `HTCLOSE`
- `window-control-state` events back to Vue for hover/pressed visual state.

However the control area was also marked as `app-region: drag`. Logs showed `WM_SYSCOMMAND wparam=61458`, which is `SC_MOVE | HTCAPTION`, meaning WebView2/Windows was interpreting clicks in the control area as caption drag behavior instead of stable button activation.

Conclusion:

- `app-region: drag` is only for blank draggable titlebar surface.
- Controls must be `app-region: no-drag`.
- Putting buttons inside a drag region makes them compete with caption movement and causes missed clicks/stuck hover.

The final CSS split is:

- `.app-window-titlebar`: `app-region: drag`
- `.app-window-titlebar__controls`: `app-region: no-drag`
- `.app-window-titlebar__button`: `app-region: no-drag`

## General First Inactive Click Delay

After drag and button-region semantics were mostly understood, the remaining issue was broader: first inactive clicks anywhere in WebView content still had a strong delay.

That proved the problem was not only "titlebar drag". It was WebView2 child-window activation/input forwarding.

The relevant WebView2 API was not the prerelease Window Controls Overlay path. The local SDK `1.0.4022.49` did not expose the experimental Window Controls Overlay interfaces needed to make that the production fix. It did expose stable controller options:

- `ICoreWebView2Environment10::CreateCoreWebView2ControllerOptions`
- `ICoreWebView2Environment10::CreateCoreWebView2ControllerWithOptions`
- `ICoreWebView2ControllerOptions4::put_AllowHostInputProcessing(TRUE)`

Microsoft describes `AllowHostInputProcessing` as allowing keyboard, mouse, touch, or pen input messages to pass through the browser window to the app process window before WebView2 receives them.

That directly matches the observed failure: the host app needed a chance to process activation/input instead of WebView2 child windows being the first and delayed owner.

Final native controller creation:

1. Query `ICoreWebView2Environment10`.
2. Create `ICoreWebView2ControllerOptions`.
3. Query `ICoreWebView2ControllerOptions4`.
4. Set `AllowHostInputProcessing(TRUE)`.
5. Create the controller with `CreateCoreWebView2ControllerWithOptions`.
6. Fall back to `CreateCoreWebView2Controller` if the runtime lacks the newer interfaces.

## Official References

- WebView2 release notes: https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/
- `ICoreWebView2ControllerOptions4`: https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2controlleroptions4
- `ICoreWebView2Settings9`: https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2settings9

The release notes also mention Window Controls Overlay for custom titlebars, but the production fix used the stable APIs available in the checked-in SDK and runtime.

## Files Changed For The Resolution

Native Windows host:

- `src/platform/win32/ui_shell/webview2_host_win32.cpp`
  - Added guarded Win32 trace logging.
  - Validated renderer drag starts against titlebar geometry.
  - Rejected delayed synthetic drags when the left button was no longer down.
  - Used current native cursor for delayed system move-loop startup during the intermediate synthetic-drag fix.
  - Enabled WebView2 non-client region support with `ICoreWebView2Settings9`.
  - Added native hit testing/state events for titlebar controls during the control phase.
  - Added `AllowHostInputProcessing` controller creation in the final fix.

Renderer frame:

- `webui/src/components/AppWindowFrame.vue`
  - Kept Windows renderer `startDrag` inert once WebView2 non-client drag regions were enabled.
  - Marked blank titlebar as `app-region: drag`.
  - Marked close/minimize controls and buttons as `app-region: no-drag`.
  - Subscribed to `window-control-state` during the native control-state experiment.

Type contract:

- `webui/src/types/exv.d.ts`
  - Added desktop shell bridge types including `startDrag`, `openExternal`, and `window-control-state`.

About-page side fix:

- `webui/src/pages/AboutPage.vue`
  - Repository link now uses `window.exv.shell.openExternal(...)` so it opens in the system browser instead of navigating inside the WebView.

Native external browser bridge:

- `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`

Regression guards:

- `tests/win32_webview2_runtime_test.cpp`
  - Guards the Win32 host architecture, including non-client region support and `AllowHostInputProcessing`.
- `webui/host/__tests__/webview-package-policy.test.ts`
  - Guards the renderer CSS contract: titlebar drag, controls no-drag, and native/system browser bridge.

## Verification Performed

Commands run successfully during final resolution:

```powershell
pnpm --dir webui webview:compile
cmake --build build-windows\cpp --target exv-ui win32_webview2_runtime_test --config Release
.\build-windows\cpp\win32_webview2_runtime_test.exe
git diff --check
```

The focused host policy test:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

Result:

- The titlebar/no-drag and native bridge assertions passed.
- One pre-existing unrelated assertion still failed: `start.ps1` delegates to `scripts/start-common.ps1`, while the test still expected the literal package path string in `start.ps1`.

Package/start verification:

- `start.ps1 -Quick -NoFrontendBuild -WebView2SdkDir build\deps\webview2\1.0.4022.49` built and packaged the WebView shell.
- The first launch attempt without explicit `WebView2SdkDir` exposed a separate script bug: the privileged launch wrapper passed an empty `-WebView2SdkDir` argument.
- With explicit SDK path, the packaged `exv-ui.exe` launched successfully.

Runtime observed:

- WebView2 runtime process used `149.0.4022.80`.
- `exv-ui.exe` launched from `build\windows\webview\package\EXV`.

Manual acceptance:

- User confirmed titlebar drag was perfectly solved after moving the blank titlebar area to WebView2 non-client drag regions.
- After the final input-processing fix, user responded positively, indicating the issue was resolved enough to archive.

## Final Mental Model

This bug was a stack of input ownership mistakes:

1. A frameless WebView2 shell visually had a custom titlebar.
2. The renderer tried to implement titlebar drag by sending a native command.
3. When the top-level window was inactive, Windows had to activate the host, WebView2 had to reconcile child-window focus/capture, and the renderer-to-native drag command could arrive late.
4. A delayed synthetic `WM_NCLBUTTONDOWN/HTCAPTION` can replay stale pointer intent after the user's physical mouse state has changed.
5. Moving blank-titlebar drag to WebView2 non-client regions gives the system/WebView2 ownership of the operation.
6. Marking real controls as drag regions breaks controls because Windows treats the area like caption surface.
7. General inactive-click delay is solved at controller creation by enabling host input processing, not by turning every control into `app-region`.

## Rules For Future Changes

- Do not reintroduce JavaScript-driven drag on Windows unless WebView2 non-client regions are removed and there is a new explicit design.
- Keep only blank titlebar surface as `app-region: drag`.
- Keep buttons, links, inputs, menus, and normal content as `app-region: no-drag` or no app-region at all.
- Do not use `PointerEvent.screenX/screenY` as authoritative native physical screen coordinates inside WebView2/DPI activation paths.
- If synthetic caption messages are ever needed again, reject them when the left button is no longer down and validate coordinates in native code.
- Prefer official WebView2 controller/settings APIs over ad hoc subclassing of WebView2 child HWNDs.
- Keep source guards for `AllowHostInputProcessing` and non-client region support; these are regression-critical.

## Follow-Up Items

- Fix the separate `start.ps1` privileged-launch empty `WebView2SdkDir` argument path.
- Update `webview-package-policy.test.ts` or `start.ps1` so the delegated start-script architecture has a passing policy assertion.
- Decide later whether to keep the guarded Win32 trace helper permanently. It is off by default and useful for future activation/focus regressions.

