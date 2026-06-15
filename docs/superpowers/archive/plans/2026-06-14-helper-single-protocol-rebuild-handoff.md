# Archived: Helper Single Protocol Rebuild Handoff Plan

Archived on 2026-06-15 because Claude continued this work in
`.claude/worktrees/helper-config-contract-pilot-continuation` after this handoff.
Use `docs/superpowers/plans/2026-06-15-helper-single-protocol-rebuild-continuation-plan.md`
as the current coordination document.

Original title: Helper Single Protocol Rebuild Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the helper single-protocol rebuild so helper has one legal protocol, one lifecycle model, real platform-owned network setup/cleanup, and no production mock/stub or multi-version path.

**Architecture:** Keep `config` and `helper` as the pilot boundary. `config` owns persisted user configuration and credential storage state only. `helper` owns privileged network session operations only. `vpn_engine` must remain protocol/data-plane focused and must receive only device I/O configuration after helper/platform has applied routes, DNS, and adapter setup.

**Tech Stack:** C++20, CMake/Ninja, `ctest`, Python standard-library codegen, Electron/TypeScript tests via `pnpm --dir webui`.

---

## Current Branch State

- Branch: `codex/helper-config-contract-pilot`
- Last committed work:
  - `9740da5 refactor: split helper and config modules`
  - `ee72c7e feat: add config helper contract manifest`
  - `10780c1 Enforce single helper protocol contract`
  - `114d103 Tighten helper oneshot lifecycle contract`
  - `a026a25 Tighten helper production network boundary`
  - `ccab45e Add Darwin helper network backend`
  - `59e9329 Add Windows helper DNS backend`
- Current uncommitted code slice: packet ownership boundary refactor. It is intentionally not committed at handoff because spec review found a blocking route propagation gap.

## Global Progress

- [x] Config/helper split exists on this branch.
- [x] Contract manifest pilot exists in `contracts/system.contract.json`, with generated C++/TypeScript artifacts and drift tests.
- [x] Helper contract naming no longer uses `helper_v2` in the current scanned production/generated paths.
- [x] Helper protocol manifest is narrowed to the single protocol operations: `Hello`, `StartSession`, `PrepareTunnelDevice`, `ApplyTunnelConfig`, `Heartbeat`, `Cleanup`, `GetSnapshot`, `Shutdown`.
- [x] Legacy helper startup routes such as `__helper-exec`, `__helper-daemon`, old pipe/socket fallbacks, and protocol-version fields were removed from the scanned helper paths.
- [x] Oneshot lifecycle work covers explicit legal argv modes, active `Shutdown`, heartbeat maintenance, and cleanup-on-timeout tests.
- [x] Windows helper network path now uses Wintun/IP Helper primitives, records cleanup state, and has a real DNS apply/restore backend through dynamically loaded `GetInterfaceDnsSettings` / `SetInterfaceDnsSettings`.
- [x] Darwin helper network path now has a real `utun`/route command backend and rejects DNS explicitly instead of pretending to apply it.
- [ ] Linux helper network backend is still missing. `PlatformNetworkOps::create()` still has no Linux implementation path.
- [ ] Darwin DNS is still not implemented.
- [ ] Packet-device compatibility overloads still expose `PacketDevice::open(TunnelMetadata)` even though current uncommitted work removes production call sites.
- [ ] Crash/orphan cleanup persistence is not fully wired end-to-end across all platforms.
- [ ] Live elevated acceptance tests for Windows/macOS/Linux service, oneshot, shutdown, heartbeat timeout, and crash recovery are not complete.

## Current Uncommitted Work

The current working tree contains an uncommitted packet ownership refactor:

- `src/vpn_engine/protocol/session.*`: `ProtocolSession::run_packet_loop()` now opens packet devices with `DeviceConfig` instead of full `TunnelMetadata`.
- `src/vpn_engine/native_engine.*`: `NativeVpnEngineDependencies` has a `network_configurator` callback that runs after CSTP connect and before packet device creation/open.
- `src/core/tunnel_controller/core_session_runner.*`: forwards the callback into `NativeVpnEngineSession`.
- `src/core/tunnel_controller/tunnel_controller_*`: wires the callback to helper/platform `prepare_tunnel_device_for_session()` and `apply_tunnel_config_for_session()`.
- `tests/native_protocol_session_test.cpp`, `tests/native_engine_contract_test.cpp`, `tests/core_session_runner_test.cpp`: cover that packet open receives only device I/O config.
- `tests/tunnel_controller_integration_test.cpp`: adds an injected native dependency path and a regression test that preserves `apply_config_failed` instead of overwriting it with `auth_failed`.

Do not commit this slice as-is.

## Blocking Review Finding

Subagent spec review found a real blocker:

- `src/core/tunnel_controller/tunnel_controller_connect.inc.cpp` currently builds `TunnelConfig` with only `interface_address`, `interface_name`, and `mtu`.
- The new native-engine callback receives `TunnelMetadata::routes` and `TunnelMetadata::server_bypass_ips`, but does not map them into `platform::TunnelConfig`.
- Result: route/server-bypass ownership is removed from `PacketDevice`, but not yet transferred to helper/platform network apply.

This violates the intended boundary. The next implementer must fix this before committing the packet ownership slice.

Subagent code-quality review found additional blockers in the same uncommitted slice:

- Privileged network config is applied before later startup failures are ruled out. If packet-device factory/open fails after helper/platform setup, the controller transitions to `Failed` without calling cleanup, and user disconnect is rejected from `Failed`.
- Native-engine reconnect reuses the initial `DeviceConfig`. Fresh CSTP metadata from reconnect is not fed back through the network configurator, so changed IP/MTU/routes/server-bypass values would be ignored.
- `CoreSessionRunner` still maps all `NativeVpnEngineSession::start()` failures to `AuthFailed`, even though failures can now happen after privileged network setup.
- The injected test constructor currently leaks `CoreSessionRunner` dependency types through `TunnelController`'s public facade. That may be acceptable for a test seam only if hidden behind a narrower internal/testing factory before finalizing.

## Verification Already Run

Before the latest injected-controller test was added:

- `cmake --build build --config Debug`: passed.
- `ctest --test-dir build -C Debug --output-on-failure`: 64/64 passed.
- `pnpm --dir webui test:contract`: 6/6 passed.
- `pnpm --dir webui test:rpc`: 22/22 passed.
- Helper legacy/static scan had no matches:
  `rg -n --glob "!*archive*" -- "--pipe|--socket|__helper-exec|__helper-daemon|helper_v2|HelperV2|protocol_version|server_version|client_version|EndSession|pid:" src\helper contracts src\contracts webui\desktop\shared\generated webui\desktop\main`
- Production metadata-open scan had no matches:
  `rg -n -g "*.cpp" -g "*.hpp" -g "*.inc.cpp" -- "->open\(metadata|\.open\(metadata|open\(metadata_" src`

After adding the injected-controller regression test:

- Red: `cmake --build build --target tunnel_controller_integration_test --config Debug` failed because `TunnelController` had no injectable native-dependencies constructor.
- Green: `cmake --build build --target tunnel_controller_integration_test --config Debug; ctest --test-dir build -C Debug --output-on-failure -R "^tunnel_controller_integration_test$"` passed.
- Full CTest and frontend tests were not rerun after the latest injected-controller test and constructor changes.

## Next Phase: Finish Packet Ownership Boundary

### Task 1: Preserve CSTP routes in helper/platform config

**Files:**
- Modify: `src/core/tunnel_controller/tunnel_controller_connect.inc.cpp`
- Modify: `tests/tunnel_controller_integration_test.cpp`
- Inspect: `src/platform/common/tunnel_config.hpp`
- Inspect: `src/vpn_engine/engine.hpp`
- Inspect: `src/helper/platform/helper_delegating_network_ops.cpp`

- [ ] Write a failing test in `tests/tunnel_controller_integration_test.cpp` that uses the injected native dependency path and asserts the `FakePlatformNetworkOps::applied_configs()[0].routes` contains the CSTP route `198.51.100.0/24`.
- [ ] In the same test, assert the server bypass IP `192.0.2.10` is represented in the platform config according to the repo's existing route model. If no explicit bypass route type exists, add the narrowest model field or conversion function first and test it.
- [ ] Run `cmake --build build --target tunnel_controller_integration_test --config Debug` and confirm the test fails because routes are missing from `TunnelConfig`.
- [ ] Update `configure_network_for_engine()` and/or `apply_tunnel_config_for_session()` so CSTP route metadata is converted to `platform::TunnelConfig` before calling `net_ops_->apply_tunnel_config()`.
- [ ] Keep `vpn_engine` free of helper/platform includes. Conversion must stay in core/controller or a core-owned mapping helper.
- [ ] Run:
  `cmake --build build --target tunnel_controller_integration_test --config Debug; ctest --test-dir build -C Debug --output-on-failure -R "^tunnel_controller_integration_test$"`

### Task 2: Add cleanup for post-network startup failures

**Files:**
- Modify: `src/core/tunnel_controller/tunnel_controller_connect.inc.cpp`
- Modify: `src/core/tunnel_controller/core_session_runner.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_events.inc.cpp`
- Test: `tests/tunnel_controller_integration_test.cpp`

- [ ] Write a failing test that makes network config succeed but packet-device creation/open fail, then asserts helper `Shutdown` or cleanup is invoked for the active session.
- [ ] In the same test, assert the controller exposes the failure without blocking cleanup because the phase is already `Failed`.
- [ ] Run `cmake --build build --target tunnel_controller_integration_test --config Debug` and confirm the new test fails.
- [ ] Fix the controller/native-runner error path so any failure after successful `configure_network_for_engine()` triggers full cleanup before or during transition to `Failed`.
- [ ] Avoid classifying post-network failures as `AuthFailed`. Use a more specific event/error path or preserve the native error code in the controller snapshot.
- [ ] Run:
  `cmake --build build --target tunnel_controller_integration_test --config Debug; ctest --test-dir build -C Debug --output-on-failure -R "^tunnel_controller_integration_test$"`

### Task 3: Decide reconnect ownership

**Files:**
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_reconnect.inc.cpp`
- Test: `tests/native_engine_contract_test.cpp`
- Test: `tests/tunnel_controller_integration_test.cpp`

- [ ] Choose one final model before editing production code:
  - Controller-owned reconnect: disable native engine reconnect and let `TunnelController` re-drive helper session, network config, and packet open.
  - Native-owned reconnect: add a callback path that reruns helper/platform network config for every fresh CSTP metadata set.
- [ ] Write a failing test for the chosen model. For controller-owned reconnect, assert native reconnect is not attempted while controller reconnect re-enters helper/platform setup. For native-owned reconnect, assert changed reconnect metadata is passed to network config before packet reopen.
- [ ] Implement only the chosen model and remove the other active reconnect path from production.
- [ ] Run native engine and controller reconnect tests.

### Task 4: Hide or narrow the injected native-dependencies seam

**Files:**
- Modify: `src/core/tunnel_controller/tunnel_controller.hpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_facade.inc.cpp`
- Modify: `tests/tunnel_controller_integration_test.cpp`

- [ ] Replace the public `TunnelController` constructor that exposes `CoreSessionRunner::NativeDependenciesFactory` with a narrower internal/test seam if the production facade should stay free of native-engine types.
- [ ] Keep normal production construction unchanged.
- [ ] Run `cmake --build build --target tunnel_controller_integration_test --config Debug`.

### Task 5: Re-review and commit the packet ownership slice

**Files:**
- Review all current uncommitted source and test files.

- [ ] Run production metadata-open scan:
  `rg -n -g "*.cpp" -g "*.hpp" -g "*.inc.cpp" -- "->open\(metadata|\.open\(metadata|open\(metadata_" src`
- [ ] Run helper legacy/static scan:
  `rg -n --glob "!*archive*" -- "--pipe|--socket|__helper-exec|__helper-daemon|helper_v2|HelperV2|protocol_version|server_version|client_version|EndSession|pid:" src\helper contracts src\contracts webui\desktop\shared\generated webui\desktop\main`
- [ ] Run full verification:
  `cmake --build build --config Debug`
  `ctest --test-dir build -C Debug --output-on-failure`
  `pnpm --dir webui test:contract`
  `pnpm --dir webui test:rpc`
  `git diff --check`
- [ ] Request code review for the packet ownership slice. The review must specifically check route/server-bypass propagation into helper/platform config.
- [ ] Commit only after the review verdict is pass and all full verification commands pass.

### Task 6: Remove remaining production metadata-open compatibility

**Files:**
- Modify: `src/vpn_engine/packet_device.hpp`
- Modify platform packet-device implementations under `src/platform/**`
- Modify packet-device fakes in tests.

- [ ] Write failing compile/test coverage that makes `PacketDevice` expose only `open(const DeviceConfig&)`.
- [ ] Remove `open(const TunnelMetadata&)` from the interface and implementations.
- [ ] Keep all route/DNS application in `PlatformNetworkOps`, not packet-device classes.
- [ ] Run native engine/protocol and platform packet-device tests.

### Task 7: Linux real backend

**Files:**
- Create: `src/platform/linux/platform_network_ops_linux.cpp`
- Modify: `src/platform/common/platform_network_ops.cpp`
- Add tests under `tests/` using fake command/ioctl seams only for unit isolation, plus mark live elevated acceptance coverage separately.

- [ ] Implement a real Linux `PlatformNetworkOps` backend that creates or opens a TUN device through real OS primitives.
- [ ] Apply routes through the existing Linux route machinery or a narrower platform command abstraction.
- [ ] Do not return production mocks or silent no-ops.
- [ ] Explicitly fail DNS until a real Linux DNS backend is added, or implement a real systemd-resolved/resolvconf backend with cleanup state.

### Task 8: Final helper lifecycle acceptance

**Files:**
- Inspect helper runtime under `src/helper/**`
- Inspect platform network ops under `src/platform/**`
- Add platform-specific elevated acceptance tests where build/CI environment supports them.

- [ ] Verify legal helper starts are only `exv-helper --service` and `exv-helper --oneshot --endpoint <random> --owner <uid/sid> --parent-pid <pid>`.
- [ ] Verify oneshot accepts active `Shutdown` and exits after cleanup.
- [ ] Verify helper checks heartbeat every 15 seconds and core sends heartbeat every 10 seconds after `StartSession`.
- [ ] Verify heartbeat timeout performs full cleanup before oneshot exit or service session reset.
- [ ] Verify cleanup order is route/DNS/firewall first, adapter last.
- [ ] Verify failed cleanup entries stay recorded and are retried.

## Stop Rule for This Handoff

Do not continue implementation from the current state unless explicitly instructed. The immediate safe action is to continue from Task 1 in this document, not to commit the current uncommitted packet ownership slice.
