# Helper-Lite Native Core Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move native VPN session ownership out of the privileged helper so the C++17 native core owns authentication, CSTP, packet-loop, reconnect, status, and stop semantics while Windows helper paths provide only scoped privileged network operations.

**Architecture:** Preserve the current Electron desktop RPC and Vue 3 + Pinia WebUI contracts as the user-facing control surface. Introduce a user-mode native core supervisor process that owns the native AnyConnect/CSTP engine and uses a helper-lite privilege broker only to lease Wintun packet access and apply Windows IP Helper API configuration. The installed helper service remains an idle privilege broker, and the Windows named-pipe/UAC one-shot helper exposes the same broker contract when no installed service is available.

**Tech Stack:** Electron desktop RPC, Vue 3 + Pinia WebUI, C++17 core, native AnyConnect/CSTP engine, Wintun packet device, Windows named pipes, UAC one-shot helper, installed `exv-helper` Windows service, Windows IP Helper API, CMake/Ninja native tests, WebUI TypeScript checks.

---

## Scope

This is a planning document only. It records the architecture target, code touchpoints, task split, dependencies, and acceptance criteria for implementation workers.

In scope:

- Windows helper-lite ownership model for native production VPN sessions.
- User-mode native core ownership of authentication, CSTP, packet-loop, reconnect policy, status, and stop.
- Privileged helper service and one-shot helper reduced to scoped network privilege broker behavior.
- Desktop/WebUI status and progress semantics needed to observe and stop core-owned sessions.
- Security boundaries that prevent credentials, cookies, and `NativeAuthSession` secrets from entering helper logs, helper request files, or packet broker summaries.

Out of scope for this plan:

- Rewriting AnyConnect XML protocol behavior beyond existing native engine tasks.
- DTLS feature work except preserving the future hook points already described in protocol docs.
- macOS/Linux helper-lite implementation. The interfaces should stay cross-platform-shaped, but the executable target is Windows first.
- Removing legacy diagnostic OpenConnect paths unless a task explicitly marks that path as compatibility-only.

---

## Architecture Topology

### Current Stack

The current Windows native desktop stack is functional but still privilege-centered for session runtime ownership:

```text
Vue 3 + Pinia WebUI
  |
  | renderer API / desktop adapter
  v
Electron desktop RPC
  |
  | child_process: exv.exe desktop-rpc <action> <json>
  v
C++17 core app_api
  |
  | auth-first orchestration creates NativeAuthSession
  | helper start request over named pipe or UAC one-shot request file
  v
Windows named-pipe helper / UAC one-shot helper
  |
  | native_start_mode=auth_session
  v
exv-helper service or oneshot worker
  |
  | owns privileged supervisor, packet device, routes, status cleanup
  v
native AnyConnect/CSTP engine
  |
  | Wintun packet device + Windows IP Helper API route/IP writes
  v
Windows networking
```

Current ownership facts:

- `src/app_api.cpp` and `src/app_api_native_orchestration.*` already contain the native auth-first seam and helper `auth_session` request construction.
- `src/helper.cpp` accepts `native_start_mode=auth_session` and dispatches native worker startup through supervisor payload handling.
- `src/platform/common/vpn_supervisor_process.*` encodes password and `auth_session` supervisor modes.
- `src/vpn_engine/native_engine.*`, `src/vpn_engine/protocol/session.*`, and `src/vpn_engine/protocol/production_transport.*` own native auth, CSTP connect, and packet forwarding logic, but they currently run inside the helper-derived privileged path for desktop native starts.
- `src/platform/win32/native_packet_device.*`, `src/platform/win32/native_wintun.*`, and `src/platform/win32/native_ip_config.*` combine Wintun session management and Windows IP Helper API configuration behind the `PacketDevice` abstraction.

### Target Stack

The target stack makes the helper a narrow broker and moves session truth into user-mode native core:

```text
Vue 3 + Pinia WebUI
  |
  | existing connect/disconnect/status actions
  v
Electron desktop RPC
  |
  | exv.exe desktop-rpc starts/controls user-mode native core supervisor
  v
C++17 native core supervisor
  |
  | owns NativeAuthenticator, NativeAuthSession, CSTP, packet-loop,
  | reconnect policy, stable-ready gate, status, and stop
  v
native AnyConnect/CSTP engine
  |
  | PacketDevice interface implemented by RemotePacketDevice on Windows desktop
  v
Windows named-pipe packet broker protocol
  |
  | installed service path: exv-helper service, idle until lease request
  | fallback path: UAC one-shot helper, started only after auth succeeds
  v
helper-lite privilege broker
  |
  | creates Wintun packet device lease
  | applies/removes Windows IP Helper API address, MTU, DNS, and routes
  | streams raw packet frames only; no credentials, cookies, or CSTP logic
  v
Wintun packet device + Windows IP Helper API
  |
  v
Windows networking
```

Target ownership rules:

- The native core owns AnyConnect authentication, CSTP TLS state, packet forwarding, reconnect eligibility, fatal-error classification, stable-ready state, and status snapshots.
- The helper owns only privileged local effects: Wintun adapter/session creation, Windows IP Helper API address/route/DNS writes, cleanup, and raw packet read/write for an active lease.
- The installed helper service is an idle privilege broker when no packet lease is active. It must not hold credentials, cookies, `NativeAuthSession`, or AnyConnect protocol state.
- The UAC one-shot helper exposes the same broker API as the installed service and exits after its lease is stopped or its owner token expires.
- Desktop RPC remains the user-facing API boundary. New fields are optional and additive unless this plan explicitly says otherwise.

---

## Grounded Findings

- `docs/windows-electron-helper-recovery.md` establishes the Windows runtime split: Electron calls `exv.exe desktop-rpc`, and service lifecycle belongs to `exv-helper.exe --service`, not `exv.exe`.
- `docs/superpowers/plans/2026-06-01-native-auth-helper-decoupling.md` records that native auth-first wiring is complete through `NativeAuthSession`, helper `auth_session` mode, and desktop `connectElevated` normal RPC fallback.
- `docs/superpowers/plans/2026-06-02-native-auth-session-readiness-and-stop-repair.md` identifies the remaining startup ownership problem: worker, helper, supervisor, and UI must agree on stable-ready and stop behavior.
- `docs/architecture/dataplane-forwarding-2026-05-31.md` documents that `ProtocolSession::run_forwarding` already separates CSTP transport read/write from packet device read/write through the `PacketDevice` interface.
- `docs/architecture/native-anyconnect-protocol-requirements.md` defines the long-term native AnyConnect/CSTP behavior, including XML aggregate-auth, CSTP metadata, DNS, routes, keepalive, DPD, reconnect, and error semantics.
- `src/vpn_engine/protocol/native_authenticator.*` and `src/vpn_engine/protocol/native_auth_session_json.*` provide the existing user-mode auth and serializable session primitives that helper-lite must keep out of helper broker payloads.
- `src/platform/win32/native_packet_device.*` already has a testable seam around Wintun packet I/O and native IP configuration through `NativePacketDeviceDependencies`.
- `src/platform/win32/native_ip_config.*` uses Windows IP Helper API functions including `CreateUnicastIpAddressEntry`, `SetIpInterfaceEntry`, `GetBestRoute2`, `CreateIpForwardEntry2`, and `DeleteIpForwardEntry2`.
- `webui/src/stores/vpn.ts` already treats `process_running` as active-session evidence, and the target should reuse that semantics for core-owned sessions.

---

## Public And Internal Interface Changes

### Public Desktop/WebUI Contract

- Keep existing desktop RPC actions:
  - `vpn.connect`
  - `vpn.disconnect`
  - `vpn.status`
  - `service.status`
  - existing service install/uninstall actions
- Keep existing WebUI routes and Pinia store entrypoints.
- Add optional status fields only:
  - `owner`: `"native_core" | "helper_legacy" | "direct" | "disconnected"`
  - `privilege_mode`: `"installed_helper" | "oneshot_helper" | "none"`
  - `packet_device_owner`: `"helper_lite" | "in_process" | "none"`
  - `core_supervisor_pid`: integer when a user-mode native core supervisor is active
  - `packet_lease_id`: redacted lease identifier prefix or opaque non-secret id
  - `stable_ready`: boolean when backend chooses to expose the internal stable-ready gate
- Add progress stages only if the existing estimated stages are insufficient:
  - `native_auth`
  - `broker_ready`
  - `cstp_connect`
  - `packet_lease`
  - `ip_config`
  - `packet_loop`
  - `stable_ready`
- Preserve `connected = process_running && network_ready` semantics. `process_running && !connected` must remain visible and stoppable.

### Helper/Broker IPC Contract

New helper-lite actions over the existing helper transport:

- `native.packet_lease.start`
  - Input: owner token, desktop owner context, sanitized `TunnelMetadata`, requested MTU, route/DNS policy, and desired packet pipe transport.
  - Output: `lease_id`, packet pipe endpoint, control pipe endpoint, interface index, interface name, effective MTU, broker PID if separate, and lease expiry.
  - Forbidden input fields: `password`, `config.password`, `cookie_header`, `auth_session`, `session_token`, `csrf`, `saml`, and raw Authorization headers.
- `native.packet_lease.stop`
  - Input: owner token and `lease_id`.
  - Output: cleanup result with route/DNS/IP cleanup status.
- `native.packet_lease.status`
  - Input: owner token and optional `lease_id`.
  - Output: active lease metadata without secrets.
- `native.packet_lease.ping`
  - Input: owner token.
  - Output: broker readiness and API version.

Packet data protocol:

- Use a dedicated named pipe per lease for length-prefixed binary packet frames.
- Control messages stay newline-delimited JSON.
- Packet frames carry only raw IP packets and close/error signals.
- The helper broker never receives AnyConnect cookies or decrypted HTTP/CSTP control messages.

### Internal C++ Interfaces

Planned new interfaces:

- `PrivilegedNetworkBrokerClient`
  - Starts, stops, and probes helper-lite packet leases.
  - Implemented by Windows helper service and one-shot helper clients.
- `PrivilegedPacketLease`
  - Owns lease id, packet pipe endpoint, control pipe endpoint, effective interface metadata, and cleanup token.
- `RemotePacketDevice`
  - Implements `vpn_engine::PacketDevice`.
  - Reads/writes packet frames over the lease packet pipe.
  - Delegates open/close to `PrivilegedNetworkBrokerClient`.
- `NativeCoreSupervisor`
  - Durable user-mode process that owns `NativeVpnEngineSession`, reconnect, status snapshots, and stop control.
  - Uses `RemotePacketDevice` on Windows desktop helper-lite paths.

Planned stable invariants:

- `NativeAuthSession` remains core-only and is never serialized into helper-lite packet lease requests.
- `TunnelMetadata` may cross into helper-lite because it contains network configuration required for Wintun/IP Helper setup.
- `Config.password` must be cleared before any helper-lite request is encoded.
- Helper logs may include stage names, route counts, interface index, and error codes, but not usernames, passwords, cookies, session tokens, or full auth diagnostics.

---

## L0 Goal

Native Windows desktop sessions are core-owned and helper-lite-privileged: a normal user-mode native core supervisor owns the VPN protocol and lifecycle, while the installed helper service or UAC one-shot helper performs only scoped Wintun/IP Helper operations.

## L1 Overview

| L1 package | Purpose | Primary owner boundary |
| --- | --- | --- |
| A. Contract freeze | Define helper-lite DTOs, forbidden fields, status additions, and broker versioning | `src/platform/common/*`, `webui/desktop/shared/*`, tests |
| B. Broker transport | Add helper-lite packet lease control and packet pipes | `src/helper*`, `src/platform/win32/helper_*`, broker tests |
| C. Remote packet device | Let native engine use helper-leased packet I/O through `PacketDevice` | `src/vpn_engine/*`, `src/platform/win32/*`, packet tests |
| D. User-mode core supervisor | Create durable non-privileged native owner process and control pipe | `src/vpn.cpp`, `src/main.cpp`, `src/app_api*`, supervisor tests |
| E. Desktop orchestration | Rewire connect/disconnect/status to core-owned sessions | `src/app_api*`, Electron preload/main, Pinia store |
| F. Helper de-ownership | Stop desktop native path from starting protocol supervisors inside helper | `src/helper.cpp`, supervisor payload handling, helper tests |
| G. Security and cleanup | Enforce secret redaction, lease TTL, ACLs, and deterministic cleanup | helper lifecycle, broker, session store, security tests |
| H. Validation and rollout | Prove current behavior stays compatible while helper-lite becomes default | CMake tests, WebUI typecheck, manual Windows validation |

---

## L2 Concrete Tasks

### Task 1: Freeze Helper-Lite Contract And DTOs

**Files:**

- Create: `src/platform/common/privileged_network_broker.hpp`
- Create: `src/platform/common/privileged_network_broker.cpp`
- Modify: `src/platform/common/vpn_supervisor_process.hpp`
- Modify: `src/platform/common/vpn_supervisor_process.cpp`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `CMakeLists.txt`
- Test: `tests/privileged_network_broker_contract_test.cpp`
- Test: `tests/vpn_supervisor_payload_test.cpp`

**Implementation:**

- Define `PrivilegedPacketLeaseRequest`, `PrivilegedPacketLeaseDescriptor`, `PrivilegedPacketLeaseStatus`, and `PrivilegedNetworkBrokerVersion`.
- Encode/decode broker requests with deterministic validation errors.
- Add a denylist and allowlist check that rejects secret-bearing fields before helper transport.
- Add optional desktop status fields behind TypeScript types without changing existing action names.

**Acceptance:**

- Broker DTO round-trip preserves route, DNS, MTU, interface, and lease fields.
- Requests containing `password`, `config.password`, `cookie_header`, `auth_session`, `session_token`, `csrf`, `saml`, or `Authorization` are rejected.
- Error messages and summaries redact usernames, cookies, and tokens.
- Existing `vpn_supervisor_payload_test` behavior for password and `auth_session` payloads remains unchanged.

**Verification:**

```powershell
cmake --build build-windows\cpp --target privileged_network_broker_contract_test vpn_supervisor_payload_test
.\build-windows\cpp\privileged_network_broker_contract_test.exe
.\build-windows\cpp\vpn_supervisor_payload_test.exe
cd webui
npm run typecheck
```

### Task 2: Implement Windows Helper-Lite Packet Lease Control

**Files:**

- Modify: `src/helper.cpp`
- Modify: `src/helper_internal.hpp`
- Modify: `src/helper_ipc.hpp`
- Modify: `src/platform/win32/helper_client.cpp`
- Modify: `src/platform/win32/helper_lifecycle.cpp`
- Create: `src/platform/win32/privileged_packet_broker.hpp`
- Create: `src/platform/win32/privileged_packet_broker.cpp`
- Create: `tests/win32_privileged_packet_broker_test.cpp`
- Modify: `tests/native_helper_session_test.cpp`
- Modify: `CMakeLists.txt`

**Implementation:**

- Add helper actions `native.packet_lease.ping`, `native.packet_lease.start`, `native.packet_lease.status`, and `native.packet_lease.stop`.
- Create an owner-restricted named-pipe endpoint for packet frames.
- Use an opaque random `lease_id` and owner token; never derive either from username, server, cookie, or PID alone.
- Keep packet lease creation non-mutating until Wintun and IP configuration are both ready to commit.
- Ensure stop is idempotent and safe after partial start failures.

**Acceptance:**

- Installed helper responds to `native.packet_lease.ping` without starting a VPN process.
- `native.packet_lease.start` returns pipe endpoints only after Wintun session creation and IP Helper configuration succeed.
- Failed lease start cleans partial routes/IP state and closes Wintun handles.
- `native.packet_lease.status` never returns raw routes with secrets, credentials, cookies, or auth diagnostics.
- `native.packet_lease.stop` removes owned routes before closing the Wintun session and succeeds if repeated.

**Verification:**

```powershell
cmake --build build-windows\cpp --target win32_privileged_packet_broker_test native_helper_session_test
.\build-windows\cpp\win32_privileged_packet_broker_test.exe
.\build-windows\cpp\native_helper_session_test.exe
```

### Task 3: Split Local Wintun/IP Helper Implementation From Packet Broker Frontend

**Files:**

- Modify: `src/platform/win32/native_packet_device.hpp`
- Modify: `src/platform/win32/native_packet_device.cpp`
- Modify: `src/platform/win32/native_wintun.hpp`
- Modify: `src/platform/win32/native_wintun.cpp`
- Modify: `src/platform/win32/native_ip_config.hpp`
- Modify: `src/platform/win32/native_ip_config.cpp`
- Modify: `tests/win32_native_packet_device_test.cpp`
- Modify: `tests/win32_native_wintun_test.cpp`
- Modify: `tests/win32_native_ip_config_test.cpp`

**Implementation:**

- Preserve the existing in-process `NativePacketDevice` for elevated/direct diagnostic paths.
- Extract reusable Wintun lease primitives used by both in-process device and helper-lite broker.
- Keep Windows IP Helper API ownership in the broker/elevated process, not in normal desktop RPC.
- Add explicit cleanup ordering: route/DNS/IP cleanup before Wintun session close, with deferred retry when cleanup fails.

**Acceptance:**

- Existing native packet device tests still pass.
- Broker tests can create a fake Wintun/IP Helper lease without constructing a full native VPN session.
- Cleanup order is deterministic: IP Helper route cleanup, DNS cleanup, address cleanup, Wintun session close.
- Failed IP Helper route creation rolls back partial routes and Wintun handles.

**Verification:**

```powershell
cmake --build build-windows\cpp --target win32_native_packet_device_test win32_native_wintun_test win32_native_ip_config_test
.\build-windows\cpp\win32_native_packet_device_test.exe
.\build-windows\cpp\win32_native_wintun_test.exe
.\build-windows\cpp\win32_native_ip_config_test.exe
```

### Task 4: Add RemotePacketDevice For Core-Owned Packet Forwarding

**Files:**

- Create: `src/vpn_engine/remote_packet_device.hpp`
- Create: `src/vpn_engine/remote_packet_device.cpp`
- Modify: `src/vpn_engine/packet_device.hpp`
- Modify: `src/vpn_engine/native_engine.hpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Create: `tests/remote_packet_device_test.cpp`
- Modify: `tests/native_engine_contract_test.cpp`
- Modify: `CMakeLists.txt`

**Implementation:**

- Implement `RemotePacketDevice` as a `vpn_engine::PacketDevice`.
- `open(TunnelMetadata)` calls `PrivilegedNetworkBrokerClient::start_packet_lease`.
- `read_packet` and `write_packet` use length-prefixed packet frames over the lease packet pipe.
- `close` calls broker stop and closes the pipe; it must be safe after read/write errors.
- `effective_interface_metadata` returns the broker-provided interface index/name.

**Acceptance:**

- Native engine can run full-duplex forwarding against a fake remote packet pipe.
- Packet frame boundaries survive partial reads/writes.
- Broker stop runs exactly once on normal close and on open failure rollback.
- `RemotePacketDevice` never sees or serializes `NativeAuthSession`, cookies, or passwords.
- Existing in-process packet-device factory remains available for elevated/direct diagnostics.

**Verification:**

```powershell
cmake --build build-windows\cpp --target remote_packet_device_test native_engine_contract_test native_protocol_session_test
.\build-windows\cpp\remote_packet_device_test.exe
.\build-windows\cpp\native_engine_contract_test.exe
.\build-windows\cpp\native_protocol_session_test.exe
```

### Task 5: Add User-Mode Native Core Supervisor

**Files:**

- Create: `src/platform/common/native_core_supervisor.hpp`
- Create: `src/platform/common/native_core_supervisor.cpp`
- Create: `src/platform/win32/native_core_supervisor.hpp`
- Create: `src/platform/win32/native_core_supervisor.cpp`
- Modify: `src/main.cpp`
- Modify: `src/vpn.cpp`
- Modify: `src/vpn.hpp`
- Modify: `src/vpn_engine/native_session_store.hpp`
- Modify: `src/vpn_engine/native_session_store.cpp`
- Test: `tests/native_core_supervisor_test.cpp`
- Modify: `tests/native_session_state_test.cpp`
- Modify: `CMakeLists.txt`

**Implementation:**

- Add an internal command such as `exv.exe __native-core-supervisor --request-file <path>` for user-mode native runtime ownership.
- Add a user-scoped control endpoint for supervisor status and stop.
- Persist supervisor PID, owner mode, packet lease id, stable-ready state, and network-ready state in user-level session state.
- The supervisor owns `NativeAuthenticator`, `NativeVpnEngineSession`, reconnect arming, packet-loop execution, and stop.
- The supervisor uses `RemotePacketDevice` for Windows helper-lite desktop paths.

**Acceptance:**

- Starting the core supervisor does not require administrator privileges.
- Desktop RPC process can exit after start while the user-mode core supervisor remains the session owner.
- Stop reaches the core supervisor without calling helper native stop except for packet lease cleanup.
- Crashed core supervisor is detected from status and does not leave UI in connected state.
- Stable-ready and reconnect arming follow the contract in `docs/superpowers/plans/2026-06-02-native-auth-session-readiness-and-stop-repair.md`.

**Verification:**

```powershell
cmake --build build-windows\cpp --target native_core_supervisor_test native_session_state_test
.\build-windows\cpp\native_core_supervisor_test.exe
.\build-windows\cpp\native_session_state_test.exe
```

### Task 6: Rewire Desktop Native Connect To Core-Owned Runtime

**Files:**

- Modify: `src/app_api.cpp`
- Modify: `src/app_api_native_orchestration.hpp`
- Modify: `src/app_api_native_orchestration.cpp`
- Modify: `src/platform/common/backend_resolver.hpp`
- Modify: `src/platform/common/backend_resolver.cpp`
- Modify: `src/platform/win32/oneshot_bootstrap.cpp`
- Modify: `tests/app_api_native_orchestration_test.cpp`
- Modify: `tests/app_api_runtime_policy_test.cpp`
- Modify: `tests/backend_resolver_test.cpp`

**Implementation:**

- For installed helper mode, probe helper-lite broker readiness before mutating network state.
- Run native authentication in user mode and keep `NativeAuthSession` in core memory only.
- If installed helper is unavailable, request UAC one-shot only after auth succeeds and before CSTP/packet lease start.
- Start the user-mode native core supervisor with a redacted request file that contains config, retry policy, helper broker endpoint, and password only when needed by the core auth step.
- Ensure helper-lite receives only `TunnelMetadata` and lease policy after core CSTP connect.

**Acceptance:**

- Bad credentials or auth protocol mismatch fail before UAC one-shot and before helper packet lease creation.
- Installed helper unavailable returns the existing remediation message unless one-shot fallback is allowed.
- Successful native desktop connect starts a core-owned session and returns status with `owner=native_core` and `packet_device_owner=helper_lite`.
- Legacy OpenConnect and non-native paths remain unchanged.
- Desktop `connectElevated` remains normal RPC with direct fallback behavior unless one-shot helper is required for packet broker access.

**Verification:**

```powershell
cmake --build build-windows\cpp --target app_api_native_orchestration_test app_api_runtime_policy_test backend_resolver_test
.\build-windows\cpp\app_api_native_orchestration_test.exe
.\build-windows\cpp\app_api_runtime_policy_test.exe
.\build-windows\cpp\backend_resolver_test.exe
```

### Task 7: De-Own Native Protocol Startup In Helper

**Files:**

- Modify: `src/helper.cpp`
- Modify: `src/helper_internal.hpp`
- Modify: `src/platform/common/vpn_supervisor_process.hpp`
- Modify: `src/platform/common/vpn_supervisor_process.cpp`
- Modify: `tests/native_helper_session_test.cpp`
- Modify: `tests/vpn_supervisor_payload_test.cpp`
- Modify: `tests/win32_helper_oneshot_test.cpp`

**Implementation:**

- Mark helper `native_start_mode=auth_session` as legacy compatibility for non-desktop paths or remove it after callers are migrated.
- Reject desktop native helper starts that include `auth_session` once helper-lite packet lease is the default.
- Keep helper service install/uninstall/status behavior unchanged.
- Keep helper stop able to clean helper-owned packet leases even when the core supervisor is gone.

**Acceptance:**

- Desktop native connect cannot start a protocol supervisor inside helper.
- Helper broker start requests with `auth_session` are rejected with deterministic non-secret errors.
- Helper service remains usable for service status, install, uninstall, lease start, lease status, and lease stop.
- One-shot helper exits after lease stop or owner-token expiry.

**Verification:**

```powershell
cmake --build build-windows\cpp --target native_helper_session_test vpn_supervisor_payload_test win32_helper_oneshot_test
.\build-windows\cpp\native_helper_session_test.exe
.\build-windows\cpp\vpn_supervisor_payload_test.exe
.\build-windows\cpp\win32_helper_oneshot_test.exe
```

### Task 8: Update Desktop And Pinia State For Core-Owned Sessions

**Files:**

- Modify: `webui/desktop/main/index.ts`
- Modify: `webui/desktop/preload/index.ts`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/components/ErrorDialog.vue`

**Implementation:**

- Preserve existing connect/disconnect APIs.
- Display core-owned sessions as active when `process_running=true` even before `connected=true`.
- Show helper-lite progress stages when backend reports them.
- Ensure tray quit and dashboard disconnect route through the core supervisor first, then helper-lite lease cleanup.
- Keep service status visually separate from active one-shot packet broker sessions.

**Acceptance:**

- `owner=native_core` with `process_running=true && connected=false` shows a stop action, not a new connect action.
- Auth failures do not show helper/UAC phases.
- Packet lease failures show Wintun/IP Helper remediation, not password retry copy.
- Tray toggle and quit stop core-owned sessions.
- Existing helper service page still reports SCM service status, not one-shot lease status.

**Verification:**

```powershell
cd webui
npm run typecheck
npm run build
```

### Task 9: Enforce Security, ACL, And Secret-Redaction Gates

**Files:**

- Modify: `src/platform/win32/helper_lifecycle.cpp`
- Modify: `src/platform/win32/privileged_packet_broker.cpp`
- Modify: `src/platform/common/privileged_network_broker.cpp`
- Modify: `src/vpn_engine/protocol/native_auth_session_json.cpp`
- Modify: `src/vpn_engine/native_session_store.cpp`
- Modify: `docs/security/native-openconnect-replacement-review-2026-05-31.md`
- Test: `tests/privileged_network_broker_contract_test.cpp`
- Test: `tests/native_helper_session_test.cpp`
- Test: `tests/remote_packet_device_test.cpp`

**Implementation:**

- Apply owner-restricted ACLs to packet pipes, request files, and lease directories.
- Add lease TTL and owner-token validation to every broker action.
- Redact broker summaries and native core session snapshots.
- Ensure crash/error paths do not write packet payload bytes or auth secrets to logs.
- Document the final trust boundary in the security review.

**Acceptance:**

- Non-owner users cannot read helper-lite request files or packet pipes.
- Expired or wrong owner tokens cannot stop or hijack a lease.
- Logs and status summaries contain no password, cookie, `webvpn=`, SAML, CSRF, session token, or packet payload.
- Security doc names the privileged broker boundary and residual risks.

**Verification:**

```powershell
cmake --build build-windows\cpp --target privileged_network_broker_contract_test native_helper_session_test remote_packet_device_test
.\build-windows\cpp\privileged_network_broker_contract_test.exe
.\build-windows\cpp\native_helper_session_test.exe
.\build-windows\cpp\remote_packet_device_test.exe
```

### Task 10: Final Integration, Packaging, And Rollout Defaults

**Files:**

- Modify: `CMakeLists.txt`
- Modify: `scripts/build-windows.ps1`
- Modify: `webui/electron-builder.config.cjs`
- Modify: `docs/windows-electron-helper-recovery.md`
- Modify: `docs/user_guide.md`
- Modify: `docs/build_guide.md`
- Create: `docs/validation/helper-lite-native-core-windows-2026-06-02.md`

**Implementation:**

- Package any new helper-lite broker code with `exv-helper.exe`.
- Ensure `exv.exe` can spawn the user-mode core supervisor from packaged desktop builds.
- Document helper-lite as the default Windows native desktop path.
- Keep a compatibility switch for the old helper-owned native start until one release after helper-lite validation.
- Add manual validation steps for installed helper, UAC one-shot, failed auth, Wintun missing, route cleanup, stop while connecting, and app quit.

**Acceptance:**

- Packaged Windows desktop can connect through installed helper without running the UI as administrator.
- Portable Windows desktop can use UAC one-shot helper after successful auth.
- Failed auth creates no Wintun lease and no helper-owned supervisor.
- Stop and quit clean routes/IP/DNS and close Wintun.
- Documentation is updated so a reviewer can tell helper service status, one-shot broker status, and core session status apart.

**Verification:**

```powershell
cmake --build --preset windows-release
cd webui
npm run desktop:build
```

Manual Windows checks:

```powershell
.\build-windows\cpp\exv.exe service status
.\build-windows\cpp\exv.exe desktop-rpc service.status "{}"
.\build-windows\cpp\exv.exe desktop-rpc vpn.status "{}"
```

---

## Dependency And Parallelism Table

| Task | Depends on | Can run in parallel with | Blocks | Notes |
| --- | --- | --- | --- | --- |
| 1. Contract and DTOs | None | None initially | 2, 4, 6, 8, 9 | Start here; every worker needs stable names and forbidden fields. |
| 2. Helper-lite packet lease control | 1 | 3 after DTO names freeze | 4, 6, 7, 9 | Broker action names and pipe framing must stabilize before client wiring. |
| 3. Wintun/IP Helper split | 1 | 2 | 2 final integration, 4 | Keep existing in-process tests passing during extraction. |
| 4. RemotePacketDevice | 1, 2 protocol shape, 3 lease primitives | 5 after control pipe shape is agreed | 6 | Can use fake broker before real helper service is complete. |
| 5. User-mode core supervisor | 1 | 2, 3, 4 | 6, 8 | Supervisor can first run with in-memory fake packet device. |
| 6. Desktop orchestration | 4, 5 | 8 after status fields freeze | 7, 10 | This is the default-path cutover. |
| 7. Helper de-ownership | 6 | 9 | 10 | Do not reject old helper native starts until desktop callers are migrated. |
| 8. Desktop/Pinia state | 1, 5 status shape | 6 | 10 | UI can be implemented against mocked status payloads. |
| 9. Security gates | 1, 2 | 7, 8 | 10 | Security tests should run before rollout default changes. |
| 10. Integration and rollout | 6, 7, 8, 9 | None | Release readiness | Final docs/package validation only after code behavior is stable. |

Recommended multi-agent split:

- Worker B: Tasks 1 and 4, because DTOs and `RemotePacketDevice` share interface design.
- Worker C: Tasks 2, 3, and 9, because broker implementation and ACL/security gates share Windows helper ownership.
- Worker D: Tasks 5 and 6, because supervisor and desktop orchestration must agree on process ownership.
- Worker E: Task 8, because WebUI/Electron state can proceed from mocked status payloads after Task 1.
- Integration lead: Tasks 7 and 10, because de-owning helper startup and rollout defaults require all previous contracts to be stable.

---

## Test Plan

Native unit and contract tests:

- `privileged_network_broker_contract_test`: broker DTO encode/decode, versioning, forbidden fields, redaction.
- `win32_privileged_packet_broker_test`: helper-lite lease lifecycle, fake pipe endpoints, partial-failure cleanup.
- `remote_packet_device_test`: length-prefixed packet framing, read/write errors, close idempotency.
- `native_core_supervisor_test`: user-mode supervisor start, status, stop, crash detection, stable-ready persistence.
- `native_engine_contract_test`: native engine runs through `RemotePacketDevice` and retains full-duplex forwarding behavior.
- `native_protocol_session_test`: reconnect and CSTP packet loop semantics remain core-owned.
- `native_helper_session_test`: helper rejects secret-bearing broker payloads and cleans leases without owning protocol startup.
- `vpn_supervisor_payload_test`: legacy payload behavior remains stable until removed.
- `app_api_native_orchestration_test`: auth-first, service probe, one-shot timing, and core supervisor request shaping.
- `app_api_runtime_policy_test` and `backend_resolver_test`: helper availability and fallback messages remain stable.
- `win32_native_packet_device_test`, `win32_native_wintun_test`, `win32_native_ip_config_test`: Wintun/IP Helper behavior remains correct after extraction.

Frontend and desktop checks:

- `cd webui; npm run typecheck`
- `cd webui; npm run build`
- Packaged Electron smoke test after native build.

Manual Windows validation:

- Installed helper mode: service installed/running, connect as normal user, no UAC, core-owned status, helper-lite lease active only during session.
- One-shot mode: service unavailable, bad auth returns before UAC, good auth triggers UAC, one-shot broker exits after stop.
- Wintun missing: failure maps to Wintun remediation and creates no core connected state.
- IP Helper route failure: partial routes are cleaned, Wintun closes, UI shows network configuration failure.
- Stop while connecting: `process_running && !connected` is visible and stoppable from dashboard and tray.
- App quit while connected: core supervisor receives stop, broker lease is released, routes/IP/DNS are cleaned.
- Real ECNU gateway: one successful native connection, packet traffic flows, reconnect policy arms only after stable-ready.

No code tests are required for creating this plan document. The implementation workers should run the test commands listed above as they complete each task.

---

## Assumptions And Defaults

- Windows is the first implementation target. Cross-platform abstractions should not force macOS/Linux helper-lite behavior in this wave.
- Existing desktop RPC action names remain stable.
- Existing service install/uninstall/status UX remains stable.
- Installed helper service probing is non-mutating and may happen before auth.
- UAC one-shot helper is launched only after user-mode auth succeeds.
- `NativeAuthSession` and AnyConnect cookies remain in the native core process only.
- `TunnelMetadata` is allowed to cross the helper-lite boundary because Wintun/IP Helper setup requires address, MTU, route, DNS, and interface policy.
- Packet payload bytes are not logged or persisted.
- Helper-lite lease ids and owner tokens are opaque random values with a short TTL.
- Auto-reconnect is armed only after stable-ready; failed first bring-up is terminal and cleaned.
- Direct/elevated in-process packet device remains as a diagnostic fallback until helper-lite has passed packaged Windows validation.
- If helper-lite packet broker is unavailable and direct fallback is not explicitly allowed, desktop connect fails with the existing helper-unavailable remediation.
- Sync-conflict files are not implementation targets unless the active build still compiles them.

---

## Reviewer Audit Checklist

- Can the reviewer identify every planned source/test/doc touchpoint from task `Files:` blocks? Yes.
- Can the reviewer verify no helper-lite request carries credentials or `NativeAuthSession`? Check Task 1, Task 2, and Task 9 acceptance.
- Can the reviewer verify who owns auth, CSTP, packet loop, reconnect, and stop? Check Target Stack and Tasks 4-7.
- Can the reviewer verify Wintun and Windows IP Helper API remain privileged? Check Target Stack, Task 2, and Task 3.
- Can the reviewer verify desktop/WebUI behavior without reading source? Check Public Desktop/WebUI Contract, Task 8, and Test Plan.
- Can the reviewer verify rollout safety? Check Task 10 and Assumptions And Defaults.
