# Native One-Shot Network-Ready Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Valid-password native one-shot connection must either reach stable `network_ready=true` with real adapter/interface metadata, or fail quickly with a specific native failure code and no reconnect flood.

**Architecture:** Treat this as a system boundary failure, not a UI-only failure. Repair the native engine contract so packet-device effective metadata, packet-loop liveness, supervisor startup probation, and helper/desktop error propagation all agree on one state model.

**Tech Stack:** C++17 native engine/helper, Windows Wintun packet device, CSTP/AnyConnect protocol session, Electron desktop RPC/WebUI error contract, CTest unit/integration tests.

---

## L0 System Goal

For `vpn_engine=native`, service unavailable, `allow_direct_fallback=true`, valid credentials, and one-shot helper mode:

- User-mode auth succeeds before helper/UAC startup.
- Helper starts native supervisor with `native_start_mode=auth_session` and no plaintext password in the supervisor payload.
- Supervisor reports `network_ready=true` only after CSTP is connected, the packet device is opened/configured, effective interface metadata is persisted, and the packet loop is still alive.
- A quiet Wintun queue (`no_data`) is treated as idle, not as a terminal connection failure.
- Before first network readiness, supervisor retries are bounded and observable; after a startup failure, one-shot helper stops the supervisor instead of leaving an infinite reconnecting helper process.
- Desktop receives a stable code such as `native_initial_ready_timeout`, `transport_closed`, `tls_handshake_failed`, `native_ip_config_*`, or `wintun_missing`; it must not collapse diagnosable native failures into generic `connection_failed`.

## Evidence From Current Failure

- Live log at `C:\Users\TomLi\AppData\Roaming\ecnuvpn\ecnuvpn.log` shows auth and CSTP success repeatedly: `[native-engine] cstp.connected: CSTP connect succeeded`, then `[native-engine] packet.loop.started: packet loop started`, followed by `Native VPN session ended; reconnect attempt N (infinite mode)`.
- Live state at `C:\Users\TomLi\AppData\Roaming\ecnuvpn\native-session-state.json` shows `packet_loop_ready=true`, `internal_ip4_address=172.20.146.122`, `interface_name=""`, `phase="stopped"`, `network_ready=false`, and empty `failure.code`.
- `src/vpn_engine/native_session_store.cpp:218` requires `session.network_ready()`, non-empty `tunnel.interface_name`, and non-empty `internal_ip4_address` before reporting native readiness.
- `src/platform/win32/native_packet_device.cpp:429` fills a local `configured_metadata.interface_index`; `src/platform/win32/native_packet_device.cpp:432` fills a local Wintun adapter name only if the incoming metadata has an empty interface name. That effective metadata is not propagated back to `ProtocolSession` or the native session store.
- `src/vpn_engine/protocol/session.cpp:171` sets `packet_loop_started()` immediately after `device->open(metadata_)`, but does not update `metadata_` from the platform packet device after Windows assigns the real adapter name/index.
- `src/vpn_engine/protocol/session.hpp:33` sets `packet_loop_no_data_poll_limit=1000`, while `src/platform/win32/native_packet_device.cpp:244` maps an empty Wintun queue to retryable `no_data`. In production, an idle Wintun queue is normal and can last far longer than 1 second.
- `src/vpn.cpp:423` starts native supervisor retries before initial network readiness with `retry_limit=-1` when desktop auto-reconnect is enabled. The live one-shot helper process stayed alive and kept reconnecting after desktop had already returned `connection_failed`.
- `src/helper.cpp:765` attempts to read native failure details only after worker exit; if the supervisor is still alive/retrying and only the readiness window timed out, helper returns the generic branch at `src/helper.cpp:806`.

## L1 Workstreams

| Workstream | Objective | Primary Files | Depends On | Parallelizable |
|---|---|---|---|---|
| A. Evidence and Tests | Lock current failure into deterministic tests before fixes. | `tests/native_helper_session_test.cpp`, `tests/native_protocol_session_test.cpp`, `tests/native_engine_contract_test.cpp`, `tests/win32_native_packet_device_test.cpp` | None | Yes, can run in parallel by subsystem. |
| B. Effective Tunnel Metadata | Propagate Wintun adapter name/interface index into protocol state, session store, and readiness. | `src/vpn_engine/packet_device.hpp`, `src/vpn_engine/protocol/session.*`, `src/platform/win32/native_packet_device.*`, `src/vpn_engine/native_engine.cpp`, tests | A test scaffolding | Can proceed in parallel with C after shared interface is agreed. |
| C. Packet Loop Idle and Liveness | Make `no_data` an idle condition, not a startup-ending condition; use DPD/transport events for death. | `src/vpn_engine/protocol/session.*`, `src/vpn_engine/native_engine.cpp`, tests | A test scaffolding | Parallel with B after tests isolate behavior. |
| D. Supervisor Startup Probation | Separate initial readiness policy from steady-state reconnect policy and stop one-shot supervisors on failed startup. | `src/vpn.cpp`, `src/helper.cpp`, `src/vpn_engine/native_error_contract.hpp`, tests | B and C for final acceptance | Can start with tests; final implementation depends on B/C. |
| E. Diagnostics and UX Contract | Preserve last native failure/state details and surface actionable desktop messages. | `src/vpn_engine/native_session_store.*`, `src/helper.cpp`, `src/feedback/*`, `webui/src/stores/vpn.ts`, `webui/src/types/ecnu-vpn.d.ts`, `webui/src/pages/LogsPage.vue` | D error codes | Parallel after D defines code names. |
| F. Live Validation | Re-run the real one-shot path and verify no orphaned reconnect process remains. | packaged `exv.exe`/`exv-helper.exe`, `%APPDATA%\ecnuvpn` logs/state, Windows process table | B/C/D/E build | Sequential final gate. |

## Detailed Tasks

### Task A1: Preserve the Live Failure Shape as a Regression Test

**Meaning:** The current failure is not password failure; it is a native readiness/state contract failure. The test must fail before the repair if `packet_loop_ready=true`, internal IP exists, interface is empty, and no failure code is persisted.

**Files:**
- Modify: `tests/native_helper_session_test.cpp`
- Modify: `src/vpn_engine/native_session_store.cpp` only if helper accessors are missing for test setup

- [ ] **Step 1: Add a fixture-style test named `test_packet_loop_without_effective_interface_is_not_ready_and_is_diagnostic`.**

  Build a `NativeSessionRecord` with:
  - `record.supervisor_pid = current_process_id()`
  - `record.server = "vpn-ct.ecnu.edu.cn"`
  - `record.route_count = 9`
  - `record.retry_limit = -1`
  - `record.session.tunnel_configured(metadata)` where `metadata.internal_ip4_address = "172.20.146.122"` and `metadata.interface_name = ""`
  - `record.session.packet_loop_started()`
  - Then call `record.session.stopped()` to mirror the live state.

  Expected before repair:
  - `snapshot.running == false`
  - `snapshot.network_ready == false`
  - `snapshot.interface_name.empty()`
  - `snapshot.failure_code.empty()`

  Expected after repair:
  - This exact stale shape remains not-ready, but the supervisor/helper path must never leave this as the final user-facing failure without an explanatory code.

- [ ] **Step 2: Run the target test.**

  Command: `cmake --build build-windows\cpp --target native_helper_session_test`

  Command: `.\build-windows\cpp\native_helper_session_test.exe`

  Acceptance: The new test documents the current live-state shape and fails only on the post-repair diagnostic assertion until Task D/E is implemented.

### Task A2: Add a Packet-Device Effective-Metadata Test

**Meaning:** Windows packet device knows the real Wintun adapter name/index after open. The protocol layer currently cannot see it.

**Files:**
- Modify: `tests/win32_native_packet_device_test.cpp`
- Modify: `tests/native_protocol_session_test.cpp`
- Modify: `tests/native_engine_contract_test.cpp`

- [ ] **Step 1: Extend the fake Wintun test state with `adapter_name = L"ECNUVPN-Test-Wintun"` and `if_index = 77`.**

  Existing test `open_starts_wintun_and_configures_native_ip` already checks `configured_metadata.interface_index == 77`. Add an assertion that the packet device exposes effective metadata with:
  - `interface_index == 77`
  - `interface_name == "ECNUVPN-Test-Wintun"` when incoming CSTP metadata had empty `interface_name`
  - original `internal_ip4_address`, routes, and server bypass routes preserved

- [ ] **Step 2: Add a protocol/session-level test where CSTP metadata has empty `interface_name`.**

  Use a fake packet device that reports effective metadata after `open()`.

  Acceptance:
  - `ProtocolSession::state().tunnel.interface_name` becomes the effective device name after open.
  - `packet.loop.started` readiness uses the effective metadata.
  - The old behavior of preserving a non-empty CSTP interface name remains covered.

- [ ] **Step 3: Add an engine-level test for `NativeVpnEngineSession::status()`.**

  Acceptance:
  - Authenticated start reaches `status.network_ready == true`.
  - `status.interface_name` is the Wintun/effective adapter name when CSTP metadata omitted it.
  - `status.internal_ip` remains the gateway-pushed internal IPv4 address.

### Task B1: Add a PacketDevice Effective Metadata Contract

**Meaning:** `PacketDevice::open()` currently returns only success/failure. Add a narrow platform-neutral way for the protocol session to read the final interface metadata after open.

**Files:**
- Modify: `src/vpn_engine/packet_device.hpp`
- Modify: all fake/test packet devices that need non-default effective metadata
- Modify: `src/platform/win32/native_packet_device.hpp`
- Modify: `src/platform/win32/native_packet_device.cpp`
- Modify: `src/platform/darwin/native_packet_device.*` if the class explicitly implements every virtual method

- [ ] **Step 1: Add a virtual effective metadata method to `PacketDevice`.**

  Preferred contract:
  - `virtual TunnelMetadata effective_metadata() const;`
  - Default implementation returns an empty `TunnelMetadata`.
  - Platform devices override it when `open()` has enough information to report adapter name/index.

  Acceptance: Existing packet devices compile without forced semantic changes; only Windows Wintun needs to return non-empty interface metadata for this bug.

- [ ] **Step 2: Store `effective_metadata_` in `NativePacketDevice`.**

  On successful open:
  - Start from incoming `metadata`.
  - Fill `interface_index` from Wintun `if_index`.
  - Fill `interface_name` from Wintun adapter name when incoming name is empty.
  - Preserve `internal_ip4_address`, `internal_ip4_netmask`, routes, exclude routes, server bypass IPs, DNS, MTU.
  - Clear `effective_metadata_` on `close_resources()`.

  Acceptance: `win32_native_packet_device_test` proves metadata is filled and cleared deterministically.

### Task B2: Use Effective Metadata in Protocol Session and Native Status

**Meaning:** Readiness must be based on actual packet-device metadata, not only gateway metadata.

**Files:**
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `tests/native_protocol_session_test.cpp`
- Modify: `tests/native_engine_contract_test.cpp`

- [ ] **Step 1: After `device->open(metadata_)` succeeds, merge effective metadata.**

  In `ProtocolSession::run_packet_loop`:
  - Call `device->effective_metadata()`.
  - If returned metadata has non-empty `interface_name` or positive `interface_index`, merge it onto `metadata_`.
  - Re-run `state_.tunnel_configured(metadata_)` before `state_.packet_loop_started()`.
  - Emit `packet.device.opened` with `interface`, `interface_index`, and `internal_ip`.

  Acceptance: Event recorder can persist real interface metadata before `packet.loop.started`.

- [ ] **Step 2: Update `NativeVpnEngineSession::on_loop_event`.**

  On `packet.device.opened`:
  - Update `status_.interface_name` and `status_.internal_ip`.
  - Do not mark `network_ready=true` yet.

  On `packet.loop.started`:
  - Mark running and network-ready only if status has a non-empty interface and internal IP.
  - If metadata is missing, persist/emit a specific `native_metadata_incomplete` failure instead of reporting transient success.

  Acceptance: A started loop without interface metadata cannot be mistaken for a ready VPN, and cannot silently degrade to generic `connection_failed`.

### Task C1: Replace the `no_data` Startup Exit With Stable Idle Behavior

**Meaning:** Wintun returning `ERROR_NO_MORE_ITEMS` is normal when no local IP packet is queued. It must not end the VPN session after 1000 idle polls.

**Files:**
- Modify: `src/vpn_engine/protocol/session.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `tests/native_protocol_session_test.cpp`
- Modify: `tests/native_engine_contract_test.cpp`

- [ ] **Step 1: Change default idle policy.**

  Set production default so retryable device reads do not terminate by `packet_loop_no_data_poll_limit`:
  - `packet_loop_no_data_poll_limit = -1` means unlimited idle polling.
  - Use explicit small limits only inside tests that need deterministic end.

  Acceptance: A silent Wintun/device does not return `no_data` as a terminal result while the transport is still alive and cancellation is not requested.

- [ ] **Step 2: Decouple idle polling from dead-peer detection.**

  Keep `no_data` as an idle tick that can service keepalive/DPD timers. Terminate only on:
  - cancellation
  - packet device fatal error other than retryable idle codes
  - inbound `disconnect`
  - transport read/write failure
  - DPD dead-peer budget expiration when DPD is enabled

  Acceptance: `test_active_loop_cancellation_during_no_data_poll_exits` still passes; add a new test proving silent `no_data` waits until cancellation when DPD is disabled.

- [ ] **Step 3: Add bounded test-only options.**

  Tests that previously relied on default 1000-poll exhaustion must explicitly set:
  - `packet_loop_no_data_poll_limit = 3` when checking exhaustion behavior
  - DPD timers and budgets when checking dead-peer reconnect

  Acceptance: Existing DPD/reconnect tests still prove `transport_closed` behavior without using `no_data` as an implicit fatal condition.

### Task C2: Emit Observable Packet Loop Termination Reasons

**Meaning:** The live log currently says “session ended” without a useful engine event in many attempts. Every packet-loop exit must record why.

**Files:**
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `src/vpn_engine/native_session_store.cpp`
- Modify: `tests/native_helper_session_test.cpp`
- Modify: `tests/native_engine_contract_test.cpp`

- [ ] **Step 1: Emit one terminal event for every `run_packet_loop` exit.**

  Event names:
  - `packet.loop.stopped` for cancellation/explicit stop
  - `packet.loop.ended` for clean packet-device drain
  - `packet.loop.failed` for fatal device/transport/protocol errors

  Fields:
  - `code`
  - `message`
  - `running_duration_ms` if available without wall-clock flakiness in tests

  Acceptance: Logs and native session state contain a terminal event after every loop exit.

- [ ] **Step 2: Persist failure only for failure events.**

  In `NativeSessionEventRecorder`, keep `packet.loop.stopped` as stopped, `packet.loop.ended` as stopped/clean, and `packet.loop.failed` as failed.

  Acceptance: Clean stop does not create a false failure; fatal transport/device errors preserve `failure.code` until helper/desktop reads them.

### Task D1: Split Initial Readiness Policy From Steady-State Auto-Reconnect

**Meaning:** Auto-reconnect should not create an infinite startup flood before the first successful network-ready state.

**Files:**
- Modify: `src/vpn.cpp`
- Modify: `src/vpn_engine/native_error_contract.hpp`
- Modify: `tests/native_helper_session_test.cpp`
- Modify: `tests/vpn_supervisor_payload_test.cpp` if payload semantics need a startup policy field

- [ ] **Step 1: Track first `network_ready` inside `run_native_supervisor_impl`.**

  Add state:
  - `bool ever_network_ready = false`
  - `int initial_attempts_used = 0`
  - `int initial_attempt_limit = 1` for auth-session one-shot startup unless config explicitly allows more

  Acceptance: Before first network-ready, supervisor cannot loop infinitely under desktop one-shot startup.

- [ ] **Step 2: Stop retrying on startup metadata/readiness contract failures.**

  Treat these as fatal before first ready:
  - `native_metadata_incomplete`
  - `native_initial_ready_timeout`
  - `packet_loop_not_started`
  - `native_ip_config_*`
  - `native_wintun_*`

  Acceptance: Helper receives a specific code instead of waiting 10 seconds and returning `connection_failed`.

- [ ] **Step 3: Keep steady-state reconnect after the VPN was once ready.**

  After `ever_network_ready=true`, use existing retry policy for transient:
  - `transport_closed`
  - `tls_handshake_failed`
  - `network_unreachable`

  Acceptance: A real network blip after initial success can reconnect; a failure before initial readiness cannot flood.

### Task D2: Make Helper One-Shot Failure Stop the Supervisor

**Meaning:** The current PID `34780` continued reconnecting after desktop returned failure. One-shot mode must not leave orphaned infinite reconnect helpers.

**Files:**
- Modify: `src/helper.cpp`
- Modify: `src/platform/win32/process_control.cpp` or existing supervisor termination wrapper if needed
- Modify: `tests/win32_helper_oneshot_test.cpp`
- Modify: `tests/native_helper_session_test.cpp`

- [ ] **Step 1: On `native_not_ready` / startup failure, terminate the spawned native supervisor.**

  In helper failure handling:
  - Read `supervisor_pid` from native session snapshot or supervisor pid file.
  - If active daemon is one-shot or the worker was started only for this request, request graceful termination.
  - Fall back to force termination only after graceful stop fails.

  Acceptance: After one-shot connect failure, no `exv-helper.exe --oneshot` or native supervisor process remains for the failed request.

- [ ] **Step 2: Persist a final failure before cleanup.**

  Before clearing native state:
  - Persist `native_initial_ready_timeout` with a message containing the last native phase, interface presence, internal IP presence, and last event.
  - Do not include passwords, cookies, auth tokens, or full cookie headers.

  Acceptance: `exv logs` and helper response contain actionable state, not only generic connection failure.

### Task E1: Define Stable Native Error Codes for This Failure Class

**Meaning:** Desktop/UI needs codes that distinguish readiness timeout, metadata contract failure, device failure, transport failure, and auth/session expiry.

**Files:**
- Modify: `src/vpn_engine/native_error_contract.hpp`
- Modify: `src/feedback/feedback.*`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `tests/feedback_test.cpp`
- Modify: `tests/native_helper_session_test.cpp`

- [ ] **Step 1: Add native readiness codes.**

  Codes:
  - `native_initial_ready_timeout`
  - `native_metadata_incomplete`
  - `packet_loop_failed`
  - Preserve existing `wintun_missing`, `tls_verify_failed`, `auth_failed`, `auth_protocol_mismatch`, `reauth_required`, `unsupported_dtls`.

  Acceptance: Unknown native codes no longer collapse to `connection_failed` before being logged; known readiness codes get clear user messages.

- [ ] **Step 2: Add user-facing messages.**

  Messages:
  - `native_initial_ready_timeout`: “Native VPN reached CSTP but did not become network-ready. Check Wintun/interface setup and packet-loop logs.”
  - `native_metadata_incomplete`: “Native VPN opened the packet path but did not report a usable tunnel interface.”
  - `packet_loop_failed`: “Native VPN packet forwarding stopped before the connection became usable.”

  Acceptance: WebUI shows an actionable error while logs retain the internal code and sanitized details.

### Task E2: Add Diagnostic State Snapshots at Component Boundaries

**Meaning:** Future failures should show which layer broke: auth, helper bootstrap, supervisor, CSTP, packet device, IP config, packet loop, or readiness persistence.

**Files:**
- Modify: `src/vpn.cpp`
- Modify: `src/vpn_engine/native_session_store.*`
- Modify: `src/platform/win32/native_packet_device.cpp`
- Modify: `src/platform/win32/native_ip_config.cpp`
- Modify: `src/helper.cpp`
- Modify: `webui/src/pages/LogsPage.vue` only if structured fields are hidden

- [ ] **Step 1: Add sanitized events.**

  Events:
  - `packet.device.opened` with interface name/index and IP presence
  - `packet.device.open_failed` with code/system error
  - `native.readiness.wait` every 1 second during initial wait with phase, supervisor PID, interface present, IP present
  - `native.readiness.timeout` once, with last phase and last event

  Acceptance: A single failed attempt is enough to identify whether the break is metadata, device/IP config, transport, or supervisor readiness.

- [ ] **Step 2: Keep sensitive data out of diagnostics.**

  Acceptance:
  - No plaintext password.
  - No full cookie header.
  - No CSRF/SAML/token value.
  - User name may be length/sanitized only where already allowed by current contract.

### Task F1: Build and Automated Test Gate

**Meaning:** Source repair is not complete until unit and desktop-facing contracts pass.

**Files:**
- No source file edits in this task.

- [ ] **Step 1: Run native C++ focused tests.**

  Command: `cmake --build build-windows\cpp --target native_protocol_session_test native_engine_contract_test native_helper_session_test win32_native_packet_device_test win32_native_ip_config_test`

  Command: `.\build-windows\cpp\native_protocol_session_test.exe`

  Command: `.\build-windows\cpp\native_engine_contract_test.exe`

  Command: `.\build-windows\cpp\native_helper_session_test.exe`

  Command: `.\build-windows\cpp\win32_native_packet_device_test.exe`

  Command: `.\build-windows\cpp\win32_native_ip_config_test.exe`

  Acceptance: All targeted tests pass.

- [ ] **Step 2: Run full native suite.**

  Command: `ctest --preset windows-release --output-on-failure`

  Acceptance: Full Windows CTest preset passes.

- [ ] **Step 3: Run WebUI contract checks.**

  Command: `cd webui; npm run typecheck`

  Command: `cd webui; npm run build`

  Acceptance: Desktop/WebUI TypeScript contracts include new codes and build cleanly.

### Task F2: Live One-Shot Validation Gate

**Meaning:** The repair must be proven against the actual failure path, not only mocks.

**Files:**
- No source file edits in this task.

- [ ] **Step 1: Stop any stale reconnecting helper before the run.**

  Command: `.\build-windows\cpp\exv.exe stop`

  Verification command: `Get-Process exv-helper -ErrorAction SilentlyContinue | Select-Object Id,ProcessName,Path,StartTime`

  Acceptance: No stale one-shot helper remains from a previous failed attempt.

- [ ] **Step 2: Clear only runtime state owned by this app.**

  Use the app’s stop/cleanup path, not manual deletion as the primary method.

  Acceptance: `native-session-state.json` and `route-ready` are absent or reset by the app cleanup path.

- [ ] **Step 3: Run the same desktop one-shot flow with valid credentials.**

  Acceptance if successful:
  - UI reports connected.
  - `native-session-state.json` has `phase="packet_loop"`, `network_ready=true`, non-empty `interface_name`, non-empty `internal_ip4_address`.
  - `route-ready` exists and contains interface and internal IP.
  - Logs contain one `cstp.connected`, one `packet.device.opened`, one `packet.loop.started`, and no reconnect flood for 30 seconds.

  Acceptance if environment rejects the tunnel:
  - UI returns a specific native code.
  - Logs identify the failing boundary.
  - No one-shot helper keeps reconnecting after the UI failure.

## Dependency and Parallelism Plan

### Sequential Spine

1. A1/A2 evidence tests.
2. B1/B2 metadata propagation.
3. C1/C2 idle/liveness and terminal events.
4. D1/D2 supervisor startup probation and one-shot cleanup.
5. E1/E2 user-visible diagnostics.
6. F1/F2 automated and live validation.

### Parallel Lanes

- Agent 1 can own A1, native session store evidence, and helper failure-response tests.
- Agent 2 can own A2, B1, B2 metadata propagation through `PacketDevice`, `ProtocolSession`, and `NativeVpnEngineSession`.
- Agent 3 can own C1/C2 packet-loop idle/liveness and terminal event tests.
- Agent 4 can own D1/D2 supervisor startup policy and one-shot cleanup after Agent 2 and Agent 3 define the final readiness/failure signals.
- Agent 5 can own E1/E2 WebUI/error-contract updates after Agent 4 freezes code names.
- Agent 6 should own F2 live validation only after F1 passes, because live gateway attempts can leave privileged processes and network state behind.

### Blocking Rules

- Do not change frontend messages before E1 freezes error code names.
- Do not tune supervisor retry policy before C1 decides whether `no_data` remains retryable idle or becomes a DPD-driven transport failure.
- Do not claim one-shot is fixed until F2 proves no orphaned reconnecting helper remains after failure.
- Do not remove plaintext-password fallback until existing Task 9 reconnect/reauth semantics are completed and validated.

## Final Acceptance Criteria

- With valid credentials, one-shot native connect does not return generic `connection_failed` after CSTP success.
- If connected, state has non-empty interface name, internal IP, `packet_loop_ready=true`, `phase=packet_loop`, and `network_ready=true`.
- If not connected, desktop receives a specific code and the logs identify the failing boundary.
- Empty Wintun queue does not terminate the VPN session by itself.
- Startup failures before first readiness do not trigger infinite reconnect loops.
- One-shot helper/supervisor processes are cleaned up after failed startup.
- Targeted C++ tests, full CTest preset, WebUI typecheck, and WebUI build pass.
