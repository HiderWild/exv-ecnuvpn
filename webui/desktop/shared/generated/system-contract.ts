// Generated from contracts/system.contract.json. Do not edit manually.

export const CONTRACT_VERSION = "2026-06-14.config-helper-contract.v1" as const

export const DESKTOP_RPC_REQUEST_FIELDS = [
  "id",
  "action",
  "payload"
] as const
export const DESKTOP_RPC_RESPONSE_FIELDS = [
  "ok",
  "data",
  "code",
  "message",
  "event"
] as const
export const CORE_RPC_REQUEST_FIELDS = [
  "action",
  "payload_json",
  "request_id"
] as const
export const CORE_RPC_RESPONSE_FIELDS = [
  "success",
  "payload_json",
  "error_code",
  "error_message",
  "request_id"
] as const

export const DESKTOP_RPC_ACTIONS = [
  "status.get",
  "vpn.connect",
  "vpn.disconnect",
  "config.getAuth",
  "config.saveAuth",
  "config.getSettings",
  "config.saveSettings",
  "config.getKey",
  "routes.list",
  "routes.add",
  "routes.remove",
  "routes.reset",
  "service.status",
  "helper.status",
  "runtime.status",
  "drivers.status",
  "drivers.install",
  "logs.list"
] as const
export const DESKTOP_RPC_EVENT_TYPES = [
  "log",
  "status",
  "heartbeat",
  "service-progress",
  "close-request",
  "core-crashed"
] as const
export const DESKTOP_RPC_ERROR_CODES = [
  "helper_unavailable",
  "service_not_installed",
  "service_installed_not_running",
  "service_start_failed",
  "oneshot_not_supported",
  "oneshot_elevation_denied",
  "helper_rpc_failed",
  "auth_failed",
  "tls_verify_failed",
  "wintun_missing",
  "utun_permission_denied",
  "unsupported_dtls",
  "permission_denied",
  "network_unreachable",
  "user_cancelled",
  "invalid_request",
  "connection_failed",
  "vpn_start_failed"
] as const
export const DESKTOP_RPC_ERROR_CODE_MAP = {
  "helperUnavailable": "helper_unavailable",
  "serviceNotInstalled": "service_not_installed",
  "serviceInstalledNotRunning": "service_installed_not_running",
  "serviceStartFailed": "service_start_failed",
  "oneshotNotSupported": "oneshot_not_supported",
  "oneshotElevationDenied": "oneshot_elevation_denied",
  "helperRpcFailed": "helper_rpc_failed",
  "authFailed": "auth_failed",
  "tlsVerifyFailed": "tls_verify_failed",
  "wintunMissing": "wintun_missing",
  "utunPermissionDenied": "utun_permission_denied",
  "unsupportedDtls": "unsupported_dtls",
  "permissionDenied": "permission_denied",
  "networkUnreachable": "network_unreachable",
  "userCancelled": "user_cancelled",
  "invalidRequest": "invalid_request",
  "connectionFailed": "connection_failed",
  "vpnStartFailed": "vpn_start_failed"
} as const

export const CONFIG_ACTIONS = [
  "config.getAuth",
  "config.saveAuth",
  "config.getSettings",
  "config.saveSettings",
  "config.getKey",
  "config.profile.get",
  "config.profile.save"
] as const
export const CONFIG_ALIASES = {
  "config.get": "config.getSettings",
  "config.save": "config.saveSettings",
  "config.get_profile": "config.profile.get",
  "config.save_profile": "config.profile.save"
} as const

export const HELPER_V2_OPS = [
  "Hello",
  "StartSession",
  "PrepareTunnelDevice",
  "ApplyTunnelConfig",
  "Heartbeat",
  "Cleanup",
  "GetSnapshot",
  "EndSession"
] as const
export const HELPER_V2_OP_CONTRACTS = [
  {
    "name": "Hello",
    "code": 1,
    "request": "HelloRequest",
    "response": "HelloResponse",
    "requires_session": false
  },
  {
    "name": "StartSession",
    "code": 2,
    "request": "StartSessionRequest",
    "response": "StartSessionResponse",
    "requires_session": false
  },
  {
    "name": "PrepareTunnelDevice",
    "code": 3,
    "request": "PrepareTunnelDeviceRequest",
    "response": "PrepareTunnelDeviceResponse",
    "requires_session": true
  },
  {
    "name": "ApplyTunnelConfig",
    "code": 4,
    "request": "ApplyTunnelConfigRequest",
    "response": "ApplyTunnelConfigResponse",
    "requires_session": true
  },
  {
    "name": "Heartbeat",
    "code": 5,
    "request": "HeartbeatRequest",
    "response": "HeartbeatResponse",
    "requires_session": true
  },
  {
    "name": "Cleanup",
    "code": 6,
    "request": "CleanupRequest",
    "response": "CleanupResponse",
    "requires_session": true
  },
  {
    "name": "GetSnapshot",
    "code": 7,
    "request": "GetSnapshotRequest",
    "response": "GetSnapshotResponse",
    "requires_session": false
  },
  {
    "name": "EndSession",
    "code": 8,
    "request": "EndSessionRequest",
    "response": "EndSessionResponse",
    "requires_session": true
  }
] as const
export const HELPER_FORBIDDEN_CREDENTIAL_FIELDS = [
  "password",
  "passwd",
  "cookie",
  "token",
  "secret",
  "credential",
  "auth_key",
  "auth_token",
  "session_cookie",
  "webvpn_cookie",
  "csrf_token",
  "bearer_token",
  "api_key",
  "apikey"
] as const

export type DesktopRpcAction = (typeof DESKTOP_RPC_ACTIONS)[number]
export type ConfigAction = (typeof CONFIG_ACTIONS)[number]
export type HelperV2Op = (typeof HELPER_V2_OPS)[number]
