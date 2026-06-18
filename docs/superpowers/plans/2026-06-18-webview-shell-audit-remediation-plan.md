# WebView Shell Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve the June 18 audit findings that belong to the WebView shell and renderer after the shared CLI/core contract fixes have been merged forward.

**Architecture:** `codex/ui-framework-webview-shell` remains the downstream UI branch. First merge and verify `codex/cli-core-ui-contract-refactor`, then implement WebView host parity, stale Electron cleanup, renderer/host behavior, and UI exposure for canonical core actions. Native-only OpenConnect deletion remains on `codex/native-only-cutover`.

**Tech Stack:** Vue 3, TypeScript, Vite, C++20, CMake/Ninja, Win32 WebView2, Cocoa WKWebView, GTK WebKitGTK, generated host contract tests, platform UI shell tests.

---

## Branch Placement

- Worktree: `D:\Development\Projects\cpp\ECNU-VPN`
- Branch: `codex/ui-framework-webview-shell`
- Upstream source branch for shared fixes: `codex/cli-core-ui-contract-refactor`
- Required order: solve and test core/contract fixes on `codex/cli-core-ui-contract-refactor`, merge them into this branch, rerun contract gates, then solve and test UI/WebView fixes here.

## Audit Findings Assigned Here

1. Linux WebKitGTK host still needs parity proof for host bridge calls such as `setMode`, `resolveClosePrompt`, window sizing, and close/tray behavior.
2. `logs.clear` needs UI/host wiring only after the CLI/core branch provides the canonical generated action and backend result shape.
3. Some documentation and comments still overstate Electron retirement or describe obsolete Electron-era paths.
4. WebView acceptance evidence used environment skips in places; release notes must distinguish hard pass from skipped host dependencies.
5. UI shell tests need to protect the renderer from importing desktop-specific APIs and the packaging path from reintroducing Electron/Chromium.

## Phase 0: Merge And Verify Shared Contract Fixes First

**Files:**
- Merge source: `codex/cli-core-ui-contract-refactor`
- Modify generated outputs only if conflict resolution requires regeneration.

- [ ] **Step 1: Merge upstream source branch**

Run from this worktree after the CLI/core remediation branch is committed:

```powershell
git merge codex/cli-core-ui-contract-refactor
```

- [ ] **Step 2: Resolve generated contract conflicts by regenerating**

If generated files conflict, resolve the source manifest first, then run:

```powershell
python scripts\generate_contracts.py
```

- [ ] **Step 3: Verify baseline before UI edits**

Run:

```powershell
cmake --build build-windows\cpp --target contract_manifest_test app_api_rpc_dispatcher_test app_api_status_contract_test ui_shell_contract_test
build-windows\cpp\contract_manifest_test.exe
build-windows\cpp\app_api_rpc_dispatcher_test.exe
build-windows\cpp\app_api_status_contract_test.exe
build-windows\cpp\ui_shell_contract_test.exe
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
```

Expected result: generated action ownership and shared App API/Core RPC behavior are green before UI work starts.

## Phase 1: Prove Linux WebKitGTK Host Parity

**Files:**
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `tests/linux_webkitgtk_runtime_test.cpp`
- Modify: `src/app/ui_shell/host_bridge.cpp`
- Modify: `src/app/ui_shell/host_bridge.hpp`
- Modify: `src/app/ui_shell/close_preference.cpp`
- Modify: `src/app/ui_shell/close_preference.hpp`
- Modify: `tests/ui_shell_contract_test.cpp`

- [ ] **Step 1: Add Linux host bridge parity tests**

Extend `tests/linux_webkitgtk_runtime_test.cpp` to assert Linux registers and dispatches the same host commands as Windows/macOS:

```text
getInitialState
setMode
resolveClosePrompt
openExternal
logs.clear
```

For `setMode`, assert advanced and minimal bounds use the shared layout constants. For `resolveClosePrompt`, assert hide-to-tray is rejected or mapped to a documented Linux fallback unless a tray dependency is intentionally added.

- [ ] **Step 2: Implement missing Linux dispatch paths**

Wire WebKitGTK message handling through the shared `host_bridge` dispatcher instead of platform-specific ad hoc JSON handling. Linux should have the same success/error envelope shape as Windows and macOS.

- [ ] **Step 3: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target ui_shell_contract_test
build-windows\cpp\ui_shell_contract_test.exe
```

Run on Linux or WSL with WebKitGTK development packages:

```bash
cmake --build build-linux/cpp --target linux_webkitgtk_runtime_test
./build-linux/cpp/linux_webkitgtk_runtime_test
```

Expected result: Linux WebKitGTK behavior is tested as a first-class native host, with any no-tray behavior explicitly encoded.

## Phase 2: Wire `logs.clear` Through Host And Renderer

**Files:**
- Modify: `webui/host/shared/host-client.ts`
- Modify: `webui/host/shared/host-contract.ts`
- Modify: `webui/src/stores/*`
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `src/app/ui_shell/host_bridge.cpp`
- Modify: `src/app/ui_shell/host_bridge.hpp`
- Modify: `tests/ui_shell_contract_test.cpp`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`
- Modify: `webui/src/**/__tests__/*` where the clear-logs UI is covered.

- [ ] **Step 1: Depend on generated canonical action**

Consume the `logs.clear` action generated by the CLI/core branch. Do not create a UI-only action string or desktop-only alias.

- [ ] **Step 2: Add renderer and host tests**

Tests must assert:

```text
the clear logs control calls generated logs.clear
success updates UI state without exposing private file paths
failure displays generated error codes or mapped user text
renderer code does not import Node, Electron, WebView2, WKWebView, or WebKitGTK APIs
```

- [ ] **Step 3: Implement UI affordance**

Expose the command in the existing diagnostics/logs area. Use an existing button/control pattern; no landing-page or explanatory UI copy is needed.

- [ ] **Step 4: Verify**

Run:

```powershell
pnpm --dir webui test
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
cmake --build build-windows\cpp --target ui_shell_contract_test feedback_test
build-windows\cpp\ui_shell_contract_test.exe
build-windows\cpp\feedback_test.exe
```

Expected result: clear-logs UI uses the canonical backend contract and keeps private paths out of renderer-facing text.

## Phase 3: Clean Stale Electron Documentation And Policy Text

**Files:**
- Modify: `CMakeLists.txt`
- Modify: `docs/architecture/*`
- Modify: `docs/superpowers/plans/2026-06-16-webview-shell-completion-plan.md`
- Modify: `docs/superpowers/reports/*`
- Modify: `scripts/package_ui_shell.py`
- Modify: `webui/host/__tests__/webview-package-policy.test.ts`

- [ ] **Step 1: Add policy scan coverage**

Extend `webui/host/__tests__/webview-package-policy.test.ts` and any C++ policy tests so active package scripts and production docs cannot mention Electron as a production dependency or runtime.

Allowed contexts:

```text
historical migration report
audit remediation plan
explicitly named legacy section
```

- [ ] **Step 2: Correct overclaims in acceptance docs**

Update reports so they state exact dates, branches, commits, commands, and whether a platform acceptance result was a hard pass or passed with environment skips.

- [ ] **Step 3: Remove obsolete comments**

Delete or rewrite CMake/package comments that describe Electron-era packaging. Comments should name native WebView hosts and sidecar core process layout.

- [ ] **Step 4: Verify**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
cmake --build build-windows\cpp --target ui_shell_cmake_policy_test
build-windows\cpp\ui_shell_cmake_policy_test.exe
```

Expected result: production policy tests fail on stale Electron runtime claims and pass only for historical/audit references.

## Phase 4: Re-run Native WebView Acceptance With Clear Evidence

**Files:**
- Modify: `scripts/accept-webview-shell-windows.ps1`
- Modify: `scripts/accept-webview-shell-macos.sh`
- Modify: `scripts/accept-webview-shell-linux.sh`
- Create or modify: `docs/superpowers/reports/<date>-webview-shell-remediation-acceptance-report.md`

- [ ] **Step 1: Make skip handling explicit**

Acceptance scripts must print one of these final states:

```text
PASS
PASS_WITH_ENVIRONMENT_SKIPS
FAIL
```

Any skipped check must include the missing dependency or unavailable host condition.

- [ ] **Step 2: Run Windows acceptance**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\accept-webview-shell-windows.ps1
```

- [ ] **Step 3: Run macOS acceptance**

Run on the macOS host used by the project:

```powershell
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
```

- [ ] **Step 4: Run Linux acceptance**

Run:

```powershell
bash scripts/accept-webview-shell-linux.sh
```

- [ ] **Step 5: Record report**

The report must include commit SHA, platform, command, final state, skip reasons, and log locations under `build/webview-acceptance/<platform>/`.

## Phase 5: Downstream Merge Gate

- [ ] **Step 1: Run release-blocking tests**

Run:

```powershell
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 2: Run WebUI and host tests**

Run:

```powershell
pnpm --dir webui test
pnpm --dir webui run build
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

- [ ] **Step 3: Merge forward or report blockers**

After this branch is green, merge it only into branches that intentionally consume WebView shell behavior. Do not merge UI-specific changes back into `codex/native-only-cutover` until that branch's native-only policy tests are also green.
