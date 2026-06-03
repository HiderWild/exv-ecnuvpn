# Desktop RPC V2 Contract Specification

> Version: Draft 1
> Date: 2026-06-02
> Status: Phase 0 Interface Freeze

---

## Overview (概述)

The Desktop RPC V2 contract defines the interface between the UI layer (Electron,
WebView, or CLI) and the Core process. The UI sends **action requests** and receives
**status responses**. The UI does not compute retry logic, manage helper sessions, or
perform privileged operations.

---

## Action Naming Convention (动作命名规范)

Actions follow the pattern `<domain>.<verb>`:

| Domain | Actions | Description |
|---|---|---|
| `vpn` | `connect`, `disconnect`, `get_status` | VPN connection lifecycle |
| `config` | `get_profiles`, `get_profile`, `save_profile`, `delete_profile` | Profile management |
| `service` | `get_helper_status`, `install_helper`, `uninstall_helper` | Helper service management |
| `route` | `get_routes`, `flush_routes` | Route table inspection |

---

## vpn.connect (连接请求)

**Request:**

```json
{
  "action": "vpn.connect",
  "request_id": "ui-001",
  "profile_id": "default",
  "intent": {
    "desired_connected": true,
    "auto_reconnect": true
  }
}
```

**Key change from V1:** The `intent` object replaces the old `retry_limit` integer.
The UI expresses *what* the user wants, not *how* to achieve it. The Core computes
retry strategy from `UserIntent` + `ReconnectPolicy`.

**Old V1 format (deprecated):**

```json
{
  "action": "vpn.connect",
  "profile_id": "default",
  "retry_limit": 5,
  "auto_reconnect": true
}
```

**Response (immediate acknowledgment):**

```json
{
  "action": "vpn.connect",
  "request_id": "ui-001",
  "ok": true,
  "error": null
}
```

The connect action returns immediately. Status updates are delivered asynchronously
via the status subscription mechanism.

---

## vpn.disconnect (断开请求)

**Request:**

```json
{
  "action": "vpn.disconnect",
  "request_id": "ui-002",
  "reason": "user_requested"
}
```

**Response:**

```json
{
  "action": "vpn.disconnect",
  "request_id": "ui-002",
  "ok": true,
  "error": null
}
```

---

## vpn.get_status (获取状态)

**Request:**

```json
{
  "action": "vpn.get_status",
  "request_id": "ui-003"
}
```

**Response:**

```json
{
  "action": "vpn.get_status",
  "request_id": "ui-003",
  "ok": true,
  "status": {
    "phase": "Connected",
    "desired_connected": true,
    "auto_reconnect": true,
    "helper_mode": "transient",
    "helper_status": "connected",
    "network_ready": true,
    "server": "vpn.example.com",
    "interface_name": "ECNUTun",
    "last_error": null,
    "reconnect": null
  },
  "error": null
}
```

### Status Fields (状态字段)

| Field | Type | Description |
|---|---|---|
| `phase` | string | Current `TunnelPhase` (see CORE_STATE_MACHINE.md) |
| `desired_connected` | bool | Whether user wants to be connected |
| `auto_reconnect` | bool | Whether auto-reconnect is enabled |
| `helper_mode` | string | `"transient"` or `"resident"` |
| `helper_status` | string | `"connected"`, `"unavailable"`, or `"version_mismatch"` |
| `network_ready` | bool | Whether OS network config is applied |
| `server` | string | VPN server hostname |
| `interface_name` | string | Tunnel interface name |
| `last_error` | object or null | Most recent error (see Error Object) |
| `reconnect` | object or null | Reconnect info when in Reconnecting phase |

### Reconnect Info (重连信息)

Present only when `phase == "Reconnecting"`:

```json
{
  "attempt": 3,
  "next_retry_ms": 4000
}
```

---

## Error Object Format (错误对象格式)

```json
{
  "domain": "transport",
  "code": "transport_closed",
  "message": "TLS connection dropped by server",
  "native_code": 10054,
  "native_api": "WSARecv",
  "recoverable": true,
  "recommended_action": "retry"
}
```

| Field | Type | Description |
|---|---|---|
| `domain` | string | Error category: `transport`, `auth`, `helper`, `os.route`, `os.dns`, `packet` |
| `code` | string | Machine-readable error code |
| `message` | string | Human-readable description |
| `native_code` | int or null | OS-specific error code |
| `native_api` | string or null | OS API that failed |
| `recoverable` | bool | Whether retry may succeed |
| `recommended_action` | string | `"retry"`, `"reconnect_helper"`, `"report"`, `"none"` |

### Error Domains and Codes (错误域与代码)

| Domain | Code | Recoverable | Description |
|---|---|---|---|
| `transport` | `transport_closed` | true | TLS/TCP connection dropped |
| `transport` | `dns_resolve_failed` | true | Cannot resolve server hostname |
| `transport` | `connection_refused` | true | Server rejected connection |
| `auth` | `auth_failed` | false | Bad credentials |
| `auth` | `cert_error` | false | Certificate validation failed |
| `auth` | `mfa_timeout` | true | MFA challenge timed out |
| `helper` | `helper_unavailable` | false | Cannot connect to helper |
| `helper` | `helper_version_mismatch` | false | Helper protocol version mismatch |
| `helper` | `session_lease_expired` | false | Helper session timed out |
| `os.route` | `route_apply_failed` | false | OS route table write failed |
| `os.dns` | `dns_apply_failed` | false | OS DNS config write failed |
| `packet` | `device_open_failed` | false | Cannot open TUN/TAP device |
| `packet` | `device_io_error` | true | TUN/TAP read/write error |

---

## Status Subscription (状态订阅)

The UI subscribes to status updates. The Core pushes a full `TunnelStatusSnapshot`
whenever the phase changes or an error occurs.

**Subscription request (WebSocket or IPC):**

```json
{
  "action": "vpn.subscribe_status",
  "request_id": "ui-010"
}
```

**Pushed updates:**

```json
{
  "event": "vpn.status_changed",
  "status": {
    "phase": "Reconnecting",
    "desired_connected": true,
    "auto_reconnect": true,
    "helper_mode": "transient",
    "helper_status": "connected",
    "network_ready": false,
    "server": "vpn.example.com",
    "interface_name": "",
    "last_error": {
      "domain": "transport",
      "code": "transport_closed",
      "message": "Connection reset by peer",
      "native_code": 10054,
      "native_api": "WSARecv",
      "recoverable": true,
      "recommended_action": "retry"
    },
    "reconnect": {
      "attempt": 2,
      "next_retry_ms": 2000
    }
  }
}
```

---

## Migration Notes from V1 (V1 迁移说明)

### Breaking Changes

1. **`retry_limit` removed.** V1 accepted `retry_limit` in connect requests. V2
   replaces this with `intent.auto_reconnect`. The Core computes retry limits
   internally via `ReconnectPolicy`.

2. **Status format changed.** V1 returned `state: "connected"` as a flat string.
   V2 returns a structured `status` object with `phase`, `helper_mode`,
   `last_error`, and `reconnect` fields.

3. **Error format changed.** V1 returned `error: "message"` as a plain string.
   V2 returns a structured error object with `domain`, `code`, `recoverable`,
   and `recommended_action`.

4. **`action` field preserved.** V2 keeps the `action` string for routing, but
   all payloads are now structured objects rather than flat key-value pairs.

### Deprecation Timeline

| V1 Feature | V2 Replacement | Removal Phase |
|---|---|---|
| `retry_limit` in connect | `intent.auto_reconnect` | Phase 1 |
| Flat `state` string | `status.phase` | Phase 1 |
| `error` plain string | `error` object | Phase 1 |
| `helper_mode` in connect | Implicit from helper install state | Phase 3 |

### Backward Compatibility

During migration (Phases 1-3), the Core API layer will accept both V1 and V2
formats. V1 requests are translated to V2 internally. V1 format will emit
deprecation warnings in logs.
