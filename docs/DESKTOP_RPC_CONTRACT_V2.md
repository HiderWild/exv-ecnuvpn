# Desktop RPC Contract V2 — TypeScript ↔ C++ Mapping

> Version: 2.0
> Date: 2026-06-02
> Status: Phase 4-6 Hardening

---

## Overview

This document defines the TypeScript types and their exact C++ counterparts for
the Desktop RPC V2 contract. All JSON field names, value conventions, and action
names MUST stay in sync between the UI layer (`webui/desktop/shared/desktop-contract.ts`)
and the Core layer (`src/core_api/`).

---

## TypeScript Types and C++ Counterparts

### TunnelPhase

| TypeScript value | C++ enum | C++ `phase_to_string()` output |
|---|---|---|
| `'idle'` | `TunnelPhase::Idle` | `"idle"` |
| `'preparing_helper'` | `TunnelPhase::PreparingHelper` | `"preparing_helper"` |
| `'authenticating'` | `TunnelPhase::Authenticating` | `"authenticating"` |
| `'connecting_cstp'` | `TunnelPhase::ConnectingCstp` | `"connecting_cstp"` |
| `'applying_network_config'` | `TunnelPhase::ApplyingNetworkConfig` | `"applying_network_config"` |
| `'opening_packet_device'` | `TunnelPhase::OpeningPacketDevice` | `"opening_packet_device"` |
| `'connected'` | `TunnelPhase::Connected` | `"connected"` |
| `'reconnecting'` | `TunnelPhase::Reconnecting` | `"reconnecting"` |
| `'disconnecting'` | `TunnelPhase::Disconnecting` | `"disconnecting"` |
| `'cleaning_up'` | `TunnelPhase::CleaningUp` | `"cleaning_up"` |
| `'failed'` | `TunnelPhase::Failed` | `"failed"` |

**Wire format:** lowercase with underscores. Defined in `src/core/tunnel_state.hpp`
and serialized by `vpn_actions.cpp::phase_to_string()`.

### ErrorInfo

| TypeScript field | C++ field | Type | Required |
|---|---|---|---|
| `domain` | `ErrorInfo::domain` | `string` | yes |
| `code` | `ErrorInfo::code` | `string` | yes |
| `message` | `ErrorInfo::message` | `string` | yes |
| `native_code` | `ErrorInfo::native_code` | `optional<int>` | no (omitted when absent) |
| `native_api` | `ErrorInfo::native_api` | `string` | no (omitted when empty) |
| `recoverable` | `ErrorInfo::recoverable` | `bool` | yes |
| `recommended_action` | `ErrorInfo::recommended_action` | `string` | yes |

**C++ sources:** `src/core/tunnel_state.hpp` (struct), `src/feedback/error_contract.hpp`
(serialization), `src/feedback/error_contract.cpp` (to_json/from_json).

### ReconnectInfo

| TypeScript field | C++ field | Type |
|---|---|---|
| `attempt` | `ReconnectInfo::attempt` | `int` |
| `next_retry_ms` | `ReconnectInfo::next_retry_ms` | `int` |

**C++ source:** `src/core/tunnel_state.hpp`. Only present in status response when
`phase == "reconnecting"`.

### VpnStatusResponse (maps to `TunnelStatusSnapshot`)

| TypeScript field | C++ field | Type | Notes |
|---|---|---|---|
| `phase` | `TunnelStatusSnapshot::phase` | `TunnelPhase` | Serialized as lowercase string |
| `desired_connected` | `TunnelStatusSnapshot::desired_connected` | `bool` | |
| `auto_reconnect` | `TunnelStatusSnapshot::auto_reconnect` | `bool` | |
| `helper_mode` | `TunnelStatusSnapshot::helper_mode` | `string` | `"transient"` or `"resident"` |
| `helper_status` | `TunnelStatusSnapshot::helper_status` | `string` | `"connected"`, `"unavailable"`, `"version_mismatch"` |
| `network_ready` | `TunnelStatusSnapshot::network_ready` | `bool` | |
| `server` | `TunnelStatusSnapshot::server` | `string` | |
| `interface_name` | `TunnelStatusSnapshot::interface_name` | `string` | |
| `last_error` | `TunnelStatusSnapshot::last_error` | `ErrorInfo?` | `null` when no error |
| `reconnect` | `TunnelStatusSnapshot::reconnect` | `ReconnectInfo?` | `null` when not reconnecting |

### HelperMode

| TypeScript value | C++ usage |
|---|---|
| `'transient'` | `helper::HelperMode::Transient` |
| `'resident'` | `helper::HelperMode::Resident` |
| `'unknown'` | Fallback / not connected |

### HelperV2Status (service.helper_status response)

| TypeScript field | C++ JSON field | Type |
|---|---|---|
| `installed` | `installed` | `bool` |
| `status` | `status` | `string` |
| `version` | `version` | `string` |

### DriverStatus (service.driver_status response)

| TypeScript field | C++ JSON field | Type |
|---|---|---|
| `installed` | `installed` | `bool` |
| `status` | `status` | `string` |

### UserRoute (route.list response items)

| TypeScript field | C++ field | Type |
|---|---|---|
| `destination` | `UserRoute::destination` | `string` |
| `gateway` | `UserRoute::gateway` | `string` |
| `metric` | `UserRoute::metric` | `int` (default 0) |
| `enabled` | `UserRoute::enabled` | `bool` (default true) |

---

## Action Request/Response Format

### vpn.connect

**Request payload:**
```json
{
  "profile_id": "default",
  "auto_reconnect": true
}
```

**Success response:**
```json
{
  "success": true,
  "payload_json": "{\"status\":\"connecting\"}",
  "error_code": "",
  "error_message": "",
  "request_id": "ui-001"
}
```

**Error response (invalid JSON):**
```json
{
  "success": false,
  "payload_json": "",
  "error_code": "invalid_payload",
  "error_message": "parse error description",
  "request_id": "ui-001"
}
```

### vpn.disconnect

**Request payload:** `{}` (no required fields)

**Success response:**
```json
{
  "success": true,
  "payload_json": "{\"status\":\"disconnecting\"}"
}
```

### vpn.status

**Request payload:** `{}`

**Success response:** Full `TunnelStatusSnapshot` as JSON (see VpnStatusResponse above).

### vpn.set_auto_reconnect

**Request payload:**
```json
{ "enabled": true }
```

**Success response:**
```json
{ "auto_reconnect": true }
```

### config.get

**Request payload:** `{}`

**Success response:**
```json
{ "config": {} }
```

### config.save

**Request payload:** Arbitrary config JSON.

**Success response:**
```json
{ "saved": true }
```

### config.get_profile

**Request payload:**
```json
{ "profile_id": "default" }
```

**Success response:**
```json
{ "profile_id": "default", "data": {} }
```

### config.save_profile

**Request payload:**
```json
{ "profile_id": "work", "data": { "server": "vpn.work.com" } }
```

**Success response:**
```json
{ "profile_id": "work", "saved": true }
```

### service.helper_status

**Request payload:** `{}`

**Success response:**
```json
{ "installed": false, "status": "unknown", "version": "" }
```

### service.install / service.uninstall

**Request payload:** `{}`

**Error response (not yet implemented):**
```json
{
  "success": false,
  "error_code": "not_implemented",
  "error_message": "Helper installation not yet implemented"
}
```

### service.driver_status

**Request payload:** `{}`

**Success response:**
```json
{ "installed": false, "status": "unknown" }
```

### route.list

**Request payload:** `{}`

**Success response:**
```json
{ "routes": [ { "destination": "...", "gateway": "...", "metric": 0, "enabled": true } ] }
```

### route.add

**Request payload:**
```json
{ "destination": "10.0.0.0/8", "gateway": "192.168.1.1", "metric": 100, "enabled": true }
```
`metric` and `enabled` are optional (default 0 and true).

**Success response:**
```json
{ "added": true }
```

### route.remove

**Request payload:**
```json
{ "destination": "10.0.0.0/8" }
```

**Success response:**
```json
{ "removed": true }
```

**Error response (not found):**
```json
{ "success": false, "error_code": "not_found", "error_message": "No route with destination: ..." }
```

### route.enable / route.disable

**Request payload:**
```json
{ "destination": "10.0.0.0/8" }
```

**Success response:**
```json
{ "enabled": true }  // or { "disabled": true }
```

---

## RpcResponse Envelope

All actions return an `RpcResponse` (defined in `src/core_api/app_rpc_dispatcher.hpp`):

| Field | Type | Description |
|---|---|---|
| `success` | `bool` | Whether the action succeeded |
| `payload_json` | `string` | JSON-encoded result data (on success) |
| `error_code` | `string` | Machine-readable error code (on failure) |
| `error_message` | `string` | Human-readable error description (on failure) |
| `request_id` | `string` | Echoed from the request for tracing |

---

## Error Handling

### Error Codes by Domain

| Domain | Error Code | Recoverable | Source |
|---|---|---|---|
| `transport` | `transport_closed` | yes | CoreErrorMapper |
| `transport` | `transport_timeout` | yes | CoreErrorMapper |
| `transport` | `tls_error` | no | CoreErrorMapper |
| `auth` | `auth_failed` | no | CoreErrorMapper |
| `auth` | `cert_error` | no | CoreErrorMapper |
| `auth` | `credential_expired` | no | CoreErrorMapper |
| `helper` | `helper_unavailable` | no | CoreErrorMapper |
| `helper` | `helper_version_mismatch` | no | CoreErrorMapper |
| `helper` | `helper_timeout` | yes | CoreErrorMapper |
| `os.route` | `route_failed` | no | CoreErrorMapper |
| `os.dns` | `dns_failed` | no | CoreErrorMapper |
| `packet` | `device_failed` | no | CoreErrorMapper |
| `packet` | `firewall_failed` | no | CoreErrorMapper |
| `config` | `invalid_config` | no | ConfigActions |
| `config` | `profile_not_found` | no | ConfigActions |
| (action) | `invalid_payload` | no | All actions (JSON parse error) |
| (action) | `unknown_action` | no | AppRpcDispatcher |
| (action) | `not_implemented` | no | Stub actions |

### Error Constants

Defined in `src/feedback/error_contract.hpp`:
- `exv::feedback::error_codes::*` — machine-readable error code strings
- `exv::feedback::error_domains::*` — domain name strings

---

## Legacy Compatibility

### V1 → V2 Migration

The V1 RPC actions listed in `desktopRpcActions` in `desktop-contract.ts` are
the legacy action names. The V2 action names (used by `core_api/`) differ:

| V1 action (desktop-contract.ts) | V2 action (core_api/) |
|---|---|
| `status.get` | `vpn.status` |
| `vpn.connect` | `vpn.connect` (same) |
| `vpn.disconnect` | `vpn.disconnect` (same) |
| `config.getAuth` | `config.get_profile` |
| `config.saveAuth` | `config.save_profile` |
| `config.getSettings` | `config.get` |
| `config.saveSettings` | `config.save` |
| `helper.status` | `service.helper_status` |
| `service.status` | `service.helper_status` |
| `routes.list` | `route.list` |
| `routes.add` | `route.add` |
| `routes.remove` | `route.remove` |
| `routes.reset` | (no V2 equivalent yet) |
| `drivers.status` | `service.driver_status` |
| `drivers.install` | `service.install` |

### Backward Compatibility Layer

During migration, the UI may translate V1 action names to V2 before sending
to the Core. The translation layer lives in the desktop/Electron shell, not
in the Core.

---

## TypeScript ↔ C++ Sync Rules

1. **Phase strings** use `lowercase_with_underscores` (not PascalCase).
   C++ source: `vpn_actions.cpp::phase_to_string()`.
2. **Action names** use `domain.verb` format matching C++ handler registrations.
3. **Optional fields** (`native_code`, `native_api`, `last_error`, `reconnect`)
   are omitted from JSON when not set, not sent as `null`.
4. **Error codes** are machine-readable strings from `error_contract.hpp`.
5. **All JSON field names** use `snake_case` (matching C++ `nlohmann::json` defaults).

---

## File Index

| File | Purpose |
|---|---|
| `webui/desktop/shared/desktop-contract.ts` | TypeScript type definitions and action constants |
| `src/core_api/app_rpc_dispatcher.hpp` | RpcRequest/RpcResponse structs, AppRpcDispatcher |
| `src/core_api/vpn_actions.hpp/.cpp` | VPN action handlers |
| `src/core_api/config_actions.hpp/.cpp` | Config action handlers |
| `src/core_api/service_actions.hpp/.cpp` | Service action handlers |
| `src/core_api/route_actions.hpp/.cpp` | Route action handlers |
| `src/core_api/core_api_setup.hpp/.cpp` | Dispatcher factory |
| `src/core/tunnel_state.hpp` | TunnelPhase, ErrorInfo, ReconnectInfo, TunnelStatusSnapshot |
| `src/core/tunnel_intent.hpp` | UserIntent, ProfileId |
| `src/feedback/error_contract.hpp/.cpp` | ErrorInfo serialization, error code/domain constants |
| `tests/core_api/vpn_actions_test.cpp` | VPN action unit tests |
| `tests/core_api/config_actions_test.cpp` | Config action unit tests |
| `tests/core_api/service_actions_test.cpp` | Service action unit tests |
| `tests/core_api/route_actions_test.cpp` | Route action unit tests |
| `tests/core_api/error_contract_test.cpp` | ErrorInfo serialization tests |
