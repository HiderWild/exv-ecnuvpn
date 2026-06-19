# Transparent Shell Mode Transition Design

Date: 2026-06-19
Status: Design approved for planning

## Context

The Windows desktop shell currently resizes the native WebView window during
mode switches. Even with a short timer interval, this still feels uneven
because WebView repaint, native non-client chrome, and `SetWindowPos` all sit in
the visible animation loop.

The accepted direction is to stop treating the physical window resize as the
animation. The renderer should animate a logical window surface inside a
transparent host shell, while the native host performs only one physical resize
per transition.

## Goals

- Use the same outer application frame for advanced and minimal modes.
- Keep existing advanced and minimal page content functionally unchanged.
- Add a narrow frame layer that owns titlebar controls, drag area, transition
masking, and mode-transition choreography.
- Avoid per-frame native window resize during mode switching.
- Preserve the existing close-confirmation business flow.
- Plan both Windows and macOS behavior, with platform-specific titlebar
integration.

## Non-Goals

- Redesign dashboard, settings, logs, or minimal-mode content.
- Change VPN connection behavior.
- Implement a fully custom macOS traffic-light replacement.
- Introduce Linux transparent-shell behavior in this phase.
- Add arbitrary user-resizable window layouts beyond the existing advanced and
minimal sizes.

## Accepted Approach

Use "Custom Titlebar + Preview Then Resize".

The renderer wraps the current page tree in an `AppWindowFrame`. During a mode
transition, `AppWindowFrame` animates the visible content surface toward the
target size. The host shell area outside that content surface remains
transparent, so the user perceives the window as growing or shrinking smoothly.

The native host performs one physical resize at a direction-specific moment:

- Minimal to advanced: resize the physical window to advanced size first, then
  animate the renderer content surface from minimal size to advanced size.
- Advanced to minimal: animate the renderer content surface from advanced size
  to minimal size first, then resize the physical window to minimal size.

The logical mask starts immediately when the user requests the mode switch. It
blocks application interaction and shows the app icon, but it must not paint the
transparent shell area as an opaque full-window panel. The mask ends 50 ms after
the native resize has completed.

## Renderer Architecture

### AppWindowFrame

`AppWindowFrame` is a new renderer-level container around existing content.

Responsibilities:

- Render the app frame and titlebar area.
- Render platform-appropriate titlebar controls.
- Provide the drag-safe titlebar layout and button exclusion zones.
- Host the current advanced or minimal content without changing page internals.
- Own mode-transition state and request sequencing.
- Render the logical transition mask and app icon.
- Apply CSS transforms and size constraints for the preview animation.

The existing advanced and minimal views remain responsible for their own page
content. They should not know how the native window is resized.

### Transition State

The renderer owns a monotonically increasing mode-transition request id. A
transition can be in one of these phases:

- `idle`
- `native-resize-before-animation`
- `preview-animating`
- `native-resize-after-animation`
- `settling`

Only the latest request id may change the physical window, update the applied
mode, or release the mask. Older async completions are ignored.

### Renderer Timing

The preview animation duration is 300 ms. The easing remains front-loaded and
non-linear: it moves near the target quickly, then converges slowly. The
post-resize settle delay is 50 ms.

The mask starts before any native resize call. The mask is removed only after:

1. the latest request is still current,
2. the required native resize has completed,
3. the preview animation has completed,
4. the 50 ms settle delay has elapsed.

## Host Bridge API

The renderer needs a clearer window bridge surface:

- `window.resizeForMode(mode, request)` performs exactly one physical resize to
  the target mode bounds and returns when the host has applied it.
- `window.minimize()` minimizes the native window.
- `window.requestClose()` asks the host to start the existing close flow.
- `window.resolveClosePrompt(result)` remains the renderer response to the
  existing close prompt.

The current `window.setMode(mode, request)` can be kept as a compatibility alias
or migrated internally to `resizeForMode`. The key contract is that the host
must no longer animate native bounds frame by frame for mode switches.

## Windows Behavior

Windows uses a fully custom titlebar in the renderer.

Host expectations:

- Remove the visible native titlebar from the client area.
- Preserve normal taskbar, Alt-Tab, window shadow, minimize, and close semantics.
- Handle drag and hit-test behavior in Win32 rather than scattering it across
  pages.
- Treat titlebar drag zones as `HTCAPTION`.
- Exclude the renderer button zone from drag hit-testing.
- Expose minimize and close-request bridge actions.
- Support transparent host/background rendering for the shell area outside the
  logical content surface.

Implementation should prefer a compositor-friendly borderless overlapped window
over a bare popup window, so system behavior remains predictable. If transparent
WebView or host transparency is unreliable on a target system, the fallback is a
theme-colored transition shell while retaining the one-shot physical resize
timing.

Windows titlebar controls:

- Renderer draws minimize and close buttons.
- Minimize calls `window.minimize()`.
- Close calls `window.requestClose()`, which reuses the existing
  close-confirmation flow.

## macOS Behavior

macOS uses the same renderer frame and transition state, but it does not replace
system traffic-light controls.

Host expectations:

- Keep native close/minimize semantics.
- Make the titlebar visually integrated with the content area, using Cocoa
  transparent-titlebar/full-size-content behavior.
- Hide native title text where appropriate.
- Keep the traffic-light buttons embedded in the window.
- Let the renderer frame reserve safe space around traffic-light controls.
- Use one physical `setFrame` resize at the same direction-specific point as
  Windows.

macOS close behavior remains:

`windowShouldClose` intercepts close, emits `close-request`, the renderer shows
the existing close prompt, and `window.resolveClosePrompt` completes the action.

If transparent titlebar or `WKWebView` transparency is unreliable, macOS may
fall back to a theme-colored shell background while retaining the shared
transition timing.

## Direction-Specific Sequences

### Minimal to Advanced

1. User toggles to advanced.
2. Renderer increments the transition request id.
3. Renderer shows the logical mask immediately.
4. Renderer asks host to `resizeForMode("advanced", request)`.
5. Host performs one physical resize to advanced bounds.
6. Renderer animates the content surface from minimal bounds to advanced bounds.
7. Renderer waits 50 ms.
8. Renderer marks advanced as applied and removes the mask.

### Advanced to Minimal

1. User toggles to minimal.
2. Renderer increments the transition request id.
3. Renderer shows the logical mask immediately.
4. Renderer animates the content surface from advanced bounds to minimal bounds
   inside the still-advanced physical host window.
5. Renderer asks host to `resizeForMode("minimal", request)`.
6. Host performs one physical resize to minimal bounds.
7. Renderer waits 50 ms.
8. Renderer marks minimal as applied and removes the mask.

## Error Handling

- If a host resize call fails, the renderer keeps the mask only long enough to
  restore a known applied mode, then releases interaction and records an error.
- If a newer request supersedes an older one, older host responses and animation
  completions are ignored.
- If a native resize does not complete within a bounded timeout, the renderer
  stops waiting and falls back to the last applied physical mode.
- Close and minimize requests remain available only through titlebar controls;
  they should be blocked while a mode transition mask is active unless the host
  receives a native close event.

## Testing Strategy

### Renderer Tests

- `AppWindowFrame` wraps both advanced and minimal content.
- The mode transition state machine differentiates minimal-to-advanced and
  advanced-to-minimal order.
- The mask starts immediately and releases only after animation, native resize,
  and the 50 ms settle delay.
- Rapid toggles only allow the latest request to commit.
- macOS layout reserves traffic-light safe space while Windows renders custom
  controls.

### Host Contract Tests

- Windows host no longer performs timer-driven native resize for mode switches.
- Windows host exposes `window.resizeForMode`, `window.minimize`, and
  `window.requestClose`.
- Windows host includes non-client and hit-test handling for custom titlebar
  behavior.
- macOS host exposes the same resize bridge and integrates the titlebar into the
  content area.
- macOS close still routes through the existing renderer close prompt.

### Visual Acceptance

- On Windows, advanced/minimal toggles do not show native-titlebar jank.
- On Windows, transparent shell space is not painted as a full opaque panel.
- On macOS, traffic-light controls remain native and correctly positioned.
- On both platforms, the app icon mask appears immediately and remains through
  the 50 ms post-resize settle period.
- On rapid repeated toggles, the final mode matches the last user request and
  the UI does not self-toggle afterward.

## Rollout

Implement behind the existing native WebView shell path. The first acceptance
target is Windows, because it is the platform where resize jank is most visible.
macOS must be included in the design and API contract before the implementation
is considered complete, but its visual polish can be verified on macOS after
the shared renderer frame and bridge contract are in place.
