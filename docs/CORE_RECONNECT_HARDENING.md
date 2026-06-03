# Core Reconnect Hardening

## Overview

This document describes the core lifecycle hardening additions for the ECNU-VPN tunnel controller. The changes introduce failure classification, injectable clock/random for deterministic testing, and structured startup failure analysis.

## FailurePhase Classification

`FailurePhase` classifies when a failure occurred during the connection lifecycle:

| Phase              | Description                              |
|--------------------|------------------------------------------|
| `PreConnect`       | Before any connection attempt            |
| `Authenticating`   | During authentication                    |
| `Connecting`       | During CSTP/TLS connect                  |
| `StableReady`      | After stable connection established      |
| `Unknown`          | Phase could not be determined            |

## Terminal vs Recoverable Failures

`NativeStartupFailureAnalyzer` classifies failures into three categories:

### Terminal (never retry)
- Auth domain errors (`domain == "auth"`)
- Certificate errors (`code == "cert_error"`)
- Credential expired (`code == "credential_expired"`)

### Recoverable (can retry)
- Transport domain errors that occur **after** a stable connection was established (`was_stable_ready == true && domain == "transport"`)

### Not Recoverable (won't retry, but not terminal)
- Pre-stable failures (connection never became stable)
- Post-stable non-transport failures (e.g., route, DNS, packet device)
- Unknown/unclassified failures

## StartupFailureInfo

Each classified failure produces a `StartupFailureInfo`:

```cpp
struct StartupFailureInfo {
    FailurePhase phase;        // When the failure occurred
    ErrorInfo error;           // Structured error details
    bool is_terminal;          // true = never retry
    bool is_recoverable;       // true = safe to retry
    std::string detail;        // Human-readable classification
};
```

## Injectable Clock Interface

`ReconnectPolicy` supports injectable clock and random functions for deterministic testing:

```cpp
using ClockFunc = std::function<std::chrono::steady_clock::time_point()>;
using RandomFunc = std::function<double()>;  // Returns [0.0, 1.0)
```

### Usage in production (default)
```cpp
ReconnectPolicy policy;  // Uses real steady_clock and random_device
```

### Usage in tests (deterministic)
```cpp
ReconnectConfig config;
config.clock = [&fake_clock]() { return fake_clock(); };
config.random = []() { return 0.5; };  // Fixed midpoint
ReconnectPolicy policy(config);
```

### How jitter works with RandomFunc

The random value maps to jitter as:
```
jitter = (random() * 2.0 - 1.0) * jitter_factor
```

| random() | jitter         | Effect                  |
|----------|----------------|-------------------------|
| 0.0      | -jitter_factor | Minimum delay           |
| 0.5      | 0.0            | No jitter (exact base)  |
| 1.0      | +jitter_factor | Maximum delay           |

## State Machine Relationship

The failure analyzer works alongside the existing `ReconnectPolicy` state machine:

1. Engine receives a native error from the transport/auth layer
2. `NativeStartupFailureAnalyzer::classify()` determines if the failure is terminal/recoverable
3. The classification sets `ErrorInfo.recoverable` appropriately
4. `ReconnectPolicy::decide()` uses `ErrorInfo.recoverable` + `UserIntent` to determine retry behavior
5. If retrying, `ReconnectPolicy::next_delay()` calculates the backoff delay using injectable clock/random

## Native Error Mapping

`map_native_error()` converts raw native error codes into structured `ErrorInfo`:

| API pattern       | Domain      | Code              | Recoverable (StableReady) |
|-------------------|-------------|-------------------|---------------------------|
| `*TLS*`, `*SSL*`  | transport   | tls_error         | Yes                       |
| `*auth*`          | auth        | auth_failed        | No                        |
| `*connect*`       | transport   | transport_closed   | Yes                       |
| Other             | unknown     | native_error       | No                        |

## Test Strategy

### `startup_failure_test.cpp`
- Auth failure is terminal regardless of phase
- Cert error and credential expired are terminal
- Pre-stable transport failure is not recoverable
- Post-stable transport failure is recoverable
- Post-stable non-transport failure is not recoverable
- Unhandled failure defaults
- Native error mapping for TLS, auth, connect, and unknown APIs
- Error info preservation through classify()

### `reconnect_deterministic_test.cpp`
- Uses fake clock (returns predetermined times) and fake random (returns fixed values)
- `auto_reconnect=false` produces no retry
- `user_disconnect` produces no retry
- Auth error (non-recoverable) produces no retry
- `transport_closed` + `auto_reconnect=true` produces retry with keep flags
- Max attempts produces no retry with `max_attempts_reached`
- Fake clock advancement is reflected in `now()`
- Jitter is deterministic: `random=0.75` with `jitter_factor=0.2` produces delay of 1100ms
- Boundary values: `random=0.0` gives minimum, `random=1.0` gives maximum delay
- Backward compatibility: default config (nullptr clock/random) still works

### Existing tests (unmodified)
- `reconnect_policy_test.cpp` continues to pass with the new `ReconnectConfig` fields defaulting to nullptr
- `core_error_mapper_test.cpp` continues to pass unchanged
