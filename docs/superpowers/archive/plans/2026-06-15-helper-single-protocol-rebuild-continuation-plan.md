# Helper Single Protocol Rebuild Continuation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover and finish the helper single-protocol packet-ownership/lifecycle work after Claude exited mid-task, then continue toward the final helper single-protocol rebuild.

**Architecture:** Treat the current repository root and Claude continuation worktree as separate authoritative states. The current root is the clean coordination branch; Claude's unfinished code lives in `.claude/worktrees/helper-config-contract-pilot-continuation`. Integrate only reviewed, verified changes from that worktree into `codex/helper-config-contract-pilot`.

**Tech Stack:** C++20, CMake/Ninja, `ctest`, helper contract JSON/messages, Electron/TypeScript tests via `pnpm --dir webui`.

---

## Current State Snapshot

Captured on 2026-06-15.

- Current root: `D:\Development\Projects\cpp\ECNU-VPN`
- Current branch: `codex/helper-config-contract-pilot`
- Current root HEAD: `acd7681 checkpoint: preserve packet ownership handoff slice`
- Current root status when this plan was written: clean except for docs archival/plan edits.
- Claude continuation worktree: `D:\Development\Projects\cpp\ECNU-VPN\.claude\worktrees\helper-config-contract-pilot-continuation`
- Claude continuation branch: `claude/helper-config-contract-pilot-continuation`
- Claude continuation HEAD: `acd7681 checkpoint: preserve packet ownership handoff slice`
- Claude continuation status: 18 modified files, uncommitted.
- Outdated handoff archived to:
  `docs/superpowers/archive/plans/2026-06-14-helper-single-protocol-rebuild-handoff.md`

Other worktrees exist but are not the active source for this helper single-protocol continuation:

- `.claude/worktrees/helper-config-package-consolidation` is on `worktree-helper-config-package-consolidation` at `1fd4a3b` and has untracked package-consolidation docs. Do not mix it into this task unless explicitly asked.
- `.claude/worktrees/agent-adac4466bd091b9a3` is on `worktree-agent-adac4466bd091b9a3` and has unrelated modified files. Do not reuse it for this task.

## Claude's Unfinished Work

Claude's unfinished diff is in `.claude/worktrees/helper-config-contract-pilot-continuation` and currently modifies:

- `src/core/tunnel_controller/core_session_runner.cpp`
- `src/core/tunnel_controller/tunnel_controller.cpp`
- `src/core/tunnel_controller/tunnel_controller_connect.inc.cpp`
- `src/core/tunnel_controller/tunnel_controller_disconnect.inc.cpp`
- `src/core/tunnel_controller/tunnel_controller_events.inc.cpp`
- `src/helper/common/helper_messages.cpp`
- `src/helper/common/helper_messages.hpp`
- `src/helper/helper_network_ops.cpp`
- `src/helper/platform/helper_delegating_network_ops.cpp`
- `src/platform/common/tunnel_config.hpp`
- `src/platform/darwin/platform_network_ops_darwin.cpp`
- `src/platform/win32/platform_network_ops_win32.cpp`
- `tests/helper_delegating_network_ops_test.cpp`
- `tests/helper_messages_connector_test.cpp`
- `tests/helper_network_ops_adapter_test.cpp`
- `tests/support/fake_helper.cpp`
- `tests/support/fake_helper.hpp`
- `tests/tunnel_controller_integration_test.cpp`

Observed substance of the unfinished diff:

- Preserves CSTP split routes by mapping `TunnelMetadata::routes` into `platform::TunnelConfig.routes`.
- Adds `server_bypass_ips` to `platform::TunnelConfig` and helper `TunnelConfig`, including JSON serialization/deserialization and helper delegation.
- Propagates `server_bypass_ips` through helper network ops and Win32/Darwin platform metadata reconstruction.
- Adds controller startup cleanup state through `network_config_applied_`.
- Refactors helper shutdown cleanup into `shutdown_helper_session_for_cleanup()` and adds `cleanup_after_failed_startup()`.
- Updates native start failure classification in `CoreSessionRunner::start()`.
- Disables native engine internal auto-reconnect in `CoreSessionRunner`; reconnect ownership moves to `TunnelController`.
- Updates reconnect timer handling to stop the runner, cleanup prior helper/network session, and re-drive `do_connect()`.
- Ignores duplicate `TransportClosed` while already `Reconnecting`.
- Changes reconnect test counters to `std::atomic<int>`.
- Adds tests for route/bypass preservation, post-network packet startup cleanup, and controller-owned reconnect.

## Evidence Collected

Evidence I verified directly:

- `git -C .claude\worktrees\helper-config-contract-pilot-continuation diff --check` returned exit code 0.
- `git -C .claude\worktrees\helper-config-contract-pilot-continuation diff --stat` shows 18 modified files and 451 insertions / 27 deletions.
- The continuation worktree has no local `build` directory, so I did not rerun CMake tests there during this handoff.

Evidence left by Claude in temp outputs:

- Focused Task 2 output reported 6/6 passing:
  `core_session_runner_test`, `tunnel_controller_integration_test`, `helper_network_ops_adapter_test`, `contract_generation_check`, `helper_messages_connector_test`, `helper_delegating_network_ops_test`.
- Focused Task 3 output reported 8/8 passing:
  `native_engine_contract_test`, `engine_event_bridge_test`, `core_session_runner_test`, `tunnel_controller_integration_test`, `helper_network_ops_adapter_test`, `contract_generation_check`, `helper_messages_connector_test`, `helper_delegating_network_ops_test`.
- Earlier broad temp outputs include failures unrelated to the final focused runs; do not use those old broad outputs as proof of current correctness.

Treat the focused test output as useful but not sufficient. Before committing Claude's diff, rerun verification from the active worktree you are about to commit from.

## Known Open Issues

- Claude's final code was not committed.
- Claude did not leave a final handoff before exiting.
- Task 4 from the prior handoff is still open: the public `TunnelController` constructor exposes `CoreSessionRunner::NativeDependenciesFactory` for test injection. This leaks a native-runner type into the controller facade.
- No full CTest/frontend verification has been rerun after Claude's final reconnect fixes.
- No final spec/quality review verdict was captured after the last fixes for duplicate `TransportClosed` and atomic test counters.
- Linux real `PlatformNetworkOps` backend is still missing.
- Darwin DNS remains explicitly unsupported.
- `PacketDevice::open(TunnelMetadata)` compatibility overloads still exist.
- Elevated/live helper acceptance tests are still incomplete.

## Immediate Recovery Plan

### Task 1: Recover Claude's continuation diff into the active branch

**Files:**
- Source worktree: `.claude/worktrees/helper-config-contract-pilot-continuation`
- Target worktree: current root `D:\Development\Projects\cpp\ECNU-VPN`

- [ ] Confirm current root is still clean:
  `git status --short --branch`
- [ ] Confirm continuation worktree is still dirty with the expected files:
  `git -C .claude\worktrees\helper-config-contract-pilot-continuation status --short --branch`
- [ ] Create a patch from continuation:
  `git -C .claude\worktrees\helper-config-contract-pilot-continuation diff --binary > %TEMP%\helper-continuation.patch`
- [ ] Apply it to the current root:
  `git apply --index %TEMP%\helper-continuation.patch`
- [ ] If patch application fails, stop and resolve only by inspecting both sides. Do not use reset or checkout to discard user/agent work.
- [ ] Unstage if needed before further edits:
  `git restore --staged .`

### Task 2: Verify and tighten route/server-bypass semantics

**Files:**
- Modify: `src/platform/common/tunnel_config.hpp`
- Modify: `src/helper/common/helper_messages.hpp`
- Modify: `src/helper/common/helper_messages.cpp`
- Modify: `src/helper/helper_network_ops.cpp`
- Modify: `src/helper/platform/helper_delegating_network_ops.cpp`
- Modify: `src/platform/win32/platform_network_ops_win32.cpp`
- Modify: `src/platform/darwin/platform_network_ops_darwin.cpp`
- Test: `tests/helper_messages_connector_test.cpp`
- Test: `tests/helper_delegating_network_ops_test.cpp`
- Test: `tests/helper_network_ops_adapter_test.cpp`
- Test: `tests/tunnel_controller_integration_test.cpp`

- [ ] Inspect the continuation diff to confirm `server_bypass_ips` is a distinct list and not encoded as ordinary split routes.
- [ ] Confirm Win32/Darwin platform apply reconstructs `TunnelMetadata::server_bypass_ips` from `platform::TunnelConfig.server_bypass_ips`.
- [ ] Run:
  `cmake --build build --target helper_messages_connector_test helper_delegating_network_ops_test helper_network_ops_adapter_test tunnel_controller_integration_test --config Debug`
- [ ] Run:
  `ctest --test-dir build -C Debug --output-on-failure -R "^(helper_messages_connector_test|helper_delegating_network_ops_test|helper_network_ops_adapter_test|tunnel_controller_integration_test)$"`
- [ ] If any route/bypass propagation is only covered by fakes, add a real boundary test before implementation changes.

### Task 3: Verify post-network startup cleanup

**Files:**
- Modify: `src/core/tunnel_controller/tunnel_controller_connect.inc.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_disconnect.inc.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_events.inc.cpp`
- Test: `tests/tunnel_controller_integration_test.cpp`

- [ ] Confirm cleanup is triggered after network config succeeds but packet-device factory/open fails.
- [ ] Confirm cleanup uses `Shutdown` with full cleanup policy and does not transition to `Idle`.
- [ ] Confirm packet startup error code is preserved and not overwritten by `auth_failed`.
- [ ] Run:
  `cmake --build build --target tunnel_controller_integration_test --config Debug`
- [ ] Run:
  `ctest --test-dir build -C Debug --output-on-failure -R "^tunnel_controller_integration_test$"`

### Task 4: Verify controller-owned reconnect

**Files:**
- Modify: `src/core/tunnel_controller/core_session_runner.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_events.inc.cpp`
- Test: `tests/tunnel_controller_integration_test.cpp`
- Test: `tests/core_session_runner_test.cpp`
- Test: `tests/native_engine_contract_test.cpp`
- Test: `tests/engine_event_bridge_test.cpp`

- [ ] Confirm `CoreSessionRunner` forces `engine_config.auto_reconnect = false`.
- [ ] Confirm `ReconnectTimerFired` stops native runner, cleans previous helper/network session, and calls `do_connect()`.
- [ ] Confirm duplicate `TransportClosed` while already `Reconnecting` is ignored.
- [ ] Confirm test counters touched across threads are `std::atomic<int>`.
- [ ] Run:
  `cmake --build build --target tunnel_controller_integration_test core_session_runner_test native_engine_contract_test engine_event_bridge_test --config Debug`
- [ ] Run:
  `ctest --test-dir build -C Debug --output-on-failure -R "^(tunnel_controller_integration_test|core_session_runner_test|native_engine_contract_test|engine_event_bridge_test)$"`

### Task 5: Hide or narrow the injected native-dependencies seam

**Files:**
- Modify: `src/core/tunnel_controller/tunnel_controller.hpp`
- Modify: `src/core/tunnel_controller/tunnel_controller.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_facade.inc.cpp`
- Modify: `tests/tunnel_controller_integration_test.cpp`

- [ ] Replace the public constructor that exposes `CoreSessionRunner::NativeDependenciesFactory` with a narrower internal/test seam.
- [ ] Keep production `TunnelController` construction unchanged for app/core users.
- [ ] Do not introduce global mutable test hooks.
- [ ] Run:
  `cmake --build build --target tunnel_controller_integration_test --config Debug`
- [ ] Run:
  `ctest --test-dir build -C Debug --output-on-failure -R "^tunnel_controller_integration_test$"`

### Task 6: Review and commit the recovered slice

**Files:**
- All files touched by recovered continuation diff and Task 5.

- [ ] Run helper legacy/static scan:
  `rg -n --glob "!*archive*" -- "--pipe|--socket|__helper-exec|__helper-daemon|helper_v2|HelperV2|protocol_version|server_version|client_version|EndSession|pid:" src\helper contracts src\contracts webui\desktop\shared\generated webui\desktop\main`
- [ ] Run production metadata-open call scan:
  `rg -n -g "*.cpp" -g "*.hpp" -g "*.inc.cpp" -- "->open\(metadata|\.open\(metadata|open\(metadata_" src`
- [ ] Run full verification:
  `cmake --build build --config Debug`
  `ctest --test-dir build -C Debug --output-on-failure`
  `pnpm --dir webui test:contract`
  `pnpm --dir webui test:rpc`
  `git diff --check`
- [ ] Request spec and quality review after the final Task 5 changes. The review prompt must mention routes, server bypass semantics, post-network cleanup, controller-owned reconnect, and the narrowed test seam.
- [ ] Commit only after review verdicts are pass and full verification is green.

## Later Plan After This Slice

- Remove `PacketDevice::open(TunnelMetadata)` compatibility overloads after production call sites remain absent and tests are updated.
- Implement a real Linux `PlatformNetworkOps` backend; do not add production mock/stub behavior.
- Decide Darwin DNS implementation path or keep explicit unsupported behavior with contract/test coverage.
- Add crash/orphan cleanup persistence coverage.
- Add elevated/live acceptance tests for service, oneshot, active shutdown, heartbeat timeout, and crash recovery.

## Stop Rule

If you are not explicitly continuing implementation, stop after updating docs and reporting this plan. Do not commit Claude's unfinished code until it has been recovered into the active branch, reviewed, fully verified, and cleaned up according to this plan.
