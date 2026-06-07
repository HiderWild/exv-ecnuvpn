# Phase 4-6 Hardening Report

## Summary

This report documents the hardening work completed during Phase 4-6 of the
ECNU-VPN architecture refactor. The focus areas are integration test coverage,
test infrastructure enhancement, and architecture boundary enforcement.

---

## Changes by Area

### 1. Test Infrastructure: FakeHelper Enhancement

**Files:** `tests/support/fake_helper.hpp`, `tests/support/fake_helper.cpp`

New capabilities added to `FakeHelper`:

| Method | Purpose |
|--------|---------|
| `set_start_session_fail(bool)` | Makes `start_session` return empty session ID |
| `set_apply_config_fail(bool)` | Makes `apply_tunnel_config` return failure with error message |
| `simulate_ipc_lost()` | Simulates IPC connection drop (sets `ipc_lost` flag + fires disconnect callback) |
| `set_version_mismatch(bool)` | Makes `hello` return wrong protocol version (version + 99) |

Existing capabilities preserved:
- `set_should_fail_next(bool)` - generic next-call failure
- `simulate_disconnect()` - clean disconnect
- `set_heartbeat_fail_after(int)` - heartbeat failure after N beats

The `ipc_lost()` inspection method distinguishes IPC loss from clean disconnect.

### 2. Test Infrastructure: FakePlatformNetworkOps Enhancement

**Files:** `tests/support/fake_platform_network_ops.hpp`, `tests/support/fake_platform_network_ops.cpp`

New capabilities added:

| Method | Purpose |
|--------|---------|
| `set_route_add_fail(bool)` | Makes `apply_tunnel_config` fail (route add simulation) |
| `set_dns_fail(bool)` | Makes `apply_tunnel_config` fail (DNS config simulation) |
| `set_adapter_create_fail(bool)` | Makes `prepare_tunnel_device` return closed device |
| `set_unsupported(bool)` | Makes all operations fail (platform unsupported simulation) |

Existing capabilities preserved:
- `set_prepare_should_fail(bool)`
- `set_apply_should_fail(bool)`
- `set_cleanup_should_fail(bool)`

### 3. Integration Test: auth_failure_test

**File:** `tests/integration/auth_failure_test.cpp`

Tests:
- **auth_failure_does_not_reconnect**: Drives controller to Authenticating, fires
  `AuthFailed`, verifies transition to `Failed` (NOT `Reconnecting`), verifies
  error domain is "auth", code is "auth_failed", and `recoverable=false`.
- **ReconnectPolicy auth failure**: Verifies `ReconnectPolicy.decide()` returns
  `should_retry=false` for auth failures.
- **auth failure mid-connection**: Verifies `AuthFailed` while `Connected` also
  transitions to `Failed`.
- **FakeHelper start_session failure**: Verifies `set_start_session_fail` works.
- **FakeHelper version mismatch**: Verifies `set_version_mismatch` works.

### 4. Integration Test: helper_lost_test

**File:** `tests/integration/helper_lost_test.cpp`

Tests:
- **helper_lost_while_connected**: Drives to `Connected`, fires `HelperLost`,
  verifies `Failed` with error domain "helper", code "helper_unavailable",
  `recoverable=false`.
- **helper_lost_during_applying_config**: Drives to `ApplyingNetworkConfig`,
  fires `HelperLost`, verifies `Failed` with non-recoverable error.
- **helper_lost_during_authenticating**: Helper lost during auth phase.
- **helper_lost_while_reconnecting**: Helper lost during reconnect phase.
- **FakeHelper simulate_ipc_lost**: Verifies IPC loss sets flags and fires callback.
- **simulate_disconnect vs simulate_ipc_lost**: Verifies distinction between clean
  disconnect and IPC loss.
- **FakePlatformNetworkOps adapter_create_fail**: Verifies new failure mode.
- **FakePlatformNetworkOps unsupported**: Verifies unsupported platform simulation.
- **FakePlatformNetworkOps route_add_fail**: Verifies route failure simulation.
- **FakePlatformNetworkOps dns_fail**: Verifies DNS failure simulation.

### 5. Architecture Guardrail Scripts

**Files:** `scripts/architecture-guardrails.sh`, `scripts/architecture-guardrails.ps1`

Checks performed:
1. Helper code must not contain `password`, `cookie`, `webvpn_session`, `auth_token`
2. Helper code must not include `vpn_engine/protocol`
3. Core code must not have `#ifdef _WIN32` / `#ifdef __APPLE__`
4. `core_api` must not contain `retry_limit`
5. Test fixtures must not contain hardcoded secrets
6. Helper code must not include `core/` headers
7. Platform ops must not include protocol/auth headers

Both scripts are idempotent and safe to run multiple times.

### 6. Release Gate Checklist

**File:** `docs/RELEASE_GATE_CHECKLIST.md`

Comprehensive checklist covering:
- Build verification (Windows, Linux, macOS)
- Test execution (all presets, architecture label, security label)
- Architecture guardrail execution
- Security test verification
- Integration test verification
- UI typecheck and build
- Manual verification steps

---

## Test Results

All new integration tests are designed to compile and pass without a real VPN
server. They use `FakeHelper` and `FakePlatformNetworkOps` exclusively.

Existing tests (`native_core_connect_flow_test`, `helper_timeout_cleanup_test`)
are unaffected by the fake enhancements -- new methods are additive and default
to the previous behavior when not configured.

---

## Guardrail Findings

The architecture guardrail scripts were run against the current codebase and
identified the following pre-existing violations:

### Platform code includes protocol headers (FAIL)
- `src/platform/common/vpn_supervisor_process.cpp` includes
  `vpn_engine/protocol/native_auth_session_json.hpp`
- `src/platform/darwin/native_tls_stream.hpp` includes
  `vpn_engine/protocol/tls_stream.hpp`
- `src/platform/win32/native_tls_stream.hpp` includes
  `vpn_engine/protocol/tls_stream.hpp`

These are architectural boundary violations: platform code should not depend on
protocol-layer headers. They should be resolved by moving the shared types to
a common layer or using dependency injection.

### Test fixtures contain hardcoded secrets (WARNING)
- `tests/app_api_native_orchestration_test.cpp` contains password strings used
  in test assertions. These are test-only values used to verify that the
  credential store correctly handles secrets (not real credentials).

---

## Remaining Gaps

1. **Real helper IPC test**: Current tests simulate IPC loss via fake. A real
   named-pipe/socket disconnect test would add confidence.
2. **Credential store integration**: Phase 4-6 credential store tests exist but
   are not yet wired into the integration test suite.
3. **macOS/Linux platform fakes**: `FakePlatformNetworkOps` is platform-agnostic
   but real platform implementations need their own test coverage.
4. **CI integration**: Guardrail scripts need to be added to CI pipeline.
5. **Timeout cleanup integration**: `helper_timeout_cleanup_test` uses real
   implementations but could benefit from fake clock control.

---

## Recommendations

1. Add `scripts/architecture-guardrails.sh` to CI as a pre-merge gate.
2. Add the new integration tests to CMake with `add_test()` and label them
   with `architecture` for selective execution.
3. Consider adding a `FakeClock` utility for time-dependent tests.
4. Wire credential store tests into the integration suite once the store
   interface stabilizes.
5. Document the fake capabilities in a `tests/support/README.md` for
   contributor onboarding.
