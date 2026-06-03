# Helper Protocol V2 Specification

> Version: Draft 1
> Date: 2026-06-02
> Status: Phase 0 Interface Freeze

---

## Overview (概述)

Helper V2 is a **privileged control-plane only** protocol. The helper daemon performs
privileged OS operations (create/prepare tunnel devices, apply IP/route/DNS config,
cleanup network state) on behalf of the unprivileged Core process. The helper does
**not** parse AnyConnect protocol messages, does **not** forward IP packets, and does
**not** hold user credentials or session cookies.

Key design principles:

- Helper is an opaque privilege escalation proxy for network configuration.
- All VPN protocol logic (authentication, CSTP, reconnection) lives in the Core.
- Transient and Resident helper modes share the same API; only the lifecycle policy differs.
- Sessions are lease-based with heartbeat keepalive.

---

## Protocol Version and Capabilities (协议版本与能力)

| Field | Value |
|---|---|
| Protocol version | `2` |
| Wire format | JSON over named pipe (Windows), Unix socket (macOS/Linux), or XPC (macOS) |
| Message framing | Length-prefixed: `[4-byte uint32 length][JSON payload]` |
| Max message size | 1 MiB |

Capability negotiation happens at `Hello` time. The helper advertises:

```json
{
  "capabilities": [
    "tunnel_device_create",
    "route_apply",
    "dns_apply",
    "route_cleanup",
    "firewall_rule_apply"
  ]
}
```

Core must check capabilities before issuing operations.

---

## Message Format (消息格式)

Every request/response is a JSON object with at minimum:

```json
{
  "op": "<OperationName>",
  "request_id": "<opaque-string>"
}
```

Responses always include:

```json
{
  "op": "<OperationName>",
  "request_id": "<matching-id>",
  "ok": true,
  "error": null
}
```

On failure:

```json
{
  "op": "<OperationName>",
  "request_id": "<matching-id>",
  "ok": false,
  "error": {
    "code": "helper_unavailable",
    "message": "Detailed description"
  }
}
```

---

## Operations (操作定义)

### 1. Hello

Handshake and capability negotiation. Must be the first message after connection.

**Request:**

```json
{
  "op": "Hello",
  "request_id": "r-001",
  "protocol_version": 2
}
```

**Response:**

```json
{
  "op": "Hello",
  "request_id": "r-001",
  "ok": true,
  "server_version": 2,
  "capabilities": ["tunnel_device_create", "route_apply", "dns_apply", "route_cleanup"],
  "mode": "transient",
  "error": null
}
```

`mode` indicates whether this helper instance is `transient` (will exit after idle
timeout) or `resident` (stays running until explicitly stopped).

---

### 2. StartSession

Creates a new VPN session lease in the helper. The helper allocates a `session_id`.

**Request:**

```json
{
  "op": "StartSession",
  "request_id": "r-002",
  "profile_id": "default",
  "mode": "transient"
}
```

**Response:**

```json
{
  "op": "StartSession",
  "request_id": "r-002",
  "ok": true,
  "session_id": "sess-abc123",
  "error": null
}
```

---

### 3. PrepareTunnelDevice

Asks the helper to create or open a tunnel device (e.g., Wintun adapter on Windows,
utun on macOS).

**Request:**

```json
{
  "op": "PrepareTunnelDevice",
  "request_id": "r-003",
  "session_id": "sess-abc123",
  "adapter_name": "ECNUTun",
  "mtu": 1400
}
```

**Response:**

```json
{
  "op": "PrepareTunnelDevice",
  "request_id": "r-003",
  "ok": true,
  "device_path": "\\\\.\\DEVICE\\ECNUTun",
  "mtu": 1400,
  "error": null
}
```

---

### 4. ApplyTunnelConfig

Applies IP address, routes, and DNS configuration to the tunnel device. The config
is a structured object -- the helper writes it to the OS network stack but does not
parse VPN protocol messages.

**Request:**

```json
{
  "op": "ApplyTunnelConfig",
  "request_id": "r-004",
  "session_id": "sess-abc123",
  "config": {
    "ipv4_address": "10.0.0.2",
    "ipv4_netmask": "255.255.255.0",
    "ipv6_address": null,
    "dns_servers": ["10.0.0.1"],
    "dns_domains": ["vpn.example.com"],
    "routes": [
      { "destination": "10.0.0.0", "netmask": "255.0.0.0", "gateway": "10.0.0.1" }
    ],
    "split_tunnel": false
  }
}
```

**Response:**

```json
{
  "op": "ApplyTunnelConfig",
  "request_id": "r-004",
  "ok": true,
  "error": null
}
```

---

### 5. Heartbeat

Periodic keepalive. Core sends this while the session is active (Connected or
Reconnecting state). The helper uses heartbeat presence to detect stale sessions.

**Request:**

```json
{
  "op": "Heartbeat",
  "request_id": "r-005",
  "session_id": "sess-abc123",
  "core_phase": "Connected"
}
```

**Response:**

```json
{
  "op": "Heartbeat",
  "request_id": "r-005",
  "ok": true,
  "warning": null,
  "error": null
}
```

---

### 6. Cleanup

Removes all OS-level artifacts created for a session (routes, DNS, firewall rules,
adapter). This operation must be idempotent.

**Request:**

```json
{
  "op": "Cleanup",
  "request_id": "r-006",
  "session_id": "sess-abc123",
  "policy": {
    "remove_adapter": true,
    "remove_routes": true,
    "remove_dns": true,
    "remove_firewall_rules": true
  }
}
```

**Response:**

```json
{
  "op": "Cleanup",
  "request_id": "r-006",
  "ok": true,
  "routes_removed": 3,
  "dns_removed": true,
  "adapter_removed": true,
  "error": null
}
```

---

### 7. GetSnapshot

Returns the helper's current view of active sessions, OS state, and health. Used
for diagnostics and UI display.

**Request:**

```json
{
  "op": "GetSnapshot",
  "request_id": "r-007"
}
```

**Response:**

```json
{
  "op": "GetSnapshot",
  "request_id": "r-007",
  "ok": true,
  "active_sessions": [
    {
      "session_id": "sess-abc123",
      "profile_id": "default",
      "mode": "transient",
      "last_heartbeat": "2026-06-02T10:30:00Z",
      "core_phase": "Connected"
    }
  ],
  "helper_mode": "transient",
  "uptime_seconds": 120,
  "error": null
}
```

---

### 8. EndSession

Explicitly terminates a session lease. The helper should run cleanup for the session
if not already done.

**Request:**

```json
{
  "op": "EndSession",
  "request_id": "r-008",
  "session_id": "sess-abc123"
}
```

**Response:**

```json
{
  "op": "EndSession",
  "request_id": "r-008",
  "ok": true,
  "error": null
}
```

---

## Session Lifecycle Diagram (会话生命周期)

```
Core                           Helper
  |                               |
  |---[Hello]-------------------->|  Handshake, capability negotiation
  |<--[HelloResponse]-------------|
  |                               |
  |---[StartSession]------------->|  Create session lease
  |<--[StartSessionResponse]------|
  |                               |
  |---[PrepareTunnelDevice]------>|  Create/open TUN adapter
  |<--[PrepareTunnelDeviceResp]---|
  |                               |
  |---[ApplyTunnelConfig]-------->|  Configure IP/route/DNS
  |<--[ApplyTunnelConfigResp]-----|
  |                               |
  |   === Connected ===           |
  |                               |
  |---[Heartbeat]---------------->|  Periodic (every ~5s)
  |<--[HeartbeatResponse]---------|
  |   ...                         |
  |                               |
  |   === Disconnect or Error === |
  |                               |
  |---[Cleanup]------------------>|  Remove routes/DNS/adapter
  |<--[CleanupResponse]-----------|
  |                               |
  |---[EndSession]--------------->|  Release session lease
  |<--[EndSessionResponse]--------|
  |                               |
```

### Reconnecting State Behavior (重连状态行为)

When the Core enters `Reconnecting` phase:

1. Core **continues** sending Heartbeat messages with `core_phase: "Reconnecting"`.
2. Helper **does not** cleanup the session or remove network config.
3. Helper **does not** consider the session stale while heartbeats arrive.
4. If heartbeats stop beyond the lease timeout, helper cleans up autonomously.
5. Core decides when to give up reconnection; on giving up, it sends Cleanup explicitly.

---

## Lease Timeout Behavior (租约超时策略)

| Helper Mode | Heartbeat Timeout | After Cleanup | Idle Exit |
|---|---|---|---|
| Transient | 30 seconds | Cleanup session artifacts | Exit after 60s idle |
| Resident | 60 seconds | Cleanup session artifacts | Stay running |

When a session lease expires (no heartbeat received within timeout):

1. Helper marks the session as `stale`.
2. Helper runs cleanup for all OS artifacts associated with the stale session.
3. Helper removes the session from its registry.
4. For transient mode: if no sessions remain, start idle exit countdown.

---

## Security Constraints (安全约束)

### Forbidden (禁止)

The helper must **never**:

- Accept raw shell command strings for execution.
- Parse AnyConnect XML/HTML protocol responses.
- Receive or store user passwords, cookies, or authentication tokens.
- Forward individual IP packets (data plane).
- Run arbitrary executables specified by the Core.
- Write to paths outside its registered cleanup registry.

### Allowed (允许)

The helper **may**:

- Write structured network configuration (IP, routes, DNS) to the OS.
- Create, open, and destroy tunnel device adapters.
- Apply and remove firewall rules.
- Validate the calling process identity (PID, UID, named pipe ACL).
- Perform version and capability negotiation.
- Maintain a cleanup registry for crash recovery.

---

## Legacy Protocol Compatibility (旧协议兼容)

The V1 protocol used action-based messages:

```json
{ "action": "start", "auth_session": {...}, "retry_limit": 5 }
{ "action": "stop" }
```

During the migration period:

- V1 `start`/`stop` actions are supported only through `legacy_openconnect_adapter`.
- The adapter translates V1 actions into V2 operations internally.
- New code must use V2 operations exclusively.
- V1 actions will be removed in Phase 5 (native path switch).

The legacy adapter must be isolated in `src/legacy/` and must not leak V1 concepts
into the Core or helper_runtime modules.
