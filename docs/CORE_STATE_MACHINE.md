# Core TunnelController State Machine Specification

> Version: Draft 1
> Date: 2026-06-02
> Status: Phase 0 Interface Freeze

---

## TunnelPhase States (状态定义)

The `TunnelPhase` enum defines all possible states of a VPN connection lifecycle:

| Phase | Description |
|---|---|
| `Idle` | No active connection. Initial state. No helper session, no network config. |
| `PreparingHelper` | Connecting to helper daemon, performing Hello handshake, starting session. |
| `Authenticating` | Core is performing AnyConnect authentication (HTTPS POST, SAML, MFA). |
| `ConnectingCstp` | Authentication succeeded; establishing CSTP/TLS tunnel to VPN server. |
| `ApplyingNetworkConfig` | Helper is applying IP address, routes, and DNS to the tunnel device. |
| `OpeningPacketDevice` | Core is opening the packet device for data-plane I/O. |
| `Connected` | Fully operational. Packet pump running, heartbeat active. |
| `Reconnecting` | Connection lost; attempting to re-establish based on ReconnectPolicy. |
| `Disconnecting` | User or system initiated disconnect; tearing down gracefully. |
| `CleaningUp` | Running cleanup on helper (remove routes, DNS, adapter). |
| `Failed` | Terminal error. User must intervene (e.g., retry connect). |

---

## State Transition Diagram (状态转换图)

```
                          UserConnect
                    +------------------+
                    |                  v
               +--------+    PreparingHelper
               |  Idle  |         |
               +--------+    +----+----+
                    ^       |         |
                    |       v         v
              CleanupSucceeded  Authenticating
                    |         |         |
                    |         |         v
                    |    AuthFailed  ConnectingCstp
                    |         |         |
                    |         v         v
                    |       Failed  CstpConnected
                    |                   |
                    |                   v
                    |         ApplyingNetworkConfig
                    |                   |
                    |            +------+------+
                    |            |             |
                    |            v             v
                    |    NetworkConfigApplied  PacketDeviceFailed
                    |            |                    |
                    |            v                    v
                    |    OpeningPacketDevice        Failed
                    |            |
                    |            v
                    |       Connected
                    |            |
                    |     +------+------+
                    |     |      |      |
                    |     v      v      v
                    | TransportClosed  HelperLost  LeaseExpired
                    |     |      |      |
                    |     v      v      v
                    |  Reconnecting   Failed (if not recoverable)
                    |     |
                    |     +---> Authenticating (re-auth)
                    |     +---> ConnectingCstp (resume)
                    |     +---> PreparingHelper (helper lost)
                    |
                    |   UserDisconnect (from any state)
                    +--- Disconnecting
                              |
                              v
                         CleaningUp
                              |
                              v
                            Idle
```

### Detailed Transitions (详细转换规则)

**From Idle:**
- `UserConnect` -> `PreparingHelper` (if `intent.desired_connected == true`)

**From PreparingHelper:**
- `HelperReady` -> `Authenticating`
- Error -> `Failed` (helper unavailable)

**From Authenticating:**
- `AuthSucceeded` -> `ConnectingCstp`
- `AuthFailed` -> `Failed`

**From ConnectingCstp:**
- `CstpConnected` -> `ApplyingNetworkConfig`
- `TransportClosed` -> `Reconnecting` (if recoverable + auto_reconnect) or `Failed`

**From ApplyingNetworkConfig:**
- `NetworkConfigApplied` -> `OpeningPacketDevice`
- Error -> `Failed` (OS config error, not retried blindly)

**From OpeningPacketDevice:**
- `PacketLoopStarted` -> `Connected`
- `PacketDeviceFailed` -> `Failed`

**From Connected:**
- `TransportClosed` -> `Reconnecting` (if auto_reconnect) or `Disconnecting`
- `HelperLost` -> `Reconnecting` (try recover helper) or `Failed`
- `LeaseExpired` -> `Failed`

**From Reconnecting:**
- `ReconnectTimerFired` -> `PreparingHelper` or `Authenticating` or `ConnectingCstp` (depending on what failed)
- `UserDisconnect` -> `Disconnecting`
- Max attempts exceeded -> `Failed`

**From Disconnecting:**
- Always -> `CleaningUp`

**From CleaningUp:**
- `CleanupSucceeded` -> `Idle`
- `CleanupFailed` -> `Idle` (log warning, best-effort cleanup)

**From Failed:**
- `UserConnect` -> `PreparingHelper` (user retries)

---

## Event Types and Effects (事件类型与效果)

| Event | Source | Effect |
|---|---|---|
| `UserConnect` | UI/RPC | Sets `desired_connected = true`, begins connection flow |
| `UserDisconnect` | UI/RPC | Sets `desired_connected = false`, initiates teardown |
| `SetAutoReconnect` | UI/RPC | Updates `auto_reconnect` flag in UserIntent |
| `HelperReady` | Helper IPC | Hello handshake completed, helper session active |
| `AuthSucceeded` | Protocol | Auth completed, cookie/session obtained |
| `AuthFailed` | Protocol | Auth rejected (bad credentials, cert, MFA failure) |
| `CstpConnected` | Protocol | CSTP tunnel established, metadata received |
| `NetworkConfigApplied` | Helper IP | IP/route/DNS written to OS |
| `PacketLoopStarted` | Data plane | Packet pump thread running |
| `TransportClosed` | Data plane | TLS/TCP connection dropped |
| `PacketDeviceFailed` | Data plane | TUN/TAP read/write error |
| `HelperLost` | Helper IPC | Helper process crashed or disconnected |
| `LeaseExpired` | Helper IPC | Session lease timed out (no heartbeat) |
| `ReconnectTimerFired` | Timer | Backoff delay elapsed, retry now |
| `CleanupSucceeded` | Helper IPC | Network artifacts removed |
| `CleanupFailed` | Helper IPC | Cleanup partially failed (logged, proceed to Idle) |

---

## Reconnect Policy Rules (重连策略规则)

The `ReconnectPolicy` receives structured `ErrorInfo` + `UserIntent` and produces a
`ReconnectDecision`. The policy is stateless per-call; the Core tracks attempt count.

| Scenario | auto_reconnect | Action |
|---|---|---|
| User initiated disconnect | any | No reconnect. Cleanup. |
| Auth failed / bad credentials | any | No reconnect. Failed(auth). |
| Certificate error | any | No reconnect. Failed(cert). |
| Transport closed (was Connected) | true | Reconnect. Exponential backoff. Keep helper session. |
| Transport closed (was Connected) | false | No reconnect. Cleanup. |
| Transport closed (was ConnectingCstp) | true | Reconnect. May need re-auth. |
| Packet device failure | any | No blind retry. Report OS config error. |
| Route/DNS apply failure | any | No blind retry. Report OS config error. |
| Helper lost (during Connected) | true | Try reconnect helper / reconcile session. Fail if impossible. |
| Helper lost (during ApplyingNetworkConfig) | any | No reconnect. Failed(helper_unavailable). |
| Lease expired | any | No reconnect. Failed(lease_expired). |

### ReconnectDecision Output

```cpp
struct ReconnectDecision {
    bool should_retry;           // true = enter Reconnecting phase
    milliseconds delay;          // how long to wait before retry
    string reason_code;          // for logging/diagnostics
    bool keep_helper_session;    // true = reuse existing helper session
    bool keep_network_config;    // true = don't Cleanup, keep routes/DNS
};
```

---

## Backoff Configuration (退避配置)

| Parameter | Default | Description |
|---|---|---|
| `base_delay` | 1000 ms | Initial retry delay |
| `max_delay` | 60000 ms | Maximum retry delay cap |
| `jitter_factor` | 0.2 | Random jitter: +/-20% of computed delay |
| `stable_reset_duration` | 60 s | After this duration in Connected state, reset attempt counter |
| `max_attempts` | 0 (unlimited) | Maximum retry attempts before giving up. 0 = no limit. |

**Delay formula:**

```
delay = min(base_delay * 2^attempt, max_delay)
jittered_delay = delay * (1 + random(-jitter_factor, +jitter_factor))
```

**Reset conditions:**
- Connected for `stable_reset_duration` seconds: reset attempt counter to 0.
- User disconnect: reset attempt counter to 0.
- Transition to Failed: reset attempt counter to 0.

---

## UserIntent Semantics (用户意图语义)

```cpp
struct UserIntent {
    bool desired_connected = false;
    bool auto_reconnect = true;
    ProfileId profile_id;
    std::optional<DisconnectReason> user_disconnect_reason;
};
```

| Field | Meaning |
|---|---|
| `desired_connected` | User wants VPN to be active. `false` = disconnect immediately, never auto-reconnect. |
| `auto_reconnect` | If `true`, recoverable failures enter Reconnecting. If `false`, failures go to Failed. |
| `profile_id` | Which VPN profile/server to connect to. |
| `user_disconnect_reason` | Set when user initiates disconnect. Used for diagnostics. |

**Key semantics:**

- `desired_connected=false` overrides `auto_reconnect`. Even if auto_reconnect is true,
  the system will not reconnect if the user does not want to be connected.
- `auto_reconnect` only affects **unexpected** disconnections. User-initiated disconnect
  never triggers reconnection regardless of auto_reconnect.
- The UI sends `UserIntent`; the Core reads it. The UI does not compute retry logic.
