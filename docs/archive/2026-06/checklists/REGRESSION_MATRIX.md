# Regression Test Matrix (回归测试矩阵)

> Version: Draft 1
> Date: 2026-06-02

This document defines the regression test scenarios that must pass at each phase
gate. Tests are organized by feature area and platform.

---

## Feature Areas (功能区域)

### 1. Connect (连接)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| C-01 | Basic connect with valid credentials | All | Native | P0 |
| C-02 | Connect with SAML/SSO auth flow | All | Native | P0 |
| C-03 | Connect with MFA challenge | All | Native | P1 |
| C-04 | Connect fails with bad credentials | All | Native | P0 |
| C-05 | Connect fails with cert error | All | Native | P1 |
| C-06 | Connect with proxy (HTTPS proxy) | All | Native | P2 |
| C-07 | Connect to server with self-signed cert (if allowed) | All | Native | P2 |
| C-08 | Connect while already connected (idempotent) | All | Native | P1 |
| C-09 | Connect with split tunnel enabled | All | Native | P2 |
| C-10 | Connect with custom DNS servers | All | Native | P2 |

### 2. Disconnect (断开)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| D-01 | User-initiated disconnect while Connected | All | Native | P0 |
| D-02 | User-initiated disconnect while Authenticating | All | Native | P1 |
| D-03 | User-initiated disconnect while Reconnecting | All | Native | P0 |
| D-04 | Disconnect cleans up routes | All | Native | P0 |
| D-05 | Disconnect cleans up DNS | All | Native | P0 |
| D-06 | Disconnect removes adapter (if configured) | All | Native | P1 |
| D-07 | Disconnect while helper is unavailable | All | Native | P1 |
| D-08 | Double disconnect (idempotent) | All | Native | P1 |
| D-09 | Disconnect during auth (race condition) | All | Native | P2 |

### 3. Auto-Reconnect (自动重连)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| R-01 | Transport drops during Connected, auto_reconnect=true | All | Native | P0 |
| R-02 | Transport drops during Connected, auto_reconnect=false | All | Native | P0 |
| R-03 | Exponential backoff timing (1s, 2s, 4s, ...) | All | Native | P1 |
| R-04 | Backoff caps at max_delay (60s) | All | Native | P1 |
| R-05 | Jitter applied to backoff (+/-20%) | All | Native | P2 |
| R-06 | Attempt counter resets after stable 60s connection | All | Native | P1 |
| R-07 | Reconnect after helper loss (transient) | All | Native | P1 |
| R-08 | Reconnect re-authenticates when session expired | All | Native | P1 |
| R-09 | Reconnect keeps helper session (no re-StartSession) | All | Native | P1 |
| R-10 | Reconnect keeps network config (no re-ApplyTunnelConfig) | All | Native | P2 |
| R-11 | Max attempts exceeded -> Failed state | All | Native | P1 |
| R-12 | User disconnect during Reconnecting stops retries | All | Native | P0 |
| R-13 | Network change during Reconnecting (Wi-Fi to Ethernet) | All | Native | P2 |

### 4. Helper Modes (Helper 模式)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| H-01 | Transient helper starts on connect | All | Native | P0 |
| H-02 | Transient helper exits after idle timeout | All | Native | P1 |
| H-03 | Resident helper stays running after disconnect | All | Native | P1 |
| H-04 | Transient helper cleanup on core crash (lease timeout) | All | Native | P0 |
| H-05 | Resident helper cleanup on core crash (lease timeout) | All | Native | P1 |
| H-06 | Helper Hello handshake with version negotiation | All | Native | P0 |
| H-07 | Helper rejects mismatched protocol version | All | Native | P1 |
| H-08 | Helper capability check (missing capability) | All | Native | P2 |
| H-09 | Helper heartbeat keeps session alive during Reconnecting | All | Native | P0 |
| H-10 | Helper stale session cleanup (no heartbeat) | All | Native | P1 |

### 5. Cleanup (清理)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| CL-01 | Normal cleanup removes all routes | Win, Linux | Native | P0 |
| CL-02 | Normal cleanup restores DNS | All | Native | P0 |
| CL-03 | Normal cleanup removes adapter (full policy) | All | Native | P1 |
| CL-04 | Idempotent cleanup (run twice) | All | Native | P1 |
| CL-05 | Cleanup after crash recovery | All | Native | P0 |
| CL-06 | Cleanup stale sessions on helper restart | All | Native | P1 |
| CL-07 | Partial cleanup failure (some routes removed) | All | Native | P2 |
| CL-08 | Windows route cleanup (previously no-op) | Win | Native | P0 |
| CL-09 | macOS DNS restore | macOS | Native | P1 |
| CL-10 | Linux route cleanup | Linux | Native | P1 |

### 6. Error Handling (错误处理)

| ID | Scenario | Platform | Path | Priority |
|---|---|---|---|---|
| E-01 | Transport closed -> structured ErrorInfo | All | Native | P0 |
| E-02 | Auth failed -> structured ErrorInfo | All | Native | P0 |
| E-03 | Helper unavailable -> structured ErrorInfo | All | Native | P0 |
| E-04 | Route apply failure -> structured ErrorInfo | All | Native | P1 |
| E-05 | DNS apply failure -> structured ErrorInfo | All | Native | P1 |
| E-06 | Packet device failure -> structured ErrorInfo | All | Native | P1 |
| E-07 | ErrorInfo.recoverable correctly set | All | Native | P0 |
| E-08 | ErrorInfo.recommended_action correctly set | All | Native | P1 |
| E-09 | Error domain and code match specification | All | Native | P1 |
| E-10 | Error propagated to UI via status update | All | Native | P0 |

---

## Platform Coverage (平台覆盖)

### Windows

| Feature | Win32 Named Pipe | Wintun Adapter | Route Table | DNS | Firewall |
|---|---|---|---|---|---|
| Connect | Required | Required | Required | Required | Required |
| Disconnect | Required | Required | Required | Required | Required |
| Reconnect | Required | - | Required | - | - |
| Cleanup | Required | Required | Required | Required | Required |
| Crash recovery | Required | Required | Required | - | Required |

### macOS

| Feature | Unix Socket | utun Device | Route Table | DNS | Keychain |
|---|---|---|---|---|---|
| Connect | Required | Required | Required | Required | Required |
| Disconnect | Required | Required | Required | Required | - |
| Reconnect | Required | - | Required | - | - |
| Cleanup | Required | Required | Required | Required | - |
| Crash recovery | Required | Required | Required | - | - |

### Linux

| Feature | Unix Socket | /dev/net/tun | Route Table | DNS | Secret Service |
|---|---|---|---|---|---|
| Connect | Required | Required | Required | Required | Optional |
| Disconnect | Required | Required | Required | Required | - |
| Reconnect | Required | - | Required | - | - |
| Cleanup | Required | Required | Required | Required | - |
| Crash recovery | Required | Required | Required | - | - |

---

## Legacy vs Native Path Coverage (旧路径 vs 原生路径)

During migration (Phases 1-5), both legacy and native paths must be tested.

| Scenario | Legacy Path | Native Path | Notes |
|---|---|---|---|
| Basic connect | Required (until Phase 5) | Required (from Phase 2) | Shadow mode in Phase 2 |
| Disconnect | Required | Required | |
| Auto-reconnect | Required | Required | Native uses ReconnectPolicy |
| Helper communication | V1 protocol | V2 protocol | Both in Phase 3-4 |
| Cleanup | Legacy cleanup | V2 Cleanup op | |
| Error reporting | String errors | Structured ErrorInfo | |
| Status reporting | Flat state string | TunnelStatusSnapshot | |

### Phase Gate Criteria (阶段门标准)

Each phase must meet these criteria before proceeding:

| Phase | Legacy Tests | Native Tests | Coverage |
|---|---|---|---|
| Phase 0 | N/A | N/A | Headers compile |
| Phase 1 | All pass | Unit tests pass | Timing + errors |
| Phase 2 | All pass | Shadow mode comparison pass | State machine |
| Phase 3 | All pass | V2 protocol tests pass | Protocol |
| Phase 4 | All pass | Control plane tests pass | Platform ops |
| Phase 5 | Legacy adapter tests pass | All native tests pass | Full switch |
| Phase 6 | Legacy removed | All tests pass | Hardening |

---

## Test Execution (测试执行)

### Unit Tests

Run with CTest:
```bash
cmake --build build/
ctest --test-dir build/ --output-on-failure
```

### Integration Tests

Require helper binary and platform-specific setup:
```bash
ctest --test-dir build/ -R "integration" --output-on-failure
```

### Manual Regression

For scenarios marked "Manual" in the matrix:
1. Connect to a real VPN server
2. Verify tunnel is operational (ping through VPN)
3. Disconnect and verify cleanup
4. Repeat with auto-reconnect enabled
5. Simulate network drop (disable/enable network adapter)
6. Verify reconnection and cleanup
