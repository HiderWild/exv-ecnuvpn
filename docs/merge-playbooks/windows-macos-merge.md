# Windows/macOS Merge Playbook

This playbook records the working agreement, merge rehearsal steps, manual conflict resolutions, validation evidence, and residual risks for the current merge-prep wave.

## Develop Merge Gate Handoff (2026-05-22)

Active successor plan:

- `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md`

Current branch heads:

- `develop` = `7d39136` (Merge Windows desktop convergence)
- `windows` = `66dbfa8` (refactor: unify desktop contract and platform status adapters)
- `macos` = `6fb5ebb` (docs: record S4 integration rehearsal results)
- `integration/platform-convergence-next` = `96d781c` (docs: plan develop merge validation gate)

Current branch relationship:

- `git merge-base --is-ancestor windows integration/platform-convergence-next`: PASS
- `git merge-base --is-ancestor macos integration/platform-convergence-next`: PASS
- `git merge-tree --write-tree --messages --name-only develop integration/platform-convergence-next`: PASS, no conflict paths reported
- `git diff --name-only develop..integration/platform-convergence-next`: 136 changed paths

Completed in the previous stage:

- G0 baseline lock: complete.
- G1.1 native platform adapter audit: complete.
- G1.2 native lifecycle audit: complete.
- G1.3 desktop contract audit: complete.
- G1.4 build/package audit: complete.
- G2 tracked sync-conflict cleanup: complete by tracked-file check; `git ls-files | rg "sync-conflict|rehearsal"` reports no tracked artifacts.
- G3 macOS automated validation: complete; native build, 5 focused tests, webui build, and Electron build passed.
- G4 develop merge rehearsal: ready; merge-tree reports no conflict paths.

Not accepted as completion evidence:

- Remote OMC team `ecnu-vpn-develop-merge-validat` is still in planning/pending state; all three tasks are pending and no validation artifact has been produced.
- The remote macmini worktree at `/Users/tomli/Development/Projects/CPP/ECNU-VPN` is dirty; those edits cannot be counted as validation until they are reviewed, committed or discarded, and tied to exact commands.
- The 2026-05-21 G3 macOS recommendation to proceed with G4 based only on automated macOS validation is superseded by the 2026-05-22 plan.

Pending before `develop` merge:

- Windows automated validation from `integration/platform-convergence-next`.
- Windows manual helper service install/connect/disconnect/uninstall validation.
- macOS manual helper-installed connect/disconnect validation with real credentials.
- macOS manual helper-missing one-time elevated connect/disconnect validation.
- Any targeted repair required by those validation gates.

Gate rule:

- Do not merge, push, or repair directly on `develop` until the pending validations pass or an explicitly accepted risk is recorded in this playbook.
- All develop-blocker repairs land on `integration/platform-convergence-next` first, followed by focused retest evidence.

## G0 Baseline Lock (2026-05-21)

Historical snapshot: this section records the earlier G0 state. The current active handoff is the "Develop Merge Gate Handoff" section above.

Branch heads:
- `macos` = 6fb5ebb (docs: record S4 integration rehearsal results)
- `windows` = 66dbfa8 (refactor: unify desktop contract and platform status adapters)
- `integration/platform-convergence-next` = 58074c5 (merge-prep: integrate macos tail commits)
- `develop` = 7d39136 (Merge Windows desktop convergence)

Worktree surface: single worktree at /Users/tomli/Development/Projects/CPP/ECNU-VPN (macos branch).

Ancestor verification:
- `git merge-base --is-ancestor windows integration/platform-convergence-next`: PASS
- `git merge-base --is-ancestor macos integration/platform-convergence-next`: PASS (after merging macos tail commit 6fb5ebb into integration)

Conflict counts (baseline):
- `windows` + `macos`: 34 conflict paths
- `develop` + `windows`: 3 conflict paths
- `develop` + `macos`: 29 conflict paths

Merge of macos into integration (G0.2): clean merge, no new conflicts. Committed as 58074c5.

**Freeze rule**: No direct merge into `develop` until G3 dual-platform validation passes. All subsequent commits must carry one of these labels: `audit`, `repair`, `validation`, or `merge-prep`.

## Scope

- Active execution contract: `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md`
- Historical execution contracts:
	- `docs/superpowers/plans/2026-05-21-develop-merge-validation-and-release-hardening.md`
	- `docs/superpowers/plans/2026-05-19-windows-macos-merge-finalization.md`
- Architecture entrypoint: `docs/merge-prep-platform-architecture.md`
- Rehearsal owner: integration lead
- Runtime behavior changes do not belong in this file; record process and evidence only.

## Frozen Shared Contracts

- `webui/desktop/shared/desktop-contract.ts`
- `src/platform/common/status_models.hpp`
- `src/platform/common/runtime_status.hpp`
- `src/platform/common/driver_status.hpp`
- `src/platform/common/helper_client.hpp`

## Lane Ownership

| Lane | Owner Focus | Allowed Primary Files | Blocked Files |
|------|-------------|-----------------------|---------------|
| Lead | Contract freeze, merge playbook, integration, rehearsal | `docs/merge-prep-platform-architecture.md`, `docs/merge-playbooks/*`, `docs/build_guide.md`, `CMakeLists.txt`, wrapper scripts | Should not do bulk edits inside lane-owned implementation files except for integration glue |
| Lane A | Tunnel boundary | `src/tunnel.cpp`, `src/platform/common/tunnel_script.hpp`, `src/platform/{darwin,linux,win32}/tunnel_script.cpp`, `tests/tunnel_script_contract_test.cpp` | `src/helper.cpp`, `src/crypto.cpp`, `webui/desktop/main/*` |
| Lane B | App API fallback policy | `src/app_api.cpp`, `src/platform/common/app_api_runtime_policy.hpp`, `src/platform/{darwin,linux,win32}/app_api_runtime_policy.cpp`, `tests/app_api_runtime_policy_test.cpp` | `src/helper.cpp`, `src/tunnel.cpp`, `webui/desktop/main/*` |
| Lane C | Crypto backend split | `src/crypto.cpp`, `src/platform/common/crypto_backend.hpp`, `src/platform/{darwin,linux,win32}/crypto_backend.cpp`, `tests/crypto_roundtrip_test.cpp` | `src/helper.cpp`, `src/tunnel.cpp`, `webui/desktop/main/*` |
| Lane D | Desktop privilege adapters | `webui/desktop/main/index.ts`, `webui/desktop/main/platform/*` | Native C++ files, wrapper scripts |
| Lane E | Helper lifecycle cleanup | `src/helper.cpp`, `src/platform/common/helper_lifecycle.hpp`, `src/platform/{darwin,linux,win32}/helper_lifecycle.cpp`, `src/platform/common/helper_service_manager.hpp`, `src/platform/{darwin,linux,win32}/helper_service_manager.cpp` | `src/crypto.cpp`, `src/app_api.cpp`, `webui/desktop/main/*` |

## Integration-Only Files

- `CMakeLists.txt`
- `docs/build_guide.md`
- `README.md`
- `README_CN.md`
- `scripts/build-windows.ps1`
- `scripts/build-macos.sh`
- `webui/package.json`

## M0 Baseline Conflict Inventory

Command used to capture the baseline inventory:

- `git diff --name-only windows...macos -- src webui scripts docs`

Current buckets:

- Native shared: `src/app_api.cpp`, `src/app_api.hpp`, `src/config.cpp`, `src/config.hpp`, `src/config_api.cpp`, `src/config_manager.cpp`, `src/config_manager.hpp`, `src/crypto.cpp`, `src/helper.cpp`, `src/helper.hpp`, `src/helper_ipc.hpp`, `src/main.cpp`, `src/tunnel.cpp`, `src/tunnel.hpp`, `src/utils.cpp`, `src/utils.hpp`, `src/virtual_network.cpp`, `src/virtual_network.hpp`, `src/vpn.cpp`, `src/vpn.hpp`, `src/vpn_runtime.cpp`, `src/webui.cpp`, `src/platform/common/driver_status.hpp`, `src/platform/common/runtime_status.hpp`, `src/platform/common/status_models.hpp`
- Desktop shared: `webui/desktop/main/index.ts`, `webui/desktop/preload/index.ts`, `webui/desktop/shared/desktop-contract.ts`, `webui/src/App.vue`, `webui/src/api/desktop.ts`, `webui/src/components/NavBar.vue`, `webui/src/components/StatusBadge.vue`, `webui/src/composables/useSSE.ts`, `webui/src/pages/AuthPage.vue`, `webui/src/pages/LogsPage.vue`, `webui/src/pages/RoutesPage.vue`, `webui/src/pages/ServicePage.vue`, `webui/src/stores/config.ts`, `webui/src/stores/vpn.ts`, `webui/src/types/ecnu-vpn.d.ts`, `webui/tsconfig.electron.json`
- Integration-only currently in diff: `docs/cross-platform-roadmap.md`, `docs/superpowers/plans/2026-05-17-macos-desktop-full-ui-closure.md`, `webui/package.json`
- Platform-owned or supporting diffs: `scripts/install-linux.sh`, `scripts/stage-openconnect-runtime-mac.sh`, `scripts/stage-openconnect-runtime-win.ps1`, `src/helper_daemon_win.cpp`, `src/helper_service_win.cpp`, `src/platform/common/driver_status_stub.cpp`, `src/platform/common/runtime_status.cpp`, `src/platform/common/service_status.hpp`, `src/platform/common/service_status_linux.cpp`, `src/platform/common/status_models.cpp`, `src/platform/darwin/service_status.cpp`, `src/platform/win32/driver_status.cpp`, `src/platform/win32/service_status.cpp`

Active merge-prep hotspots for lane work:

- `src/tunnel.cpp`
- `src/app_api.cpp`
- `src/helper.cpp`
- `src/crypto.cpp`
- `webui/desktop/main/index.ts`

## Lane Launch Notice

Send this working agreement to each implementation lane before more code motion begins:

1. Work in a dedicated worktree for your lane only.
2. Treat the frozen shared contracts as closed unless the integration lead opens a contract task.
3. Do not edit integration-only files during lane execution.
4. If you need a new shared header or contract name, reserve it before coding.
5. After your first substantive edit, run your focused lane validation before any follow-up refactor.
6. Hand off with a short diff summary and the exact validation commands you ran.

## Bidirectional Rehearsal

### Preconditions

- `M1`, `M3`, `M4`, and `M5` are merged or interface-stable.
- `M6` validation wrappers are green.
- Rehearsals run in fresh worktrees only.
- Integration-only files are clean before rehearsal starts.

### Windows to macOS

1. `git worktree add <path> macos`
2. From the macOS rehearsal worktree, run `git merge windows`.
3. Record every conflict file, owner, and chosen resolution in the table below.
4. Run `./scripts/validate-merge-prep-macos.sh`.
5. Record validation results and any residual risk.

### macOS to Windows

1. `git worktree add <path> windows`
2. From the Windows rehearsal worktree, run `git merge macos`.
3. Record every conflict file, owner, and chosen resolution in the table below.
4. Run `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1`.
5. Record validation results and any residual risk.

## Conflict Notes

| Direction | File | Bucket | Owner | Resolution | Follow-up |
|-----------|------|--------|-------|------------|-----------|
| windows -> macos, macos -> windows | `CMakeLists.txt` | Integration-only | Lead | Pending manual merge | Keep under integration lead commit |
| windows -> macos, macos -> windows | `src/app_api.cpp` | Native shared hotspot | Lane B | Pending manual merge | Reconcile through `app_api_runtime_policy` boundaries |
| windows -> macos, macos -> windows | `src/helper.cpp` | Native shared hotspot | Lane E | Pending manual merge | Reconcile through `helper_lifecycle` boundaries |
| windows -> macos, macos -> windows | `src/helper_daemon_win.cpp` | Platform-owned support | Lead + Lane E | Pending manual merge | Verify win helper daemon entrypoint assumptions |
| windows -> macos, macos -> windows | `src/vpn_runtime.cpp` | Native shared | Lead | Pending manual merge | Confirm process-control adapter ownership and behavior |
| windows -> macos, macos -> windows | `tests/vpn_runtime_test.cpp` | Native shared tests | Lead | Pending manual merge | Update test expectations after runtime merge resolution |
| windows -> macos, macos -> windows | `webui/desktop/preload/index.ts` | Desktop shared | Lead + Lane D | Pending manual merge | Keep desktop contract unchanged |
| windows -> macos, macos -> windows | `webui/package.json` | Integration-only | Lead | Pending manual merge | Integrate packaging/build script split |
| windows -> macos, macos -> windows | `webui/scripts/prepare-native.cjs` | Desktop build support | Lead + Lane D | Pending manual merge | Keep split build layout and helper staging |
| windows -> macos, macos -> windows | `webui/src/api/desktop.ts` | Frontend shared | Lead | Pending manual merge | Preserve stable RPC envelope |
| windows -> macos, macos -> windows | `webui/src/components/NavBar.vue` | Frontend shared | Lead | Pending manual merge | Resolve UI parity without contract drift |
| windows -> macos, macos -> windows | `webui/src/composables/useSSE.ts` | Frontend shared | Lead | Pending manual merge | Preserve SSE payload compatibility |
| windows -> macos, macos -> windows | `webui/src/stores/config.ts` | Frontend shared state | Lead | Pending manual merge | Preserve existing persisted config semantics |
| windows -> macos, macos -> windows | `webui/src/stores/vpn.ts` | Frontend shared state | Lead | Pending manual merge | Preserve connect/disconnect action semantics |
| windows -> macos, macos -> windows | `webui/src/types/ecnu-vpn.d.ts` | Frontend shared contract typing | Lead | Pending manual merge | Keep type compatibility with shared desktop contract |

Rehearsal summary:

- Fresh worktrees created:
	- `.claude/worktrees/rehearsal-macos` on `rehearsal/macos-merge-windows`
	- `.claude/worktrees/rehearsal-windows` on `rehearsal/windows-merge-macos`
- `git merge --no-commit --no-ff windows` in `rehearsal-macos` failed with 15 conflicts.
- `git merge --no-commit --no-ff macos` in `rehearsal-windows` failed with the same 15 conflicts.
- Directional asymmetry: none observed in conflict file set.

Current-state reconciliation note:

- The rehearsal worktrees were created from clean branch heads and do not contain the current uncommitted merge-prep files from either active worktree.
- Because of that, the branch-head rehearsal conflict set is useful as a hotspot inventory, but not as the final integration verdict for the current working state.
- Current integration work therefore compares the active macOS workspace against the active `windows` worktree directly, then ports only the Windows-side deltas that are still missing from the current workspace.
- For the current working state, the shared hotspot files `src/app_api.cpp`, `src/helper.cpp`, `src/vpn_runtime.cpp`, `webui/desktop/preload/index.ts`, and `webui/src/api/desktop.ts` are already equal or superseded by the current workspace version; they were not reverted.
- The only Windows-side deltas that were still missing and were ported into the current workspace in this pass are:
	- `src/helper_daemon_win.cpp`
	- `tests/vpn_runtime_test.cpp`
	- `webui/scripts/prepare-native.cjs`

Synthetic merge preview for the current dirty state:

- A no-commit synthetic merge preview was generated from temporary tree snapshots built from the active macOS workspace and the active `windows` worktree, without creating commits or branches.
- Snapshot inputs:
	- `BASE=b2f2a93287e519d1cd2236edb73c24999ee51dd7`
	- `MACOS_TREE=08a9eeaf0b85c99f4a3c3787e4314117209a7bb5`
	- `WINDOWS_TREE=7d877451084ed5a239bca89dc2e478d3dc7ba1a9`
- `git merge-tree` still reports textual conflicts for the current dirty state, but the follow-up per-file comparison shows that many of those conflicts should resolve by keeping the current workspace version because it already supersedes the Windows-side content.
- Current-state conflict files reported by `git merge-tree`:
	- `CMakeLists.txt`
	- `src/app_api.cpp`
	- `src/helper.cpp`
	- `src/helper_daemon_win.cpp`
	- `src/helper_service_win.cpp`
	- `src/platform/common/service_status_linux.cpp`
	- `src/platform/darwin/service_status.cpp`
	- `src/platform/win32/service_status.cpp`
	- `src/vpn_runtime.cpp`
	- `tests/vpn_runtime_test.cpp`
	- `webui/desktop/main/index.ts`
	- `webui/desktop/preload/index.ts`
	- `webui/package.json`
	- `webui/scripts/prepare-native.cjs`
	- `webui/src/api/desktop.ts`
	- `webui/src/components/NavBar.vue`
	- `webui/src/composables/useSSE.ts`
	- `webui/src/stores/config.ts`
	- `webui/src/stores/vpn.ts`
	- `webui/src/types/ecnu-vpn.d.ts`
- Current-state interpretation for those files:
	- Keep current workspace version for `src/app_api.cpp`, `src/helper.cpp`, `src/vpn_runtime.cpp`, `src/helper_service_win.cpp`, `src/platform/common/service_status_linux.cpp`, `src/platform/darwin/service_status.cpp`, `src/platform/win32/service_status.cpp`, `webui/desktop/main/index.ts`, `webui/desktop/preload/index.ts`, `webui/src/api/desktop.ts`, and the equivalent frontend shared files.
	- Keep the integrated current versions of `src/helper_daemon_win.cpp`, `tests/vpn_runtime_test.cpp`, and `webui/scripts/prepare-native.cjs`, which now include the missing Windows-side deltas and have been revalidated.

Current-state manual resolution procedure:

1. Do not reuse `.claude/worktrees/rehearsal-macos` or `.claude/worktrees/rehearsal-windows` for the final gate. Create fresh worktrees from clean commits that already contain the current merge-prep slices.
2. Run the directional merge with `--no-commit --no-ff`, then resolve conflicts by applying the current-state file policy before making any ad hoc edits:
	- Keep the current workspace version for `src/app_api.cpp`, `src/helper.cpp`, `src/vpn_runtime.cpp`, `src/helper_service_win.cpp`, `src/platform/common/service_status_linux.cpp`, `src/platform/darwin/service_status.cpp`, `src/platform/win32/service_status.cpp`, `webui/desktop/main/index.ts`, `webui/desktop/preload/index.ts`, `webui/src/api/desktop.ts`, `webui/src/components/NavBar.vue`, `webui/src/composables/useSSE.ts`, `webui/src/stores/config.ts`, `webui/src/stores/vpn.ts`, and `webui/src/types/ecnu-vpn.d.ts`.
	- Keep the already integrated current versions of `src/helper_daemon_win.cpp`, `tests/vpn_runtime_test.cpp`, and `webui/scripts/prepare-native.cjs`.
3. Resolve the integration-only files last:
	- `CMakeLists.txt`: keep the current workspace target wiring for `vpn_runtime_test`, `tunnel_script_contract_test`, `app_api_runtime_policy_test`, `crypto_roundtrip_test`, and the extracted platform source lists. Reapply opposite-branch hunks only if they add something not already present in the current file.
	- `webui/package.json`: keep the current split desktop build and packaging scripts. Reapply opposite-branch metadata or aliases only if they are absent from the current file after merge.
4. After conflicts are staged, rerun `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1` in the Windows result and `./scripts/validate-merge-prep-macos.sh` in the macOS result before accepting the rehearsal.
5. If any file outside the documented conflict set still collides, or any documented keep-current resolution fails validation, stop and promote that file set into `M8` instead of widening the rehearsal scope.

Actual current-state fresh-worktree rehearsal (2026-05-20):

- Snapshot commits used for the rehearsal:
	- `MACOS_SNAPSHOT=b5511a7d8ace140e9a1e7d139b0f71675f853292`
	- `WINDOWS_SNAPSHOT=4ec85e6d01d7a0036f16715aa668383a4d5b125b`
- Fresh temporary worktrees created from those snapshots:
	- `.claude/worktrees/rehearsal-macos-new`
	- `.claude/worktrees/rehearsal-windows-new`
- Actual conflict files for `windows -> macos`:
	- `CMakeLists.txt`
	- `src/app_api.cpp`
	- `src/helper.cpp`
	- `src/helper_daemon_win.cpp`
	- `src/vpn_runtime.cpp`
	- `tests/vpn_runtime_test.cpp`
	- `webui/desktop/preload/index.ts`
	- `webui/package.json`
	- `webui/scripts/prepare-native.cjs`
	- `webui/src/api/desktop.ts`
	- `webui/src/components/NavBar.vue`
	- `webui/src/composables/useSSE.ts`
	- `webui/src/stores/config.ts`
	- `webui/src/stores/vpn.ts`
	- `webui/src/types/ecnu-vpn.d.ts`
- Actual conflict files for `macos -> windows`: identical to the reverse direction.
- Difference from the earlier synthetic preview: `src/helper_service_win.cpp`, `src/platform/common/service_status_linux.cpp`, `src/platform/darwin/service_status.cpp`, `src/platform/win32/service_status.cpp`, and `webui/desktop/main/index.ts` now merge cleanly in a real current-state rehearsal, so they no longer need to be treated as active residual hotspots.
- Windows-side resolution check: in `.claude/worktrees/rehearsal-windows-new`, resolving the 15 documented conflicts by taking the incoming current-workspace/macOS side produced a merge result that passed manual tracked validation on Windows.
- Windows-side wrapper gate: in `.claude/worktrees/rehearsal-windows-wrapper-6`, a fresh snapshot-based rehearsal also passed `scripts/validate-merge-prep-windows.ps1` after staging the current merge-prep inventory into the active indices, restoring the intended `CMakeLists.txt` blob, fixing the wrapper to build `webui/dist` before the native embed step, and adding the missing `platform/common/config_defaults.hpp` include to `tests/tunnel_script_contract_test.cpp`.

## Final Validation

- Windows wrapper: `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1`
- macOS wrapper: `./scripts/validate-merge-prep-macos.sh`
- Focused native tests remain green.
- Desktop smoke remains green where required.

Current validation evidence:

- `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1 -Action cpp` after `tests/vpn_runtime_test.cpp` merge: pass.
- `ctest --preset windows-release -R 'vpn_runtime_test' --output-on-failure`: pass.
- `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1 -Action cpp` after `src/helper_daemon_win.cpp` merge: pass.
- `build\windows\cpp\exv.exe service status`: pass (`Installed: yes`, `State: running`, `Socket Ready: yes`, `VPN Running: no`).
- `cd webui && npm run prepare:native` after `webui/scripts/prepare-native.cjs` merge: pass.
- `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1`: pass on the current working state after the Windows-side delta integration.
- `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1`: pass again on the current main workspace after the wrapper ordering fix and the `tests/tunnel_script_contract_test.cpp` include fix.
- In `.claude/worktrees/rehearsal-windows-new`, `scripts/validate-merge-prep-windows.ps1` was not present in the snapshot-based worktree, so equivalent tracked validation commands were used instead of the wrapper.
- In `.claude/worktrees/rehearsal-windows-new`, removing 9 temporary `*sync-conflict*` artifact files restored the desktop build path without changing tracked source.
- In `.claude/worktrees/rehearsal-windows-new\webui`, `npm run build`: pass.
- In `.claude/worktrees/rehearsal-windows-new\webui`, `npm run build:electron`: pass.
- In `.claude/worktrees/rehearsal-windows-new\webui`, `npm run prepare:native`: pass.
- In `.claude/worktrees/rehearsal-windows-new`, `cmake -B build -DCMAKE_BUILD_TYPE=Release`: pass.
- In `.claude/worktrees/rehearsal-windows-new`, `cmake --build build --config Release`: pass.
- In `.claude/worktrees/rehearsal-windows-new`, `ctest --test-dir build -C Release -R "platform_status_models_test|vpn_runtime_test|tunnel_script_contract_test|app_api_runtime_policy_test|crypto_roundtrip_test" --output-on-failure`: pass, with `vpn_runtime_test` and `platform_status_models_test` discovered in that rehearsal build tree.
- In `.claude/worktrees/rehearsal-windows-wrapper-6`, `powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1`: pass after the wrapper ordering fix and the `tests/tunnel_script_contract_test.cpp` include fix.

## Residual Risk Capture

| File or Area | Trigger | Action | Owner | Status |
|--------------|---------|--------|-------|--------|
| Shared native hotspots (`src/app_api.cpp`, `src/helper.cpp`, `src/vpn_runtime.cpp`) | Bidirectional rehearsal still conflicts after lane extraction | Execute integration-only conflict-resolution pass and re-run wrappers | Lead + Lane B + Lane E | open |
| Shared desktop layer (`webui/desktop/preload/index.ts`, `webui/src/**`) | 8 frontend/desktop shared files still conflict in the real current-state rehearsal | Resolve in one integration commit with desktop contract freeze check | Lead + Lane D | open |
| Integration-only files (`CMakeLists.txt`, `webui/package.json`) | Rehearsal conflicts include build wiring and packaging | Finalize integration file merges last, then run full validation wrappers | Lead | open |
| Merge-prep inventory in branch history | Snapshot-based fresh worktrees only reflect the real current state after staging the current merge-prep files and tracked modifications into the active indices | Land the staged merge-prep inventory in branch history before using branch-head rehearsals as the final verdict | Lead | open |

Current branch-history landing scope (2026-05-21):

- Main workspace still carries unlanded merge-prep inventory across three buckets: scripts and presets (`CMakePresets.json`, build wrappers, validation wrappers), native/test surfaces (`CMakeLists.txt`, shared hotspot implementations, platform adapter trees, focused regression tests), and desktop/build surfaces (`webui/desktop/main/index.ts`, platform runners, `webui/package.json`, Electron build/staging scripts).
- The active `windows` worktree still carries the Windows-side subset of that landing batch: scripts/presets/wrappers plus desktop build/package wiring (`webui/package.json`, Electron build scripts, `prepare-native.cjs`).
- Until those batches are landed into branch history, branch-head rehearsals remain a hotspot inventory only; the current-state snapshot rehearsal remains the authoritative integration signal.

## S4 Integration Rehearsal (2026-05-21)

Branch: `integration/platform-convergence-next` (from `develop`)

### Step 1: Merge windows into integration

- `git merge windows --no-commit`
- 3 conflicts: `webui/src/api/desktop.ts`, `webui/src/pages/ServicePage.vue`, `webui/src/stores/vpn.ts`
- Resolution: preferred windows-side contract patterns (desktopApiPaths constants, ServiceStatus label/endpoint fields, VpnErrorType)
- Committed as `8b05440`

### Step 2: Merge macos into integration

- `git merge macos --no-commit`
- 47 conflicts across C++ core, platform adapters, tests, docs, and webui
- Resolution strategy:
  - C++ core: preferred macos platform adapter delegation
  - Platform adapters: macos for darwin/common, windows for win32
  - Tests: preferred macos audit-fixed versions
  - Docs: combined both platforms' content
  - Webui: preferred integration branch (richer state machine, structured errors)
- Committed as `75244f0`

### Step 3: Post-merge fixes

- Removed unused Shield/ShieldCheck/ShieldOff imports and serviceName from ServicePage.vue (`fc42921`)
- Removed stale `src/platform/common/helper_platform_linux.cpp` (moved to `src/platform/linux/`) (`f2a6f0c`)

### Validation

- `cmake --build --preset macos-release`: pass
- `ctest --preset macos-release --output-on-failure`: 5/5 passed
- `cd webui && npm run build`: pass

### Conflict inventory (47 files, all resolved)

C++ core (10): .gitignore, CMakeLists.txt, src/app_api.cpp, src/helper.cpp, src/helper.hpp, src/helper_daemon_win.cpp, src/helper_service_win.cpp, src/vpn.hpp, src/vpn_runtime.cpp, scripts/embed_assets.py

Platform adapters (16): src/platform/common/{driver_status.hpp, driver_status_stub.cpp, helper_client.hpp, helper_platform.hpp, runtime_status.cpp, runtime_status.hpp, service_status.hpp, service_status_linux.cpp, status_models.cpp, status_models.hpp}, src/platform/darwin/{helper_client.cpp, helper_platform.cpp, service_status.cpp}, src/platform/win32/{driver_status.cpp, helper_platform.cpp, service_status.cpp}

Tests+docs (4): tests/platform_status_models_test.cpp, tests/vpn_runtime_test.cpp, docs/code_guide.md, docs/user_guide.md

Webui (17): webui/README.md, webui/desktop/main/index.ts, webui/desktop/preload/index.ts, webui/desktop/shared/desktop-contract.ts, webui/package.json, webui/scripts/prepare-native.cjs, webui/src/{App.vue, api/desktop.ts, components/NavBar.vue, composables/useSSE.ts, pages/{DashboardPage.vue, LogsPage.vue, ServicePage.vue, SettingsPage.vue}, stores/{config.ts, vpn.ts}, types/ecnu-vpn.d.ts}

## Closure Criteria

- Both merge directions are rehearsed in fresh worktrees.
- Remaining conflicts are limited to documented integration-only files, or they trigger a new residual-hotspot plan.
- Final validation evidence is attached for both platforms.
