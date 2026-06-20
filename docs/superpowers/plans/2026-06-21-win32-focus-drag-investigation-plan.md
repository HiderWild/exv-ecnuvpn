# Win32 Focus Drag Investigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:systematic-debugging before implementing this plan. Steps use checkbox (`- [ ]`) syntax for tracking. Do not implement a behavioral fix before Task 4 has identified the failing path from logs.

**Goal:** Determine why the Windows WebView2 shell briefly ignores input and jumps when a previously unfocused window is clicked and immediately dragged.

**Architecture:** Add temporary Win32 drag/focus tracing around the existing WebView2 host, reproduce the inactive-window path with a fixed manual matrix, then run one-variable A/B probes. The investigation must separate duplicate drag entry, foreground activation reentry, mouse activation behavior, and coordinate/DPI issues before choosing a fix.

**Tech Stack:** Win32 window messages, WebView2 host bridge, Vue `AppWindowFrame`, C++ source guards, Node host tests, manual Windows UI verification.

---

## Scope Rules

- Do not change drag behavior in Task 1. Only add diagnostics guarded by `EXV_WIN32_DRAG_TRACE`.
- Do not run multiple A/B probes in one build. One variable per probe.
- Preserve unrelated dirty working-tree changes.
- Keep each A/B change temporary until the logs identify the root cause.
- If three A/B probes fail to explain the issue, stop and reassess the architecture before adding another fix.

## Current Evidence

- Renderer drag entry exists in `webui/src/components/AppWindowFrame.vue` through `@pointerdown="startWindowDrag"` and `window.exv?.window?.startDrag?.()`.
- Native synthetic drag entry exists in `src/platform/win32/ui_shell/webview2_host_win32.cpp` through `window.startDrag`, `ReleaseCapture()`, and `SendMessageW(hwnd_, WM_NCLBUTTONDOWN, HTCAPTION, ...)`.
- Native hit-test drag entry also exists through `WM_NCHITTEST` returning `HTCAPTION` for the titlebar.
- `start_window_drag()` promotes the window with `SetWindowPos(hwnd_, HWND_TOP, ...)` and `SetForegroundWindow(hwnd_)` before entering the system move loop.
- `WM_MOUSEACTIVATE` currently returns `MA_ACTIVATE`.

## File Structure

- Modify temporarily: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
  - Adds drag/focus trace logging and later one-variable A/B probes.
- Modify temporarily: `webui/src/components/AppWindowFrame.vue`
  - Used only for the renderer-drag-disabled A/B probe.
- Modify after a confirmed root cause: `tests/win32_webview2_runtime_test.cpp`
  - Updates source guards so the final chosen drag architecture does not regress.
- Modify after a confirmed root cause: `webui/host/__tests__/webview-package-policy.test.ts`
  - Updates host/package source guards if renderer drag behavior changes.
- Create after reproduction: `docs/superpowers/reports/2026-06-21-win32-focus-drag-investigation.md`
  - Records reproduction results, log excerpts, root-cause conclusion, and final fix decision.

## What Requires User Action

- Task 2 requires manual interaction with the packaged Windows UI because the bug depends on real focus, activation, mouse capture, and window move-loop behavior.
- The requested user output is small: for each reproduction attempt, report whether there was a jump, a short input freeze, double focus/IME flash, and whether the app was active before the click.
- A short screen recording is useful but optional. The trace log is mandatory.

## Task 1: Add Temporary Drag and Focus Diagnostics

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`

- [x] **Step 1: Add diagnostic includes**

Add these includes near the existing standard-library includes:

```cpp
#include <fstream>
#include <sstream>
```

- [x] **Step 2: Add trace helpers**

Add these helpers in the anonymous namespace after `utf8_from_wide`:

```cpp
bool win32_drag_trace_enabled() {
  wchar_t value[8]{};
  return GetEnvironmentVariableW(L"EXV_WIN32_DRAG_TRACE", value,
                                 static_cast<DWORD>(sizeof(value) /
                                                    sizeof(value[0]))) > 0;
}

const char *drag_message_name(UINT message) {
  switch (message) {
  case WM_MOUSEACTIVATE:
    return "WM_MOUSEACTIVATE";
  case WM_ACTIVATE:
    return "WM_ACTIVATE";
  case WM_SETFOCUS:
    return "WM_SETFOCUS";
  case WM_KILLFOCUS:
    return "WM_KILLFOCUS";
  case WM_NCHITTEST:
    return "WM_NCHITTEST";
  case WM_NCLBUTTONDOWN:
    return "WM_NCLBUTTONDOWN";
  case WM_LBUTTONDOWN:
    return "WM_LBUTTONDOWN";
  case WM_MOUSEMOVE:
    return "WM_MOUSEMOVE";
  case WM_ENTERSIZEMOVE:
    return "WM_ENTERSIZEMOVE";
  case WM_EXITSIZEMOVE:
    return "WM_EXITSIZEMOVE";
  case WM_CAPTURECHANGED:
    return "WM_CAPTURECHANGED";
  case WM_CANCELMODE:
    return "WM_CANCELMODE";
  default:
    return "other";
  }
}

std::string win32_drag_trace_path() {
  wchar_t temp_path[MAX_PATH]{};
  if (GetTempPathW(MAX_PATH, temp_path) == 0) {
    return "exv-win32-drag-trace.log";
  }
  const std::wstring path = std::wstring(temp_path) + L"exv-win32-drag-trace.log";
  return utf8_from_wide(path.c_str());
}

void append_win32_drag_trace(HWND hwnd, const char *phase, UINT message,
                             WPARAM wparam, LPARAM lparam,
                             LRESULT detail = 0) {
  if (!win32_drag_trace_enabled()) {
    return;
  }
  POINT cursor{};
  GetCursorPos(&cursor);
  RECT rect{};
  GetWindowRect(hwnd, &rect);
  std::ostringstream line;
  line << GetTickCount64() << " phase=" << phase
       << " msg=" << drag_message_name(message)
       << " hwnd=" << reinterpret_cast<std::uintptr_t>(hwnd)
       << " fg=" << (GetForegroundWindow() == hwnd ? 1 : 0)
       << " capture=" << reinterpret_cast<std::uintptr_t>(GetCapture())
       << " cursor=" << cursor.x << "," << cursor.y
       << " rect=" << rect.left << "," << rect.top << ","
       << rect.right << "," << rect.bottom
       << " wparam=" << static_cast<std::uintptr_t>(wparam)
       << " lparam=" << static_cast<std::intptr_t>(lparam)
       << " detail=" << static_cast<std::intptr_t>(detail)
       << "\n";
  std::ofstream out(win32_drag_trace_path(), std::ios::app);
  out << line.str();
}
```

- [x] **Step 3: Instrument `start_window_drag()` without changing behavior**

Inside `start_window_drag()`, add traces at entry, before foreground promotion, after foreground promotion, before `SendMessageW`, and after `SendMessageW`:

```cpp
append_win32_drag_trace(hwnd_, "start-drag-enter", 0, 0, 0);
```

Before the `if (GetForegroundWindow() != hwnd_)` branch:

```cpp
append_win32_drag_trace(hwnd_, "start-drag-before-foreground", 0, 0, 0);
```

Inside the branch, capture return values:

```cpp
const BOOL top_result = SetWindowPos(hwnd_, HWND_TOP, 0, 0, 0, 0,
                                     SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
const BOOL foreground_result = SetForegroundWindow(hwnd_);
append_win32_drag_trace(hwnd_, "start-drag-after-foreground", 0,
                        static_cast<WPARAM>(top_result),
                        static_cast<LPARAM>(foreground_result));
```

Then replace the existing `SetWindowPos(...)` and `SetForegroundWindow(...)` calls in that branch with the captured-return version above.

Immediately before `ReleaseCapture()`:

```cpp
append_win32_drag_trace(hwnd_, "start-drag-before-send", WM_NCLBUTTONDOWN,
                        HTCAPTION, MAKELPARAM(cursor.x, cursor.y));
```

Immediately after `SendMessageW(...)`:

```cpp
append_win32_drag_trace(hwnd_, "start-drag-after-send", WM_NCLBUTTONDOWN,
                        HTCAPTION, MAKELPARAM(cursor.x, cursor.y));
```

- [x] **Step 4: Instrument relevant window messages**

At the top of `window_proc`, after `self` is available and before the `switch`, add:

```cpp
if (message == WM_MOUSEACTIVATE || message == WM_ACTIVATE ||
    message == WM_SETFOCUS || message == WM_KILLFOCUS ||
    message == WM_NCHITTEST || message == WM_NCLBUTTONDOWN ||
    message == WM_ENTERSIZEMOVE || message == WM_EXITSIZEMOVE ||
    message == WM_CAPTURECHANGED || message == WM_CANCELMODE) {
  append_win32_drag_trace(hwnd, "wndproc-enter", message, wparam, lparam);
}
```

In the `WM_NCHITTEST` case, after `const LRESULT hit = self->hit_test_custom_frame(lparam);`, add:

```cpp
append_win32_drag_trace(hwnd, "nchittest-result", message, wparam, lparam, hit);
```

Add explicit trace-only cases before `WM_SIZE`:

```cpp
case WM_ENTERSIZEMOVE:
  append_win32_drag_trace(hwnd, "enter-size-move", message, wparam, lparam);
  break;
case WM_EXITSIZEMOVE:
  append_win32_drag_trace(hwnd, "exit-size-move", message, wparam, lparam);
  break;
case WM_CAPTURECHANGED:
  append_win32_drag_trace(hwnd, "capture-changed", message, wparam, lparam);
  break;
case WM_CANCELMODE:
  append_win32_drag_trace(hwnd, "cancel-mode", message, wparam, lparam);
  break;
```

- [x] **Step 5: Build the diagnostic binary**

Run:

```powershell
pnpm --dir webui webview:compile
cmake --build build-windows\cpp --target exv-ui win32_webview2_runtime_test --config Release
.\build-windows\cpp\win32_webview2_runtime_test.exe
python scripts\package_ui_shell.py
```

Expected:

- WebView renderer compile succeeds.
- `exv-ui` builds.
- `win32_webview2_runtime_test.exe` exits 0.
- The package script completes and prints the packaged WebView shell path.

## Task 2: Run Manual Reproduction Matrix

**Files:**
- Read: `%TEMP%\exv-win32-drag-trace.log`

- [x] **Step 1: Start with a fresh trace log**

Run:

```powershell
Remove-Item "$env:TEMP\exv-win32-drag-trace.log" -ErrorAction SilentlyContinue
$env:EXV_WIN32_DRAG_TRACE = '1'
& "build\windows\webview\package\EXV\exv-ui.exe"
```

If the package path differs, use the path printed by `python scripts\package_ui_shell.py`.

- [x] **Step 2: User performs baseline active-window drag**

User action:

1. Click the EXV window so it is already active.
2. Drag the titlebar area five times.
3. Note whether any jump or short input freeze appears.

Expected:

- No jump.
- No short input freeze.
- Each completed drag should have one `WM_ENTERSIZEMOVE` and one `WM_EXITSIZEMOVE`.

- [x] **Step 3: User performs inactive-window activation-only click**

User action:

1. Click another app so EXV loses focus.
2. Click the EXV titlebar area once without dragging.
3. Repeat five times.

Expected:

- No jump because no drag was attempted.
- Trace should show `WM_MOUSEACTIVATE` and focus/activation messages, but no `WM_ENTERSIZEMOVE`.

- [x] **Step 4: User performs inactive-window immediate drag**

User action:

1. Click another app so EXV loses focus.
2. Press on the EXV titlebar and immediately drag without waiting after mouse-down.
3. Repeat ten times.

For each attempt, record:

```text
attempt=N active_before_click=no jump=yes/no freeze=yes/no ime_or_z_flash=yes/no
```

Expected if the bug is reproduced:

- At least one attempt shows jump or short input freeze.
- Trace should identify whether there are duplicate drag starts, duplicate move loops, or repeated foreground activation.

- [x] **Step 5: User performs tray-restore immediate drag**

User action:

1. Minimize or hide EXV to tray if available.
2. Restore it.
3. Immediately drag the titlebar.
4. Repeat five times.

Expected:

- This checks the path mentioned by commit `5d1a0a3`, where stale message position previously caused jump/refuse-to-start behavior.

- [x] **Step 6: Collect the trace**

Run after closing EXV:

```powershell
Get-Content "$env:TEMP\exv-win32-drag-trace.log" -Tail 300
Copy-Item "$env:TEMP\exv-win32-drag-trace.log" ".\exv-win32-drag-trace.log"
```

Expected:

- The workspace has `exv-win32-drag-trace.log` for analysis.
- User has reported which attempts reproduced the issue.

## Task 3: Interpret the First Trace

**Files:**
- Read: `exv-win32-drag-trace.log`

- [x] **Step 1: Count drag entries per physical attempt**

For each reproduced attempt, count:

```text
start-drag-enter
WM_MOUSEACTIVATE
WM_NCHITTEST detail=HTCAPTION
WM_NCLBUTTONDOWN
WM_ENTERSIZEMOVE
WM_EXITSIZEMOVE
WM_CAPTURECHANGED
```

Expected healthy pattern:

```text
one start-drag-enter
one WM_MOUSEACTIVATE only when initially inactive
one WM_ENTERSIZEMOVE
one WM_EXITSIZEMOVE
no second start-drag-enter before WM_EXITSIZEMOVE
```

- [x] **Step 2: Classify the failure**

Use these rules:

```text
duplicate start-drag-enter or duplicate WM_ENTERSIZEMOVE
=> duplicate drag startup is the primary suspect.

single start-drag-enter, repeated foreground changes, focus changes, or IME/Z flash
=> foreground activation reentry is the primary suspect.

start-drag-enter occurs before WM_MOUSEACTIVATE/WM_ACTIVATE settles
=> activation ordering is the primary suspect.

WM_NCLBUTTONDOWN lParam cursor differs from GetCursorPos cursor
=> coordinate packing or multi-monitor coordinate issue is the primary suspect.

WM_CAPTURECHANGED appears before WM_ENTERSIZEMOVE and no reset follows
=> mouse capture interruption is the primary suspect.
```

- [x] **Step 3: Save a short investigation report**

Create `docs/superpowers/reports/2026-06-21-win32-focus-drag-investigation.md` with:

```markdown
# Win32 Focus Drag Investigation

## Reproduction

- Active-window drag:
- Inactive activation-only click:
- Inactive immediate drag:
- Tray restore immediate drag:

## Trace Summary

- Reproduced attempts:
- start-drag-enter count:
- WM_ENTERSIZEMOVE count:
- Foreground promotion result:
- Capture changes:

## Current Root-Cause Hypothesis

Hypothesis:
Confidence:
Evidence:
Unknowns:
```

## Task 4: Run One-Variable A/B Probes

**Files:**
- Modify temporarily: `webui/src/components/AppWindowFrame.vue`
- Modify temporarily: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Read: `%TEMP%\exv-win32-drag-trace.log`

- [x] **Step 1: Probe A - disable renderer drag on Windows only**

Temporarily change `startWindowDrag` in `webui/src/components/AppWindowFrame.vue`:

```ts
function startWindowDrag(event: PointerEvent) {
  if (isWindows.value) return
  if (event.button !== 0) return
  const target = event.target instanceof Element ? event.target : null
  if (target?.closest('[data-window-control-region="true"], button, a, input, textarea, select')) {
    return
  }
  event.preventDefault()
  void window.exv?.window?.startDrag?.()
}
```

Build and rerun Task 2 Step 4.

Interpretation:

```text
drag still works and bug disappears
=> duplicate renderer/native drag entry is confirmed.

drag stops working
=> top-level WM_NCHITTEST is not sufficient through the WebView child; keep investigating synthetic drag path.

drag works and bug remains
=> duplicate entry is not sufficient; continue to Probe B/C.
```

Revert only this temporary probe before the next probe.

- [ ] **Step 1b: Probe B - use the renderer pointerdown screen point**

Probe A showed that disabling renderer drag on Windows makes the titlebar unable
to drag through the WebView child surface. Re-enable renderer drag, pass
`screenX/screenY` from the pointerdown event to `window.startDrag`, and make the
native drag path validate that point against the host titlebar before sending
`WM_NCLBUTTONDOWN`.

Interpretation:

```text
drag works and the jump disappears
=> delayed/stale live cursor coordinates were the immediate cause.

drag works but the jump remains
=> move-loop startup needs activation serialization in addition to source-point use.

drag often refuses to start and logs start-drag-reject-hit-test
=> renderer pointer coordinates do not match the native titlebar coordinate model.
```

- [ ] **Step 2: Probe B - disable native HTCAPTION titlebar hit-test**

Temporarily change the titlebar branch in `hit_test_custom_frame`:

```cpp
if (content_y >= 0 && content_y < titlebar_height) {
  return HTCLIENT;
}
```

Build and rerun Task 2 Step 4.

Interpretation:

```text
bug disappears
=> native HTCAPTION path participates in duplicate drag startup or activation ordering.

bug remains
=> synthetic JS-to-native startDrag path remains suspect.
```

Revert only this temporary probe before the next probe.

- [ ] **Step 3: Probe C - remove foreground promotion from synthetic drag**

Temporarily change `start_window_drag()` by replacing:

```cpp
if (GetForegroundWindow() != hwnd_) {
  SetWindowPos(hwnd_, HWND_TOP, 0, 0, 0, 0,
               SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
  SetForegroundWindow(hwnd_);
}
```

with:

```cpp
append_win32_drag_trace(hwnd_, "start-drag-skip-foreground", 0, 0, 0);
```

Build and rerun Task 2 Step 4.

Interpretation:

```text
bug disappears and drag still starts
=> foreground promotion is the root cause.

bug disappears but first inactive drag does not start
=> direct foreground promotion is unsafe, but drag must be delayed until activation settles.

bug remains
=> foreground promotion alone is not the root cause.
```

Revert only this temporary probe before the next probe.

- [ ] **Step 4: Probe D - delay synthetic drag until after activation**

Only run this if Probe C shows drag does not start without foreground promotion.

Add a temporary private member:

```cpp
bool pending_drag_after_activate_ = false;
```

Add a temporary message id:

```cpp
constexpr UINT kStartWindowDragAfterActivateMessage = WM_APP + 0x46;
```

Change inactive `start_window_drag()` path:

```cpp
if (GetForegroundWindow() != hwnd_) {
  pending_drag_after_activate_ = true;
  PostMessageW(hwnd_, kStartWindowDragAfterActivateMessage, 0, 0);
  append_win32_drag_trace(hwnd_, "start-drag-post-after-activate", 0, 0, 0);
  return;
}
```

Add a window-proc case:

```cpp
case kStartWindowDragAfterActivateMessage:
  if (self->pending_drag_after_activate_) {
    self->pending_drag_after_activate_ = false;
    self->start_window_drag();
  }
  return 0;
```

Build and rerun Task 2 Step 4.

Interpretation:

```text
bug disappears and first inactive drag starts
=> final fix should serialize activation and drag startup.

bug remains
=> delayed synthetic drag is not sufficient; reassess before further fixes.
```

## Task 5: Choose the Final Fix Based on Evidence

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify if needed: `webui/src/components/AppWindowFrame.vue`
- Modify: `tests/win32_webview2_runtime_test.cpp`
- Modify if needed: `webui/host/__tests__/webview-package-policy.test.ts`

- [ ] **Step 1: If duplicate drag startup is confirmed**

Choose one drag authority:

```text
Prefer native WM_NCHITTEST/HTCAPTION if Probe A proves it works through WebView.
Otherwise keep synthetic window.startDrag and make WM_NCHITTEST return HTCLIENT for the renderer titlebar area.
```

Required test update:

```text
Source guards must reject the removed second drag path so both paths cannot return later.
```

- [ ] **Step 2: If foreground promotion is confirmed**

Remove direct foreground promotion from the drag path.

Required behavior:

```text
User click lets Windows activate the window.
Synthetic drag either starts only when hwnd_ is already foreground, or is posted once after activation.
No SetWindowPos(HWND_TOP) or SetForegroundWindow inside the titlebar pointer-down path.
```

Required test update:

```text
Source guards must reject SetForegroundWindow inside start_window_drag().
Tray restore may keep restore_or_focus_window() behavior if it is not on the pointer-down drag path.
```

- [ ] **Step 3: If activation ordering is confirmed**

Add a single pending-drag state with cleanup on:

```cpp
WM_EXITSIZEMOVE
WM_CAPTURECHANGED
WM_CANCELMODE
WM_LBUTTONUP
```

Required invariant:

```text
One physical mouse-down can create at most one pending drag and one system move loop.
```

- [ ] **Step 4: If coordinate/DPI is confirmed**

Keep `GetCursorPos()` and verify that `WM_NCLBUTTONDOWN` uses screen coordinates.

Required test/manual coverage:

```text
Repeat inactive immediate drag on a secondary monitor with negative X/Y if available.
Repeat at 125% or 150% DPI.
```

## Task 6: Final Verification

**Files:**
- Read: `docs/superpowers/reports/2026-06-21-win32-focus-drag-investigation.md`

- [ ] **Step 1: Build and run source guards**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui webview:compile
cmake --build build-windows\cpp --target exv-ui win32_webview2_runtime_test --config Release
.\build-windows\cpp\win32_webview2_runtime_test.exe
git diff --check
```

Expected:

- Host tests pass.
- WebView compile succeeds.
- Win32 runtime source guard exits 0.
- No whitespace errors.

- [ ] **Step 2: User reruns the reproduction matrix**

User repeats Task 2 Steps 2-5 on the final build.

Expected:

```text
active-window drag: no jump, no freeze
inactive activation-only click: no jump, no freeze
inactive immediate drag: no jump, no freeze
tray restore immediate drag: no jump, no freeze
```

- [ ] **Step 3: Confirm message-loop invariant**

Trace expectation:

```text
For every successful physical drag:
1 start-drag-enter at most
1 WM_ENTERSIZEMOVE
1 WM_EXITSIZEMOVE
0 repeated foreground-promotion events inside drag startup
```

- [ ] **Step 4: Remove temporary diagnostics or leave behind disabled instrumentation**

If the trace helper is no longer useful, remove it before the final fix commit.

If retained, it must remain off by default and guarded by `EXV_WIN32_DRAG_TRACE`.

## Completion Criteria

- The root-cause report identifies exactly one primary cause with trace evidence.
- The final code has one authoritative drag path per physical mouse-down.
- The final code does not perform redundant foreground promotion from the titlebar pointer-down path unless logs prove it is required and serialized.
- User manual reproduction confirms the inactive-window immediate drag no longer jumps or freezes.
- Static tests prevent reintroducing the losing drag path.
