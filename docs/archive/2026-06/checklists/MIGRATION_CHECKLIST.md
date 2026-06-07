# Migration Checklist (迁移检查清单)

> Version: Draft 1
> Date: 2026-06-02

This document tracks verification steps for each phase of the architecture migration.
Check off items as they are completed. Each phase must pass all verification steps
before proceeding to the next.

---

## Phase 0: Interface Freeze (接口冻结)

**Goal:** Define all new interfaces without modifying existing code.

- [x] `src/core/` headers created (8 files)
- [x] `src/helper_common/` headers created (7 files)
- [x] `src/helper_runtime/` headers created (6 files)
- [x] `src/platform/common/` new headers created (8 files)
- [x] `src/core_api/` headers created (5 files)
- [x] `src/feedback/error_contract.hpp` created
- [x] `tests/support/` fake implementations created (3 pairs)
- [x] `docs/HELPER_PROTOCOL_V2.md` written
- [x] `docs/CORE_STATE_MACHINE.md` written
- [x] `docs/DESKTOP_RPC_V2.md` written
- [x] `docs/INTERFACE_CHANGELOG.md` written
- [ ] All new headers compile independently (no missing includes)
- [ ] `INTERFACE_CHANGELOG.md` reviewed by all agents

**Verification:**
```
cmake --build . --target check-headers  # or equivalent header-only compile check
```

---

## Phase 1: Low-Risk Extraction (低风险抽取)

**Goal:** Extract timing, error mapping, and split `app_api.cpp` without changing behavior.

### Timing extraction
- [ ] `StageTimer` implementation in `src/core/timing.cpp`
- [ ] All existing timing code migrated to use `StageTimer`
- [ ] Timing tests pass

### Error mapping extraction
- [ ] `CoreErrorMapper` implementation in `src/core/core_error_mapper.cpp`
- [ ] All existing error strings mapped to structured `ErrorInfo`
- [ ] Error mapper unit tests pass

### app_api split
- [ ] `app_rpc_dispatcher.hpp/.cpp` created and functional
- [ ] `vpn_actions.hpp/.cpp` extracted from `app_api.cpp`
- [ ] `config_actions.hpp/.cpp` extracted
- [ ] `service_actions.hpp/.cpp` extracted
- [ ] `route_actions.hpp/.cpp` extracted
- [ ] All existing RPC tests still pass
- [ ] No behavior changes (same request/response format)

**Verification:**
```
ctest --test-dir build/ -R "app_api|timing|error"
```

---

## Phase 2: Core TunnelController (Shadow Mode)

**Goal:** Implement TunnelController in shadow mode alongside existing code.

- [ ] `TunnelController` implementation in `src/core/tunnel_controller.cpp`
- [ ] `ReconnectPolicy` implementation in `src/core/reconnect_policy.cpp`
- [ ] `UserIntent` handling replaces `retry_limit` computation
- [ ] State machine transitions match `CORE_STATE_MACHINE.md`
- [ ] Shadow mode: TunnelController runs alongside existing code, logs decisions
  but does not actuate
- [ ] Shadow mode comparison tests: old path vs new path produce same decisions
- [ ] `FakeHelper` and `FakePlatformNetworkOps` used in unit tests
- [ ] State machine unit tests cover all transitions from diagram
- [ ] Reconnect policy tests cover all rules table entries

**Verification:**
```
ctest --test-dir build/ -R "tunnel_controller|reconnect_policy|state_machine"
```

---

## Phase 3: Helper Protocol V2 (Parallel with V1)

**Goal:** Implement V2 protocol alongside V1. Both work simultaneously.

### Common types
- [ ] `helper_protocol.hpp` types implemented
- [ ] `helper_messages.hpp` serialization/deserialization
- [ ] `helper_capabilities.hpp` constants
- [ ] `helper_error.hpp` error types

### Client side (Core)
- [ ] `HelperClient` interface implementation
- [ ] `HelperConnector` platform factories (Windows, macOS, Linux)
- [ ] Message framing (length-prefixed JSON)
- [ ] Request/response correlation via `request_id`

### Server side (Helper)
- [ ] `HelperServer` listens for connections
- [ ] `HelperRequestDispatcher` routes ops to handlers
- [ ] `SessionLeaseManager` tracks active sessions
- [ ] `CleanupRegistry` records OS artifacts
- [ ] Transient/Resident lifecycle policies

### Integration
- [ ] V2 Hello handshake works end-to-end
- [ ] All 8 operations work end-to-end
- [ ] V1 and V2 run simultaneously without interference
- [ ] `FakeHelper` updated to match final V2 interface

**Verification:**
```
ctest --test-dir build/ -R "helper_v2|helper_protocol|session_lease"
```

---

## Phase 4: Control Plane Migration (控制面迁移)

**Goal:** Route all privileged operations through Helper V2.

- [ ] `PlatformNetworkOps` implementations created (Win32, macOS, Linux)
- [ ] Tunnel device create/open routed through helper
- [ ] IP/route/DNS apply routed through helper
- [ ] Cleanup routed through helper
- [ ] Old direct OS calls wrapped in legacy adapter
- [ ] `FakePlatformNetworkOps` used in all control-plane tests
- [ ] Cleanup registry survives helper crash (test with simulated crash)
- [ ] Route cleanup verified on Windows (previously no-op)

**Verification:**
```
ctest --test-dir build/ -R "platform_network|cleanup|helper_integration"
# Manual: verify routes are cleaned after disconnect on Windows
```

---

## Phase 5: Native Path Switch (原生路径切换)

**Goal:** Replace legacy supervisor-based VPN with TunnelController-driven native path.

- [ ] TunnelController drives full connection lifecycle (not just shadow)
- [ ] Legacy `start`/`stop` actions removed from active code path
- [ ] `legacy_openconnect_adapter` isolates old code
- [ ] Native path: Core handles auth, CSTP, reconnection
- [ ] Helper only does privileged control plane
- [ ] All existing connect/disconnect/reconnect scenarios work
- [ ] `retry_limit` removed from RPC interface
- [ ] `UserIntent` is the only user input for connection

**Verification:**
```
ctest --test-dir build/ -R "native|integration"
# Manual: full connect/disconnect/reconnect cycle on Windows
# Manual: full connect/disconnect/reconnect cycle on macOS
```

---

## Phase 6: Platform Hardening (平台加固)

**Goal:** Security and reliability hardening across all platforms.

### Security
- [ ] Helper validates caller identity (PID, UID, named pipe ACL)
- [ ] Helper rejects any non-V2 protocol messages
- [ ] No password/cookie/token ever reaches helper
- [ ] No shell command execution in helper
- [ ] Credential store uses DPAPI (Windows) / Keychain (macOS) / secret-service (Linux)

### Reliability
- [ ] Cleanup registry survives helper crash and restart
- [ ] Stale sessions cleaned up on helper startup
- [ ] Route cleanup verified on all platforms
- [ ] DNS restore verified on all platforms
- [ ] Firewall rule cleanup verified on Windows

### Testing
- [ ] CI runs all CTest suites
- [ ] CI runs on Windows, macOS, and Linux
- [ ] Regression matrix fully covered (see REGRESSION_MATRIX.md)

**Verification:**
```
ctest --test-dir build/
# Manual: verify cleanup after simulated crash on each platform
# Manual: verify stale session cleanup on helper restart
```

---

## Cross-Phase Rules (跨阶段规则)

1. **No phase may modify interfaces defined in Phase 0** without updating
   `INTERFACE_CHANGELOG.md` first.
2. **Each phase must maintain backward compatibility** with the previous phase's
   public API until explicitly deprecated.
3. **All new code must have unit tests** using the fake implementations from
   `tests/support/`.
4. **Legacy code must be isolated** in `src/legacy/` and marked with deprecation
   comments.
5. **Each phase verification must pass** before the next phase begins.
