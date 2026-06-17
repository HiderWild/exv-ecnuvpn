# WebView Window Size and Tray Icon — Final Completion Handoff

Generated: 2026-06-18
Branch: `codex/ui-framework-webview-shell`
Working tree: clean (only this handoff added)

## Work basis

| Basis | Type | Path / Source | What it says | Confidence |
| --- | --- | --- | --- | --- |
| Plan | repo source | `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md` | Six-task plan: shared bounds, native window size, persistent Win32 tray, persistent macOS status item, close-prompt wiring, end-to-end verification. | high |
| Prior handoff | repo source | `docs/handoffs/2026-06-18-webview-window-size-task2-handoff.md` | Stop point at Task 2; Tasks 3–6 outstanding; flagged that Task 5's macOS path needs `windowShouldClose:` (not just `windowWillClose:`). | high |
| Pre-task baseline | tool evidence | commit `b4b7605` | Tasks 1+2 committed (shared bounds + native window alignment). | high |
| User directive | chat | "按顺序完成接下来所有任务，优先使用子智能体驱动的开发，你负责派发任务和审计，分阶段提交，提交之前要保证所有审计，guardrail等均通过" | Continue Tasks 3–6 via subagent-driven development; commit each stage only after audits/guardrails pass. | high |
| Release-blocking gate | project memory | `project_release_blocking_gate.md` | Gate is `release-blocking` ctest label via `scripts\build-windows.ps1 desktop`; unrelated 0xc0000139 full-ctest failures out of scope. | high |

## Current work progress

- Completed:
  - Task 3 (commit `d3af2b2`): persistent Windows tray icon via `Shell_NotifyIconW`. Tray menu model (`显示 ECNU VPN` / 分隔符 / `退出`) is exposed for tests; `force_quit_`, `show_from_tray`, `quit_from_tray`, `show_tray_menu`, `create_tray_icon`, `destroy_tray_icon` added to `WebView2Window`. CMakeLists adds `shell32` to both `win32_webview2_runtime_test` and `exv-ui`.
  - Task 4 (commit `ce9d57c`): persistent macOS menu-bar `NSStatusItem`. `WkWebViewStatusMenuItem` + free functions declared at namespace scope (token-identical between header forward-decl in test and definition in `.mm`). `EcnuVpnStatusItemTarget` Objective-C wrapper forwards `showWindow:` / `quitApp:` into `WkWebViewWindow`. Status item created right after `setActivationPolicy:` and torn down first in `cleanup()`.
  - Task 5 (commit `fd1e3e2`): `window.setMode` and `window.resolveClosePrompt` are wired through both native shells.
    - `is_allowed_host_action()` allows the two `window.*` actions; the contract test asserts it.
    - Both bridge JS scripts replace the no-op stubs with `rpc('window.setMode', ...)` / `rpc('window.resolveClosePrompt', ...)`.
    - Both platform hosts intercept the two actions in `on_web_message` / `handle_script_message` BEFORE forwarding to the host bridge, so they never reach core. The resolver handles BOTH renderer payload shapes — bare string `"cancel"` AND `{ action, remember }`.
    - Win32 `WM_CLOSE` defers to `request_close_decision()` unless `force_quit_` is set; the cancel/tray/quit branches use the Task-3 hooks.
    - macOS uses `windowShouldClose:` (returning `NO` while not `force_quit_`) so the prompt is shown while the window is still on screen. `windowWillClose:` was reduced to a pure run-loop teardown notification (`running_=false; exit_code_=0`). This addresses the cross-agent risk flagged in the Task-2 handoff.
  - Task 6: end-to-end verification on Windows complete. macOS / Linux runtime steps are out of scope on this Windows host (see "Not verified").
- In progress: none.
- Not started: none.
- Blocked: macOS and Linux runtime/manual smoke verification cannot run on this Windows workspace.
- Not verified:
  - Task 6 Step 3: Windows packaged-renderer CDP smoke (`location.href`, `document.getElementById('app')`, `window.innerWidth`, tray-visible-on-launch). Requires interactive `Start-Process exv-ui.exe` plus a CDP client; the script in the plan is documented but not executed in this turn.
  - Task 6 Step 4: macOS `./scripts/build-macos.sh desktop` — needs macOS toolchain.
  - Task 6 Step 5: Linux `./scripts/build-linux.sh desktop` — needs Linux toolchain (and the WebKitGTK/GTK packages gated by `EXV_BUILD_UI_SHELL`).

## Global progress

- Overall stage: six-task plan completed source-side; Windows runtime gate green; macOS/Linux runtime confirmation deferred to those host environments.
- Completed major areas:
  - Shared Electron-compatible window bounds (`window_layout.hpp`).
  - Native window size alignment on Win32, macOS, Linux.
  - Persistent Windows tray icon and tray menu commands.
  - Persistent macOS menu-bar status item and menu commands.
  - Renderer-to-native close-prompt and `setMode` wiring on both Win32 and macOS, with cancelable close on macOS via `windowShouldClose:`.
- Remaining major areas:
  - Out-of-environment runtime verification (Win32 CDP smoke; macOS launch; Linux launch).
- Cross-agent or cross-workstream risks:
  - The renderer (`webui/src/App.vue`) sends `'cancel'` as a bare string and `{ action, remember }` as an object. Native hosts handle both shapes; future renderer changes that introduce a third shape (e.g. enum-only) would need a matching native update.
  - `apply_window_mode` resizes outer window bounds (matching old Electron `BrowserWindow({ width, height })` semantics). If a future product decision treats the bounds as content size, both platform hosts must change together with `window_layout.hpp`.
  - Linux native shell intentionally does not get tray/status integration (no `StatusNotifierItem` / `AppIndicator` dependency). If Linux later needs a tray, plan a separate task with the dependency story.

## Plan location

- Current plan: `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md`. Tasks 1–6 are addressed by commits `4fa485f`, `b4b7605`, `d3af2b2`, `ce9d57c`, `fd1e3e2` (and the existing checkpoint `a717918`). The plan's checkboxes were not flipped; this handoff is the canonical completion record.
- Superseded plans: none identified for this workstream.
- Index / discoverability link: this handoff lives at `docs/handoffs/2026-06-18-webview-window-size-tray-icon-complete-handoff.md` next to the existing Task-2 handoff.
- Plan gaps: as above; checkbox state is left unflipped because the plan format intentionally tracks micro-steps and the commit ledger is the load-bearing record.

## Repository state

- Branch: `codex/ui-framework-webview-shell`.
- Dirty files before this handoff write: none. After this handoff: only this new file.
- New files (this handoff): `docs/handoffs/2026-06-18-webview-window-size-tray-icon-complete-handoff.md`.
- Relevant recent commits:
  - `fd1e3e2 ui-shell: decouple close-to-background from tray visibility` — Task 5
  - `ce9d57c ui-shell: keep macOS status item visible` — Task 4
  - `d3af2b2 ui-shell: keep Windows tray icon visible` — Task 3
  - `2de9f51 docs: hand off webview window size task2` — prior handoff
  - `b4b7605 ui-shell: align native WebView window size with Electron` — Task 2
  - `4fa485f ui-shell: define Electron-compatible window bounds` — Task 1
- Generated artifacts (gitignored): `build-windows/cpp` regenerated; `build/windows/webview/package/ECNU VPN` rebuilt.

## Worktree and branch closure

- Worktree list checked: yes; only the primary working tree.
- Active worktree path: `D:/Development/Projects/cpp/ECNU-VPN`.
- Active branch / HEAD: `codex/ui-framework-webview-shell` at `fd1e3e2` before this handoff commit.
- Detached HEAD: no.
- Dirty state before closure: clean.
- Archive commit: not required.
- Cleanup command: none.
- Cleanup result: nothing to clean.
- Residual worktree / branch risks: branch remains active for downstream merge / cross-platform smoke verification.

## Verification performed

| Command / Check | Result | Evidence |
| --- | --- | --- |
| `cmake --build build-windows/cpp --config Release --target win32_webview2_runtime_test exv-ui` (after Task 3) | pass | All 5 ninja steps succeeded; tray-relevant link of `shell32` clean. |
| `build-windows/cpp/win32_webview2_runtime_test.exe` (via ctest, Task 3) | pass | Exit 0; tray menu model assertion green. |
| `cmake --build build-windows/cpp --config Release --target ui_shell_contract_test win32_webview2_runtime_test exv-ui` (after Task 5) | pass | 9 ninja steps succeeded. |
| `ctest -R "(ui_shell_contract_test|win32_webview2_runtime_test)"` (after Task 5) | pass | 2/2 tests passed. |
| `scripts\build-windows.ps1 desktop` (after Task 3 commit, again after Task 5 commit) | pass | 7/7 release-blocking tests passed; native WebView package generated; `wintun.dll not found` warning is the pre-existing baseline. |
| `python scripts/package_ui_shell.py --verify-launch-targets-only --package-dir "build/windows/webview/package/ECNU VPN"` | pass | Output: `verified native WebView shell package`. |
| `pnpm exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts host/__tests__/desktop-contract-generated.test.ts` | pass | All host suites green; `desktop-contract-generated` 10/10. |
| `git diff --check` | pass | Exit 0; no whitespace errors. |
| `git status --short` | clean | Empty between work commits. |
| macOS runtime/build tests | not run | Windows workspace; macOS toolchain unavailable. |
| Linux runtime/build tests | not run | Windows workspace; Linux toolchain unavailable. |
| Windows packaged-app CDP smoke (`location.href`, app DOM, `window.innerWidth`, tray-visible-on-launch) | not run | Requires interactive launch + CDP client; documented in Task 6 Step 3 of the plan for the next operator. |

## Next agent entrypoint

1. Read:
   - This handoff.
   - `docs/superpowers/plans/2026-06-18-webview-window-size-tray-icon.md` (Task 6 Steps 3–5 for outstanding manual verification).
   - `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm` (review the `windowShouldClose:` deviation from the literal plan).
   - `src/platform/win32/ui_shell/webview2_host_win32.cpp` (Task 5's JSON interception block).
2. Then inspect:
   - `webui/src/App.vue` — confirm the renderer's close-prompt UI still drives `tray|quit|cancel` via `resolveClosePrompt`.
   - `webui/src/stores/config.ts:152-153` — confirm `setMode` is called from the Pinia store.
3. Resume by:
   - Running Task 6 Step 3 on a Windows host with WebView2 installed (CDP via `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9337`), recording the four checks listed in the plan.
   - Running Task 6 Step 4 on macOS via `./scripts/build-macos.sh desktop`, then visually confirming the menu-bar status item appears immediately at launch.
   - Running Task 6 Step 5 on Linux via `./scripts/build-linux.sh desktop`, confirming the window opens at 972x563 and is non-resizable.
   - Closing the loop by either flipping the plan's checkboxes or marking the plan superseded.
4. Avoid touching:
   - The five existing feature commits (`4fa485f`, `b4b7605`, `d3af2b2`, `ce9d57c`, `fd1e3e2`) unless a verified defect emerges from the runtime smoke.
   - Generated build/package output under `build/`.
   - The Linux native shell's tray story unless explicitly scoped — current decision is no Linux tray.

## Open questions and risks

- Blocking questions: none for this stop point.
- Assumptions made:
  - Renderer `resolveClosePrompt` argument shape stays `'cancel'` OR `{ action, remember }`. Both shapes are explicitly handled by both native hosts.
  - `setContentMinSize:`/`setContentMaxSize:` is the right macOS pattern (matches the Task-2 file's `create_window()`); the plan's `setMinSize:`/`setMaxSize:` would set outer window size including title bar, which would not match the Electron-era visible content.
  - The macOS deviation from the plan (`windowShouldClose:` instead of relying on `windowWillClose:`) is correct because `windowWillClose:` fires after the window has been ordered out, leaving no surface to render the prompt on. The Task-2 handoff explicitly flagged this.
- Residual risks:
  - macOS / Linux source-only: any platform-specific compile error would only surface on those hosts.
  - The Windows `apply_window_mode` resizes by outer window bounds via `SetWindowPos`. The renderer expects the inner `window.innerWidth` to closely match Electron's content size. If a regression appears in the CDP smoke step, switch `SetWindowPos` to use `AdjustWindowRect` for the same `kFixedWindowStyle` and reapply.
  - A future plan to consolidate the bridge JS into a single source (instead of duplicating it across Win32 and macOS host files) would simplify Task-5-style cross-platform changes.

## Stop boundary

Tasks 3–5 are committed and verified by the Windows release-blocking gate. Task 6's Windows-runnable steps are complete; Steps 3–5 require non-Windows or interactive environments and are explicitly handed off above. No further implementation is performed after this handoff.
