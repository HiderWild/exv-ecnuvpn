# WebView Window Size Task 2 Handoff

Generated: 2026-06-18 03:21 +08:00
Branch: codex/ui-framework-webview-shell
Working tree: clean after Task 2 verification

## Work basis before handoff

| Basis | Type | Path / Source | What it says | Confidence |
| --- | --- | --- | --- | --- |
| WebView window size and tray icon plan | repo source | `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md` | Defines six tasks: shared Electron bounds, native window size alignment, persistent Windows tray, persistent macOS status item, close-to-background bridge behavior, and end-to-end verification. | high |
| Latest stop instruction | user instruction | Chat message: "task2结束后，执行收尾和交接。" | Stop after Task 2 completes; do not continue Task 3+. Write a handoff. | high |
| Baseline checkpoint | tool evidence | commit `a717918` | Captured prior WebView white-screen fix, start/build script updates, command validator cleanup, tests, and the new six-task plan. | high |
| Task 1 commit | tool evidence | commit `4fa485f` | Adds shared Electron-compatible window bounds in `src/app/ui_shell/window_layout.hpp` and contract coverage in `tests/ui_shell_contract_test.cpp`. | high |
| Task 2 commit | tool evidence | commit `b4b7605` | Applies shared advanced bounds to Win32, macOS, and Linux native WebView hosts and platform runtime tests. | high |
| Electron-era sizing semantics | tool evidence | `git show c23cb2f^:webui/desktop/main/index.ts` | Old Electron shell used `BrowserWindow({ width, height, resizable:false })` and `mainWindow.setSize(width,height)` without `useContentSize:true`; Task 2 keeps the bounds as outer window dimensions. | medium |

## Current work progress

- Completed:
  - Task 1: shared `ecnuvpn::ui_shell::WindowBounds` contract with `kElectronAdvancedWindowBounds{972, 563}` and `kElectronMinimalWindowBounds{302, 118}`.
  - Task 2: Win32, macOS, and Linux native WebView hosts now consume the shared advanced bounds instead of hardcoded `1180x760`.
  - Task 2: user resizing is disabled/fixed on all three native shells.
  - Task 2: platform runtime tests now check the default bounds helpers with explicit `if (...) return` checks so the new coverage is active under Release/NDEBUG.
  - Task 2 review issue accepted: release-gate `assert` checks were converted to explicit return checks.
  - Task 2 review issue rejected with evidence: Win32 client-area adjustment was not applied because the plan and old Electron code define outer window bounds, not WebView client bounds.
- In progress:
  - None. Work stopped by user instruction after Task 2.
- Not started:
  - Task 3: persistent Windows tray icon.
  - Task 4: persistent macOS menu-bar status item.
  - Task 5: close-to-background behavior and `window.*` bridge wiring.
  - Task 6: full end-to-end verification across Windows/macOS/Linux.
- Blocked:
  - macOS and Linux runtime/build verification are blocked in this Windows workspace.
- Not verified:
  - macOS `darwin_wkwebview_runtime_test` execution.
  - Linux `linux_webkitgtk_runtime_test` execution.
  - Visual/manual confirmation of exact native window dimensions after launch.

## Global progress

- Overall stage: six-task plan is partially complete and intentionally paused after Task 2.
- Completed major areas:
  - Existing WebView packaged renderer white-screen fix was already committed in `a717918`.
  - Shared window bounds contract is committed in `4fa485f`.
  - Native WebView default window size alignment is committed in `b4b7605`.
- Remaining major areas:
  - Always-on Windows tray icon.
  - Always-on macOS status item.
  - Renderer-to-native close prompt and mode actions.
  - Full end-to-end smoke/manual verification, including tray/status visibility.
- Cross-agent or cross-workstream risks:
  - Task 5 should intercept `window.setMode` and `window.resolveClosePrompt` in the platform hosts before generic `host_bridge` forwarding, otherwise those native shell actions can be misrouted to core.
  - macOS close-to-background should use a cancelable close hook such as `windowShouldClose:` rather than relying only on `windowWillClose:`.
  - If a later reviewer requests Win32 `AdjustWindowRect` to make the WebView client area equal `972x563`, first re-check the product requirement. Current implementation intentionally matches old Electron outer window bounds.

## Plan location

- Current plan: `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md`.
- Superseded plans: none identified for this workstream.
- Index / discoverability link: no existing top-level `docs/README.md`, `docs/INDEX.md`, or handoff index was found. This handoff is discoverable at `docs/handoffs/2026-06-18-webview-window-size-task2-handoff.md`.
- Plan gaps:
  - The plan has not been rewritten to mark Task 1 and Task 2 checkboxes complete.
  - The plan still contains future Task 3-6 instructions that were not executed in this stop point.

## Repository state

- Branch: `codex/ui-framework-webview-shell`.
- Dirty files: none at handoff creation time before this document was added; this handoff document should be the only new staged/committed handoff change.
- New files:
  - `docs/handoffs/2026-06-18-webview-window-size-task2-handoff.md`.
- Relevant recent commits:
  - `b4b7605 ui-shell: align native WebView window size with Electron`
  - `4fa485f ui-shell: define Electron-compatible window bounds`
  - `a717918 checkpoint: commit webview shell fixes and tray plan`
- Generated artifacts:
  - `powershell.exe -NoProfile -NoLogo -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop` regenerated the ignored Windows WebView build/package under `build/windows/webview/package/ECNU VPN`.
  - Build output warned that optional `wintun.dll` was not found; this warning existed outside Task 2 scope.

## Worktree and branch closure

- Worktree list checked: yes.
- Active worktree path: `D:/Development/Projects/cpp/ECNU-VPN`.
- Active branch / HEAD: `codex/ui-framework-webview-shell` at `b4b7605` before the handoff document commit.
- Detached HEAD: no.
- Dirty state before closure: clean before writing this handoff document.
- Archive commit: not required because the branch is not detached and Task 1-2 are complete feature commits.
- Archive commit message includes handoff warning: not applicable.
- Cleanup command: none.
- Cleanup result: no worktree cleanup needed.
- Residual worktree / branch risks:
  - Branch remains active for future Task 3+ continuation.
  - No merge back to another branch was performed in this handoff step.

## Verification performed

| Command / Check | Result | Evidence |
| --- | --- | --- |
| `cmake --build build-windows\cpp --config Release --target win32_webview2_runtime_test ui_shell_contract_test` | pass | Exit code 0; Ninja reported targets up to date. |
| `build-windows\cpp\win32_webview2_runtime_test.exe` | pass | Exit code 0. |
| `build-windows\cpp\ui_shell_contract_test.exe` | pass | Exit code 0. |
| `ctest --test-dir build-windows\cpp -C Release --output-on-failure -R "(win32_webview2_runtime_test|ui_shell_contract_test)"` | pass | 2/2 tests passed. |
| `powershell.exe -NoProfile -NoLogo -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop` | pass | WebView renderer built, `exv-ui.exe` relinked, 7/7 release-blocking tests passed, package generated. |
| `git diff --check HEAD^ HEAD` | pass | Exit code 0; no whitespace errors in Task 2 diff. |
| `git diff --check` | pass | Exit code 0 after verification. |
| `rg -n "1180|760|kAdvancedWindowBounds|kMinimalWindowBounds" src\platform tests src\app\ui_shell` | pass | Exit code 1 because no matching legacy literals/symbols remained in the checked scope. |
| macOS runtime/build tests | not run | Current environment is Windows. |
| Linux runtime/build tests | not run | Current environment is Windows. |

## Next agent entrypoint

1. Read:
   - `docs/handoffs/2026-06-18-webview-window-size-task2-handoff.md`
   - `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md`
   - `src/platform/win32/ui_shell/webview2_host_win32.cpp`
   - `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
2. Then inspect:
   - `tests/win32_webview2_runtime_test.cpp`
   - `tests/darwin_wkwebview_runtime_test.cpp`
   - `src/app/ui_shell/host_bridge.cpp`
   - `webui/src/App.vue`
   - `webui/src/stores/config.ts`
3. Resume by:
   - Continuing with Task 3 from the plan: add persistent Windows tray icon and tray menu model.
   - Use subagent-driven development with a fresh implementer and reviewers.
   - Keep each task as its own commit.
4. Avoid touching:
   - Existing Task 1 and Task 2 commits unless a new verified defect is found.
   - Generated build output under `build/`.
   - Unrelated helper/runtime or script changes from the `a717918` baseline.

## Open questions and risks

- Blocking questions:
  - None for the current stop point.
- Assumptions made:
  - Electron-era `width`/`height` are treated as outer native window bounds because the removed Electron code did not use `useContentSize:true`.
  - Linux tray/status integration remains out of scope for this plan.
- Residual risks:
  - macOS Objective-C++ changes from Task 2 are source-reviewed but not compiled on macOS in this Windows workspace.
  - Linux GTK changes from Task 2 are source-reviewed but not compiled on Linux in this Windows workspace.
  - Future Task 5 must be careful not to simply add `window.*` to `host_bridge` allowlist without platform interception, because `handle_host_request()` forwards allowed actions to core.

## Stop boundary

The current agent stopped after writing this handoff. No further implementation work was performed after this point.
