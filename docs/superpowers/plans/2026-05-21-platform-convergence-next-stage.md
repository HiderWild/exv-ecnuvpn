# Windows/macOS Platform Convergence Next Stage Plan

> Status: CLOSED / SUPERSEDED.
> Closed on: 2026-05-21.
> Superseded by: `docs/superpowers/plans/2026-05-21-develop-merge-validation-and-release-hardening.md`.
> Closure summary: G0 baseline lock, G1 audits, G2 tracked-artifact cleanup, and macOS automated validation have been recorded on `integration/platform-convergence-next`. Remaining Windows validation, macOS functional validation, final `develop` merge gating, and release hardening are transferred to the successor plan.
> Historical note: keep this file for context only; do not use it as the active execution checklist.

> Status: execution plan for the next large integration stage.
> Date: 2026-05-21
> Primary goal: turn the current Windows/macOS convergence work into a validated `integration/platform-convergence-next` branch, then merge that branch into `develop` only after both platforms pass the agreed gates.

## 1. Stage Goal And Current Baseline

### Overall Goal

Converge the `windows` and `macos` branches through one temporary integration branch while preserving the platform-specific behavior that is already working on each side.

This stage is not a new feature wave. It is a branch-integration, conflict-audit, and validation wave. Product behavior should change only when a regression is found during integration validation.

### Current Baseline

Current repository facts as of 2026-05-21:

- Active local worktree count is reduced to one Windows worktree: `D:\Development\Projects\cpp\ECNU-VPN`.
- Current working tree is clean on `macos`.
- Long-lived branches are `main`, `develop`, `windows`, and `macos`.
- Temporary integration branch is `integration/platform-convergence-next`.
- `windows` is already an ancestor of `integration/platform-convergence-next`.
- `macos` is not yet an ancestor of `integration/platform-convergence-next`; the integration branch is missing the latest macOS tail commit.
- Direct `windows` + `macos` merge still reports 34 conflicting paths, mostly because both branches independently added platform-adapter files with the same names.
- `develop` + `windows` reports only 3 conflicts, all in desktop UI/state files.
- `develop` + `macos` reports 29 conflicts, covering native, desktop, scripts, and docs.

### Stage Exit Criteria

This stage is complete only when all of the following are true:

- `integration/platform-convergence-next` contains the current heads of both `windows` and `macos`.
- Integration branch conflict resolutions are audited against both source branches, not accepted solely because they compile.
- Windows native build, focused tests, Electron build, desktop package, and manual service/connect/disconnect scenarios pass from the integration branch.
- macOS native build, focused tests, Electron build, desktop package, helper scenarios, and one-time elevated connect/disconnect pass from the integration branch.
- `develop` is not touched until the integration branch passes both platform gates.
- The final merge playbook records the conflict inventory, chosen resolutions, validation commands, manual scenarios, and residual risks.

## 2. Level 1 Workstream Overview

| Workstream | Purpose | Primary Output | Parallelizable |
|---|---|---|---|
| G0. Baseline Lock | Make the integration branch the single source of truth for final convergence | Clean branch inventory and updated integration head | No |
| G1. Resolution Audit | Verify previous conflict resolutions preserve both platforms | Audited integration diff with no unowned conflict decisions | Yes by lane |
| G2. Regression Repair | Fix only validated integration regressions | Small integration-only repair commits | Partly |
| G3. Dual-Platform Validation | Prove the integration branch runs on Windows and macOS | Green validation matrix and manual evidence | Yes until final gate |
| G4. Develop Merge Prep | Prepare the safe merge and GitHub review path | Push/PR/merge checklist and rollback policy | No |

Dependency overview:

```mermaid
flowchart LR
  G0["G0 Baseline Lock"] --> G1["G1 Resolution Audit"]
  G1 --> G2["G2 Regression Repair"]
  G2 --> G3["G3 Dual-Platform Validation"]
  G3 --> G4["G4 Develop Merge Prep"]
```

## 3. Level 2 Detailed Task Plan

### G0. Baseline Lock

Objective: stop new convergence work from spreading across branch heads and make `integration/platform-convergence-next` the only final-merge candidate.

#### G0.1 Confirm Branch And Worktree Surface

Owner: integration lead

Actions:

- Run and record `git status --short --branch`, `git branch --list --verbose --no-abbrev`, and `git worktree list --porcelain`.
- Confirm only `main`, `develop`, `windows`, `macos`, and `integration/platform-convergence-next` are active.
- Record the current direct merge conflict counts:
  - `windows` + `macos`: 34 conflict paths.
  - `develop` + `windows`: 3 conflict paths.
  - `develop` + `macos`: 29 conflict paths.

Scope boundary:

- Do not delete branches in this task unless they are already confirmed obsolete by the branch cleanup decision.
- Do not edit runtime code.

Acceptance criteria:

- `docs/merge-playbooks/windows-macos-merge.md` has a dated baseline entry matching current branch/worktree facts.
- No untracked rehearsal worktree is treated as an implementation source.

#### G0.2 Bring Integration Branch To Both Current Heads

Owner: integration lead

Actions:

- Check out `integration/platform-convergence-next`.
- Merge or cherry-pick the missing `macos` tail commit(s), starting with the current documentation tail.
- If new conflicts appear, resolve them only in the integration branch and record them in the merge playbook.

Scope boundary:

- Do not merge `integration/platform-convergence-next` into `develop` in this task.
- Do not rewrite `windows` or `macos` history unless explicitly required by a later cleanup decision.

Acceptance criteria:

- `git merge-base --is-ancestor windows integration/platform-convergence-next` succeeds.
- `git merge-base --is-ancestor macos integration/platform-convergence-next` succeeds.
- Integration branch working tree is clean.

#### G0.3 Freeze Final Integration Inputs

Owner: integration lead

Actions:

- Announce that new platform-specific fixes should land on their platform branch only if they are urgent.
- Non-urgent shared fixes should land directly on the integration branch as small, named repair commits.
- Update the merge playbook with the rule: no direct merge into `develop` until G3 is green.

Acceptance criteria:

- Every later commit in the stage has one of these labels in the commit message or playbook note: `audit`, `repair`, `validation`, or `merge-prep`.

### G1. Resolution Audit

Objective: audit the current integration branch by subsystem so previous conflict resolutions become intentional decisions rather than opaque merge artifacts.

#### G1.1 Native Platform Adapter Audit

Owner: native platform lane

Primary files:

- `src/platform/common/*`
- `src/platform/win32/*`
- `src/platform/darwin/*`
- `tests/platform_status_models_test.cpp`

Actions:

- Compare integration branch versions against `windows` and `macos` for add/add adapter files.
- Keep shared DTO/model definitions under `src/platform/common`.
- Keep OS APIs and service/process/runtime probing under `src/platform/win32` and `src/platform/darwin`.
- Remove or relocate any common file that is actually Linux-only or platform-specific.

Scope boundary:

- This lane may not change Electron files.
- This lane may not change public desktop contract fields unless G1.3 opens a contract repair task.

Acceptance criteria:

- `src/platform/common` contains interfaces, DTOs, and platform-neutral logic only.
- Windows service status still exposes installed/running/available/binary path semantics.
- macOS service status still exposes launchd/helper availability semantics.
- `platform_status_models_test` passes on both platforms.

#### G1.2 Native RPC, Helper, And VPN Runtime Audit

Owner: native lifecycle lane

Primary files:

- `src/app_api.cpp`
- `src/helper.cpp`
- `src/helper.hpp`
- `src/helper_daemon_win.cpp`
- `src/helper_service_win.cpp`
- `src/vpn.hpp`
- `src/vpn_runtime.cpp`
- `tests/vpn_runtime_test.cpp`

Actions:

- Verify shared native files own routing, state shaping, and orchestration only.
- Verify Windows helper service still uses the dedicated service executable and named-pipe behavior validated on Windows.
- Verify macOS helper and direct elevated session behavior still supports service-installed and helper-missing paths.
- Confirm direct/elevated disconnect uses the correct privilege path for the process owner.

Scope boundary:

- Do not redesign helper IPC.
- Do not change VPN protocol or route policy semantics.
- Do not add new fallback behavior unless a validation failure proves it is required.

Acceptance criteria:

- Windows helper-installed connect/disconnect works through the service.
- macOS helper-installed connect/disconnect works through launchd helper.
- macOS helper-missing one-time elevated connect and disconnect both work.
- `vpn_runtime_test` passes on both platforms.

#### G1.3 Desktop Contract And Renderer Audit

Owner: desktop contract lane

Primary files:

- `webui/desktop/shared/desktop-contract.ts`
- `webui/desktop/main/index.ts`
- `webui/desktop/preload/index.ts`
- `webui/src/api/desktop.ts`
- `webui/src/stores/vpn.ts`
- `webui/src/stores/config.ts`
- `webui/src/types/ecnu-vpn.d.ts`

Actions:

- Treat the Windows UI layout and state model as canonical.
- Keep one shared desktop contract for Windows and macOS.
- Confirm renderer code does not send Vue reactive/proxy payloads over IPC.
- Confirm helper-missing, elevated fallback, service unavailable, and command-cancelled errors use structured errors instead of brittle string matching.

Scope boundary:

- This lane may change TypeScript contract and store logic only if the native RPC lane confirms the same fields exist natively.
- This lane may not fork Vue pages by platform.

Acceptance criteria:

- `npm run build` and `npm run build:electron` pass.
- Windows service page still shows progress and final installed/running/available state correctly.
- macOS one-time connection UX displays actionable results for success, cancellation, and helper unavailable states.
- No duplicate `mode`/`session_mode` contract fields remain unless both are documented compatibility fields.

#### G1.4 Packaging, Runtime, And Build Script Audit

Owner: build/package lane

Primary files:

- `CMakeLists.txt`
- `CMakePresets.json`
- `scripts/build-windows.ps1`
- `scripts/build-macos.sh`
- `scripts/validate-merge-prep-windows.ps1`
- `scripts/validate-merge-prep-macos.sh`
- `webui/package.json`
- `webui/scripts/prepare-native.cjs`

Actions:

- Confirm Windows package staging includes `exv.exe`, `exv-helper.exe`, MinGW runtime DLLs, OpenConnect runtime, and `wintun.dll`.
- Confirm macOS package staging includes the staged/codesigned OpenConnect runtime and dylibs.
- Confirm build scripts do not depend on stale rehearsal paths.
- Confirm build outputs remain platform-separated.

Scope boundary:

- Do not change package identifiers, signing identity, installer behavior, or release naming unless a validation failure requires it.
- Do not collapse Windows and macOS build output directories.

Acceptance criteria:

- Windows package can be produced without manually copying runtime files.
- macOS package can be produced after staging/signing runtime assets.
- Validation wrappers invoke the same build path documented in `docs/build_guide.md`.

### G2. Regression Repair

Objective: fix only issues proven by G1 audit or G3 validation. Keep repairs small and tied to evidence.

#### G2.1 Windows Regression Repair Gate

Owner: Windows repair lane

Triggers:

- UI service uninstall says success but state still shows installed after refresh.
- Service install succeeds but helper availability stays false.
- Connect button returns to idle with helper unavailable despite service running.
- Packaged runtime lacks required helper/runtime files.

Actions:

- Reproduce on `integration/platform-convergence-next`.
- Patch the smallest owner module.
- Record the exact failing command or manual step and the retest evidence.

Acceptance criteria:

- Administrator-launched Windows UI can install service, connect, disconnect, uninstall service, and refresh service state without false failure.
- CLI `service.status` and desktop RPC `service.status` agree on installed/running/available.

#### G2.2 macOS Regression Repair Gate

Owner: macOS repair lane

Triggers:

- Helper-installed connect/disconnect regresses.
- Helper-missing one-time elevated connect succeeds but disconnect fails.
- Runtime staging produces unsigned or killed OpenConnect binary.
- UI service install is blocked by interactive CLI prompts.

Actions:

- Reproduce on the mac mini checkout from the integration branch.
- Patch the smallest owner module.
- Record exact commands and user-visible result.

Acceptance criteria:

- Helper service install/uninstall/connect/disconnect works.
- Helper-missing one-time elevated connect and disconnect both work.
- Packaged macOS runtime passes signing verification.

#### G2.3 Shared Contract Repair Gate

Owner: integration lead plus affected lane

Triggers:

- Windows and macOS require incompatible fields for the same UI state.
- Renderer code must branch by platform to interpret the same RPC response.
- Native code and TypeScript contract disagree on success/error envelope.

Actions:

- Open one explicit contract repair commit.
- Update native DTOs, desktop contract, preload typings, API client, and store usage together.
- Add or update a focused contract test if one exists for the touched area.

Acceptance criteria:

- Renderer sees one shared response shape.
- Platform-specific meaning is carried by values, not by different field names.
- Both platform validation lanes rerun after the contract change.

### G3. Dual-Platform Validation

Objective: prove the integration branch is shippable enough to merge into `develop`.

#### G3.1 Windows Automated Validation

Owner: Windows validation lane

Commands:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"
powershell -ExecutionPolicy Bypass -File ".\scripts\validate-merge-prep-windows.ps1"

cd "D:\Development\Projects\cpp\ECNU-VPN\webui"
npm run build
npm run build:electron
npm run desktop:build
```

Acceptance criteria:

- Native build passes.
- Focused native tests pass.
- Electron TypeScript build passes.
- Desktop package build passes.
- `webui\release\win-unpacked\resources\bin` contains all required native/runtime files.

#### G3.2 Windows Manual Validation

Owner: Windows validation lane

Scenarios:

- Launch packaged or dev Electron UI from an administrator PowerShell.
- Install helper service from UI.
- Confirm service page shows installed/running/available.
- Save auth settings.
- Connect through helper.
- Disconnect through helper.
- Uninstall helper service from UI.
- Confirm service page refreshes to uninstalled/not running/not available.

Acceptance criteria:

- No `Helper daemon is not available` error after service status reports available.
- No false "uninstall incomplete" message after SCM settles.
- No IPC clone error when saving auth/settings/routes.
- UI button state returns to the correct final state after connect/disconnect failure or success.

#### G3.3 macOS Automated Validation

Owner: macOS validation lane

Commands:

```bash
cd /Users/tomli/Development/Projects/cpp/ECNU-VPN
./scripts/validate-merge-prep-macos.sh

cd webui
npm run build
npm run build:electron
npm run desktop:build
```

Acceptance criteria:

- Native build passes.
- Focused native tests pass.
- Electron TypeScript build passes.
- Desktop package build passes.
- Staged OpenConnect runtime is present and signed.

#### G3.4 macOS Manual Validation

Owner: macOS validation lane

Scenarios:

- Install helper service from UI.
- Confirm service status shows installed/running/available.
- Connect through helper.
- Disconnect through helper.
- Uninstall helper service.
- With helper missing, run one-time elevated connect.
- Disconnect that one-time elevated session.
- Cancel the privilege prompt and confirm UI shows a clean cancellation/error state.

Acceptance criteria:

- Helper-installed path works.
- Helper-missing one-time connect works.
- Helper-missing one-time disconnect works.
- OpenConnect is not killed by macOS signing/quarantine checks.
- UI does not require direct terminal commands for normal validation scenarios.

#### G3.5 Final Merge Rehearsal

Owner: integration lead

Actions:

- From a clean checkout/worktree, merge `integration/platform-convergence-next` into `develop` with `--no-commit --no-ff`.
- Confirm conflicts are either zero or already documented integration-only conflicts.
- Abort the rehearsal merge after recording the result.

Acceptance criteria:

- No new conflict appears outside the documented conflict classes.
- The merge playbook records the final `develop` rehearsal command and result.

### G4. Develop Merge And GitHub Prep

Objective: land the validated integration branch into `develop` with a clear rollback and review path.

#### G4.1 Pre-Merge Checklist

Owner: integration lead

Actions:

- Confirm `windows`, `macos`, and `integration/platform-convergence-next` are clean.
- Confirm integration contains both platform heads.
- Confirm G3 Windows and macOS validation evidence is recorded.
- Confirm unresolved risks are either closed or explicitly accepted.

Acceptance criteria:

- No merge to `develop` happens without the checklist being complete.

#### G4.2 Merge Into Develop

Owner: integration lead

Actions:

- Check out `develop`.
- Merge `integration/platform-convergence-next`.
- Run a lightweight post-merge smoke check on the merge result.
- Record the merge commit and validation summary in the merge playbook.

Scope boundary:

- Do not perform new feature repairs on `develop`.
- If merge-result validation fails, revert or reset the merge locally before pushing, then repair on integration.

Acceptance criteria:

- `develop` contains the validated integration branch.
- `develop` is clean after merge.
- No unreviewed local artifact is committed.

#### G4.3 GitHub Push And PR Sequence

Owner: integration lead

Preferred sequence:

1. Push `windows`.
2. Push `macos`.
3. Push `integration/platform-convergence-next`.
4. Merge or PR `integration/platform-convergence-next` into `develop`.
5. Push `develop` only after the final local validation gate.

Acceptance criteria:

- GitHub history has one understandable integration path.
- The PR or merge description links to the merge playbook and this plan.
- Reviewers can reproduce validation without local hidden state.

## 4. Multi-Agent Collaboration Model

### Lane Assignments

| Lane | Responsibility | Primary Write Scope | Blocked Scope |
|---|---|---|---|
| Integration Lead | Branch hygiene, integration branch, merge playbook, final gate | docs, merge commits, integration-only conflict resolutions | Lane-owned implementation changes unless unblocking merge |
| Native Platform Lane | Platform DTOs/status/runtime adapters | `src/platform/**`, status tests | Electron/UI files |
| Native Lifecycle Lane | app API/helper/VPN runtime behavior | `src/app_api.cpp`, `src/helper*`, `src/vpn*`, runtime tests | Build/package scripts except via integration lead |
| Desktop Contract Lane | Electron contract, preload, stores, renderer API | `webui/desktop/**`, `webui/src/api/**`, `webui/src/stores/**`, typings | Native C++ behavior |
| Build Package Lane | CMake, wrapper scripts, runtime staging | CMake/scripts/package files | Runtime behavior unless required by packaging validation |
| Windows Validation Lane | Windows automated/manual validation | validation notes only; repair patches only after trigger | macOS-only behavior |
| macOS Validation Lane | macOS automated/manual validation | validation notes only; repair patches only after trigger | Windows-only behavior |

### Parallelism Rules

- G0 is serial.
- G1.1, G1.2, G1.3, and G1.4 can run in parallel after G0.
- G2 repair lanes can run in parallel only when they touch disjoint files.
- G3.1/G3.2 and G3.3/G3.4 can run in parallel on Windows and macOS machines.
- G3.5 and all G4 tasks are serial final gates.

### Cross-Lane Blocking Rules

- Desktop contract changes block all UI validation until native DTO and TypeScript typings agree.
- Native lifecycle changes block manual connect/disconnect validation on both platforms.
- Build/package changes block desktop package validation on both platforms.
- Any change to `webui/desktop/shared/desktop-contract.ts` requires rerunning both Windows and macOS desktop validation.
- Any change to `src/vpn_runtime.cpp` or process-control adapters requires rerunning connect/disconnect manual scenarios on both platforms.

### Handoff Requirements

Every lane handoff must include:

- Branch name and commit hash.
- Files intentionally changed.
- Files intentionally not touched.
- Exact validation commands run.
- Manual scenarios run, if any.
- Known residual risk.

## 5. Repair Of Previously Completed Work

### Keep

- Windows dedicated helper executable and service-manager path.
- Windows named-pipe helper availability model.
- Windows UI layout and service progress UX as canonical frontend.
- macOS stable helper install path and launchd helper model.
- macOS staged/codesigned OpenConnect runtime.
- macOS one-time elevated connection concept when helper is missing.
- Shared desktop contract and platform adapter direction.
- Single temporary integration branch strategy.

### Repair Or Re-Audit

- `integration/platform-convergence-next` must be updated to include the current `macos` head.
- Add/add platform adapter resolutions must be audited; compiling is not enough.
- Direct/elevated connect and disconnect semantics must be validated on both platforms from the integration branch.
- Windows service uninstall UI state must be rechecked because this has regressed before.
- macOS helper-missing one-time disconnect must be rechecked because this has regressed before.
- Packaging scripts must be validated from clean branch state, not from runtime files copied manually during debugging.

### Do Not Repeat

- Do not create more rehearsal worktrees unless a specific validation step requires one.
- Do not merge directly into `develop` before G3 passes.
- Do not resolve direct `windows` + `macos` conflicts from scratch when the integration branch already contains a resolved candidate.
- Do not move platform files into separate renamed directories just to avoid Git conflicts; use the existing `src/platform/{common,win32,darwin,linux}` ownership model and audit content instead.

## 6. Final Readiness Checklist

Before merging to `develop`, verify:

- [ ] `integration/platform-convergence-next` contains current `windows`.
- [ ] `integration/platform-convergence-next` contains current `macos`.
- [ ] G1 audit notes are recorded for native platform, native lifecycle, desktop contract, and build/package lanes.
- [ ] Windows automated validation passes.
- [ ] Windows manual service/connect/disconnect validation passes.
- [ ] macOS automated validation passes.
- [ ] macOS manual helper and helper-missing validation passes.
- [ ] Final `develop` merge rehearsal is recorded.
- [ ] `docs/merge-playbooks/windows-macos-merge.md` contains final validation evidence and residual risks.
- [ ] `develop` merge is performed only after all previous boxes are checked.
