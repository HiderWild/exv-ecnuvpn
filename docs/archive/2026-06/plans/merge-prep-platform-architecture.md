# Merge-Prep Platform Architecture Plan

## Goal

Prepare the `windows` and `macos` branches for low-conflict merging by moving platform-specific behavior out of shared files, while keeping one frontend, one desktop contract, and one shared native RPC surface.

Detailed execution plans:

- `docs/superpowers/plans/2026-05-19-windows-macos-merge-finalization.md`
- `docs/superpowers/plans/2026-05-21-platform-convergence-next-stage.md`

The guiding rule for this phase is:

- Shared files own contract, orchestration, and serialization.
- Platform directories own OS APIs, privileged transport, service/process details, and path/layout differences.
- New branch-specific work should prefer adding platform files over adding more `#ifdef` blocks in shared code.

## Current Baseline

Already completed and validated on both Windows and macOS:

- Shared desktop contract is unified and Windows UI remains the canonical desktop frontend.
- Native build outputs are split by platform under `build/windows/*` and `build/macos/*`.
- Platform config defaults were extracted from shared config code into platform modules.
- Path/home/ownership behavior was extracted from `src/utils.cpp` into platform modules.
- Helper client transport was extracted from shared entrypoints into platform modules and reused by both `app_api.cpp` and `webui.cpp`.

Current platform extraction files:

- `src/platform/common/config_defaults.hpp`
- `src/platform/{darwin,win32}/config_defaults.cpp`
- `src/platform/common/config_defaults_linux.cpp`
- `src/platform/common/path_utils.hpp`
- `src/platform/{darwin,linux,win32}/path_utils.cpp`
- `src/platform/common/helper_client.hpp`
- `src/platform/{darwin,linux,win32}/helper_client.cpp`

## Current Merge Wave Working Agreement

This section is the `M0` baseline for the current merge-prep wave. The detailed execution contract lives in `docs/superpowers/plans/2026-05-19-windows-macos-merge-finalization.md`, and the rehearsal log lives in `docs/merge-playbooks/windows-macos-merge.md`.

### Frozen Shared Contracts

The following files are frozen after `M0`. Open a dedicated contract task before changing any of them:

- `webui/desktop/shared/desktop-contract.ts`
- `src/platform/common/status_models.hpp`
- `src/platform/common/runtime_status.hpp`
- `src/platform/common/driver_status.hpp`
- `src/platform/common/helper_client.hpp`

### Lane Ownership

| Lane | Owner Focus | Allowed Primary Files | Blocked Files |
|------|-------------|-----------------------|---------------|
| Lead | Contract freeze, merge playbook, integration, rehearsal | `docs/merge-prep-platform-architecture.md`, `docs/merge-playbooks/*`, `docs/build_guide.md`, `CMakeLists.txt`, wrapper scripts | Should not do bulk edits inside lane-owned implementation files except for integration glue |
| Lane A | Tunnel boundary | `src/tunnel.cpp`, `src/platform/common/tunnel_script.hpp`, `src/platform/{darwin,linux,win32}/tunnel_script.cpp`, `tests/tunnel_script_contract_test.cpp` | `src/helper.cpp`, `src/crypto.cpp`, `webui/desktop/main/*` |
| Lane B | App API fallback policy | `src/app_api.cpp`, `src/platform/common/app_api_runtime_policy.hpp`, `src/platform/{darwin,linux,win32}/app_api_runtime_policy.cpp`, `tests/app_api_runtime_policy_test.cpp` | `src/helper.cpp`, `src/tunnel.cpp`, `webui/desktop/main/*` |
| Lane C | Crypto backend split | `src/crypto.cpp`, `src/platform/common/crypto_backend.hpp`, `src/platform/{darwin,linux,win32}/crypto_backend.cpp`, `tests/crypto_roundtrip_test.cpp` | `src/helper.cpp`, `src/tunnel.cpp`, `webui/desktop/main/*` |
| Lane D | Desktop privilege adapters | `webui/desktop/main/index.ts`, `webui/desktop/main/platform/*` | Native C++ files, wrapper scripts |
| Lane E | Helper lifecycle cleanup | `src/helper.cpp`, `src/platform/common/helper_lifecycle.hpp`, `src/platform/{darwin,linux,win32}/helper_lifecycle.cpp`, `src/platform/common/helper_service_manager.hpp`, `src/platform/{darwin,linux,win32}/helper_service_manager.cpp` | `src/crypto.cpp`, `src/app_api.cpp`, `webui/desktop/main/*` |

### Integration-Only Files

These files stay under integration-lead control even when they are not part of the current diff inventory:

- `CMakeLists.txt`
- `docs/build_guide.md`
- `README.md`
- `README_CN.md`
- `scripts/build-windows.ps1`
- `scripts/build-macos.sh`
- `webui/package.json`

### Current Conflict Inventory

Baseline command:

- `git diff --name-only windows...macos -- src webui scripts docs`

Primary shared merge hotspots in the current inventory:

- Native shared: `src/app_api.cpp`, `src/app_api.hpp`, `src/config.cpp`, `src/config.hpp`, `src/config_api.cpp`, `src/config_manager.cpp`, `src/config_manager.hpp`, `src/crypto.cpp`, `src/helper.cpp`, `src/helper.hpp`, `src/helper_ipc.hpp`, `src/main.cpp`, `src/tunnel.cpp`, `src/tunnel.hpp`, `src/utils.cpp`, `src/utils.hpp`, `src/virtual_network.cpp`, `src/virtual_network.hpp`, `src/vpn.cpp`, `src/vpn.hpp`, `src/vpn_runtime.cpp`, `src/webui.cpp`, `src/platform/common/driver_status.hpp`, `src/platform/common/runtime_status.hpp`, `src/platform/common/status_models.hpp`
- Desktop shared: `webui/desktop/main/index.ts`, `webui/desktop/preload/index.ts`, `webui/desktop/shared/desktop-contract.ts`, `webui/src/App.vue`, `webui/src/api/desktop.ts`, `webui/src/components/NavBar.vue`, `webui/src/components/StatusBadge.vue`, `webui/src/composables/useSSE.ts`, `webui/src/pages/AuthPage.vue`, `webui/src/pages/LogsPage.vue`, `webui/src/pages/RoutesPage.vue`, `webui/src/pages/ServicePage.vue`, `webui/src/stores/config.ts`, `webui/src/stores/vpn.ts`, `webui/src/types/ecnu-vpn.d.ts`, `webui/tsconfig.electron.json`
- Integration-only currently in diff: `docs/cross-platform-roadmap.md`, `docs/superpowers/plans/2026-05-17-macos-desktop-full-ui-closure.md`, `webui/package.json`
- Platform-owned or supporting diffs outside the shared merge buckets: `scripts/install-linux.sh`, `scripts/stage-openconnect-runtime-mac.sh`, `scripts/stage-openconnect-runtime-win.ps1`, `src/helper_daemon_win.cpp`, `src/helper_service_win.cpp`, `src/platform/common/driver_status_stub.cpp`, `src/platform/common/runtime_status.cpp`, `src/platform/common/service_status.hpp`, `src/platform/common/service_status_linux.cpp`, `src/platform/common/status_models.cpp`, `src/platform/darwin/service_status.cpp`, `src/platform/win32/driver_status.cpp`, `src/platform/win32/service_status.cpp`

The active implementation hotspot list for this wave remains unchanged: `src/tunnel.cpp`, `src/app_api.cpp`, `src/helper.cpp`, `src/crypto.cpp`, and `webui/desktop/main/index.ts`.

### Lane Launch Rules

This working agreement is now in effect for the current merge-prep wave:

- Each implementation lane works in its own worktree.
- No lane edits frozen shared contracts without an explicit follow-up task.
- No lane edits integration-only files during implementation; those changes batch through the integration lead.
- If a lane needs a new shared header or contract name, reserve it before coding.
- After the first substantive edit in a lane, the next action is that lane's focused validation command, not another refactor.
- Each lane handoff must include the exact validation commands that were run.

## Target Structure

Shared native files should converge toward these responsibilities:

- `src/app_api.cpp`: shared desktop RPC actions and response shaping only.
- `src/helper.cpp`: shared helper command routing and state transitions only.
- `src/vpn.cpp`: shared VPN orchestration and runtime state transitions only.
- `src/utils.cpp`: shared file/string/process-neutral utilities only.
- `src/tunnel.cpp`: shared tunnel model and template assembly only.
- `src/crypto.cpp`: shared crypto orchestration only.
- `src/virtual_network.cpp`: shared status shaping only.

Platform directories should converge toward these responsibilities:

- `src/platform/win32/*`: named pipes, service control manager, Windows process control, Windows paths, driver/runtime integration, Electron elevation helpers.
- `src/platform/darwin/*`: launchd/service status, Unix socket transport, root-home ownership recovery, macOS process control, Electron privileged execution.
- `src/platform/linux/*` or `src/platform/common/*` with explicit Linux ownership: Linux-only service/runtime behavior needed to preserve current native CLI support.
- `src/platform/common/*`: cross-platform interfaces, shared DTOs, and logic that is genuinely common without hidden OS branching.

## Conflict Hotspots And Treatment Plan

### 1. `src/app_api.cpp`

Risk:

- Shared RPC layer still contains direct-fallback and privileged execution branching.

Planned treatment:

- Keep shared action routing in `app_api.cpp`.
- Extract direct-connect fallback preparation and helper-unavailable handling helpers into platform-facing modules where behavior diverges.
- Preserve current desktop JSON contract.

Status:

- Helper transport extraction completed.

### 2. `src/helper.cpp`

Risk:

- Service install/uninstall, daemon stop signaling, stable install paths, pipe/socket wake-up, and privilege-sensitive file ownership are still mixed together.

Planned treatment:

- Extract service manager operations into platform modules.
- Extract helper endpoint constants and wake-up logic into platform modules.
- Keep request parsing and shared action handling in `helper.cpp`.

### 3. `src/vpn.cpp` and `src/vpn_runtime.cpp`

Risk:

- Direct session stop logic, PID/process lookup, and platform process termination are still major merge-conflict surfaces.

Planned treatment:

- Extract process lookup/aliveness/termination into platform modules.
- Keep shared VPN lifecycle and runtime snapshot orchestration in common code.
- Continue protecting against killing unrelated `openconnect` processes.

### 4. `src/tunnel.cpp`

Risk:

- Platform command/script generation still tends to drift between branches.

Planned treatment:

- Separate tunnel model assembly from platform script emission.
- Keep route and DNS data shaping shared.
- Generate Windows/Unix artifacts in platform modules.

### 5. `src/virtual_network.cpp`

Risk:

- Adapter inspection and network readiness checks vary significantly by platform.

Planned treatment:

- Keep frontend-facing status fields shared.
- Move adapter enumeration and platform-specific readiness checks into platform modules.

### 6. `src/crypto.cpp`

Risk:

- Platform-specific key storage and entropy/bootstrap behavior can create repeated conflicts.

Planned treatment:

- Keep config encryption/decryption contract shared.
- Move platform-dependent key path and system integration details into platform modules.

### 7. `webui/desktop/main/index.ts`

Risk:

- Elevation and privileged execution remain one of the main Windows/macOS divergence surfaces.

Planned treatment:

- Keep desktop contract, IPC names, and renderer bridge shared.
- Move platform-specific elevation and privileged spawn behavior behind per-platform desktop adapters.

## Implementation Order

The order below is dependency-driven. Earlier phases reduce duplication and make later extractions smaller.

### Phase 1: Native platform boundaries

1. Extract config defaults.
2. Extract path/home/ownership behavior.
3. Extract helper client transport.
4. Extract helper service-manager operations.
5. Extract VPN process/session control.
6. Extract tunnel artifact emission.
7. Extract virtual-network platform inspection.
8. Extract crypto platform integration.

### Phase 2: Desktop privileged execution boundaries

1. Extract Electron privileged command runners.
2. Split Windows/macOS elevation paths behind one shared adapter contract.
3. Keep renderer and shared contract untouched unless the desktop API itself changes.

### Phase 3: Merge hardening

1. Re-check branch conflict hotspots after each extraction slice.
2. Normalize build/package scripts so each branch writes to its own artifact tree.
3. Minimize broad edits to shared files after interfaces stabilize.

## Dependency Graph

Hard dependencies:

- Config defaults -> path/home extraction: config and runtime files depend on stable path ownership behavior.
- Path/home extraction -> helper service extraction: helper install/state files rely on path and ownership primitives.
- Helper service extraction -> VPN process extraction: helper and direct-session control share lifecycle assumptions.
- VPN process extraction -> tunnel and virtual-network extraction: runtime and interface state depend on process/session ownership.
- Native platform boundaries -> Electron elevation extraction: desktop privileged flows should call stable native platform adapters, not unstable shared code.

Soft dependencies:

- Tunnel emission and virtual-network extraction can overlap once VPN process ownership is stable.
- Crypto platform integration can proceed after path ownership is stable.

## Parallelism Guidance

The following tracks can run in parallel once the listed prerequisite is complete.

Parallel lane A:

- `src/helper.cpp` service-manager extraction.

Parallel lane B:

- `src/vpn.cpp` process/session extraction.

Parallel lane C:

- `src/crypto.cpp` platform storage extraction.

Prerequisite for A/B/C:

- Config defaults, path/home, and helper client transport must already be extracted and validated.

After lane B stabilizes, these can run in parallel:

- `src/tunnel.cpp` platform emission extraction.
- `src/virtual_network.cpp` platform inspection extraction.

After native interfaces stabilize, desktop work can split into parallel tasks:

- Electron Windows privileged runner extraction.
- Electron macOS privileged runner extraction.
- Shared desktop adapter contract cleanup.

## Validation Rules For Each Slice

Each implementation slice should follow the same validation sequence:

1. Run the focused Windows native build.
2. Run `platform_status_models_test` and `vpn_runtime_test`.
3. If the slice touches active macOS codepaths, sync the changed native files to `macmini` and run the same native build/test flow there.
4. Only after native validation passes should the next slice begin.

Current validated commands:

- Windows: `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1 -Action cpp`
- Windows: `ctest --test-dir build\windows\cpp -R 'platform_status_models_test|vpn_runtime_test' --output-on-failure`
- macOS: `cmake --preset macos-release`
- macOS: `cmake --build --preset macos-release --target exv platform_status_models_test vpn_runtime_test`
- macOS: `ctest --preset macos-release -R 'platform_status_models_test|vpn_runtime_test' --output-on-failure`

## Review Checklist Before Branch Merge

- Shared desktop contract remains single-source and unchanged across branches.
- Shared frontend continues to prefer the Windows layout and interaction model.
- Shared files no longer accumulate new OS-specific transport/service/process branches.
- Build outputs and packaging outputs stay platform-separated.
- New platform behavior lands under `src/platform/*` or `webui/desktop/*` platform adapters, not in shared orchestration files.
- Windows and macOS native validation remain green after every extraction slice.

## Immediate Next Steps

Recommended next implementation slice order from the current state:

1. Launch `M1`, `M2`, `M3`, and `M5` in separate worktrees using the ownership table above.
2. Hold `CMakeLists.txt`, wrapper scripts, shared docs, and `webui/package.json` for integration-only commits.
3. Start `M4` only after the tunnel and app-api interfaces are stable.
4. Begin `M6` only after the lane-owned test targets and validation inputs stop moving.
