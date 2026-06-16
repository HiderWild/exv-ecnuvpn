// Generated from contracts/system.contract.json. Do not edit manually.

export const CONTRACT_VERSION = "2026-06-15.config-helper-tunnel-utils-contract.v1" as const

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
  "service.install",
  "service.uninstall",
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

export const HELPER_OPS = [
  "Hello",
  "StartSession",
  "PrepareTunnelDevice",
  "ApplyTunnelConfig",
  "Heartbeat",
  "Cleanup",
  "GetSnapshot",
  "Shutdown",
  "Inspect",
  "AcquireCoreLease",
  "KeepAlive",
  "ReleaseCoreLease",
  "InstallService",
  "UninstallService",
  "ExportCleanupLease",
  "HandoffSession",
  "FinalizeHandoff"
] as const
export const HELPER_OP_CONTRACTS = [
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
    "name": "Shutdown",
    "code": 8,
    "request": "ShutdownRequest",
    "response": "ShutdownResponse",
    "requires_session": true
  },
  {
    "name": "Inspect",
    "code": 9,
    "request": "InspectRequest",
    "response": "InspectResponse",
    "requires_session": false
  },
  {
    "name": "AcquireCoreLease",
    "code": 10,
    "request": "AcquireCoreLeaseRequest",
    "response": "AcquireCoreLeaseResponse",
    "requires_session": false
  },
  {
    "name": "KeepAlive",
    "code": 11,
    "request": "KeepAliveRequest",
    "response": "KeepAliveResponse",
    "requires_session": false
  },
  {
    "name": "ReleaseCoreLease",
    "code": 12,
    "request": "ReleaseCoreLeaseRequest",
    "response": "ReleaseCoreLeaseResponse",
    "requires_session": false
  },
  {
    "name": "InstallService",
    "code": 13,
    "request": "InstallServiceRequest",
    "response": "InstallServiceResponse",
    "requires_session": false
  },
  {
    "name": "UninstallService",
    "code": 14,
    "request": "UninstallServiceRequest",
    "response": "UninstallServiceResponse",
    "requires_session": false
  },
  {
    "name": "ExportCleanupLease",
    "code": 15,
    "request": "ExportCleanupLeaseRequest",
    "response": "ExportCleanupLeaseResponse",
    "requires_session": false
  },
  {
    "name": "HandoffSession",
    "code": 16,
    "request": "HandoffSessionRequest",
    "response": "HandoffSessionResponse",
    "requires_session": false
  },
  {
    "name": "FinalizeHandoff",
    "code": 17,
    "request": "FinalizeHandoffRequest",
    "response": "FinalizeHandoffResponse",
    "requires_session": false
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

export const TUNNEL_PHASE_CONTRACTS = [
  {
    "name": "Idle",
    "wire_name": "idle",
    "running": false,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "PreparingHelper",
    "wire_name": "preparing_helper",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "Authenticating",
    "wire_name": "authenticating",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "ConnectingCstp",
    "wire_name": "connecting_cstp",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "ApplyingNetworkConfig",
    "wire_name": "applying_network_config",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "OpeningPacketDevice",
    "wire_name": "opening_packet_device",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "Connected",
    "wire_name": "connected",
    "running": true,
    "connected": true,
    "network_ready": true
  },
  {
    "name": "Reconnecting",
    "wire_name": "reconnecting",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "Disconnecting",
    "wire_name": "disconnecting",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "CleaningUp",
    "wire_name": "cleaning_up",
    "running": true,
    "connected": false,
    "network_ready": false
  },
  {
    "name": "Failed",
    "wire_name": "failed",
    "running": false,
    "connected": false,
    "network_ready": false
  }
] as const
export const TUNNEL_EVENTS = [
  "UserConnect",
  "UserDisconnect",
  "SetAutoReconnect",
  "HelperReady",
  "AuthSucceeded",
  "AuthFailed",
  "CstpConnected",
  "NetworkConfigApplied",
  "PacketLoopStarted",
  "TransportClosed",
  "PacketDeviceFailed",
  "HelperLost",
  "LeaseExpired",
  "ReconnectTimerFired",
  "CleanupSucceeded",
  "CleanupFailed"
] as const
export const TUNNEL_DISCONNECT_REASONS = [
  "UserRequested",
  "AuthFailed",
  "CertError",
  "TransportClosed",
  "HelperLost",
  "PacketDeviceFailed",
  "NetworkConfigFailed",
  "LeaseExpired"
] as const
export const TUNNEL_ERROR_DOMAINS = [
  "transport",
  "auth",
  "helper",
  "os.route",
  "os.dns",
  "packet",
  "config",
  "native"
] as const
export const TUNNEL_STATUS_FIELDS = [
  "phase",
  "desired_connected",
  "auto_reconnect",
  "helper_mode",
  "helper_status",
  "network_ready",
  "server",
  "interface_name",
  "last_error",
  "reconnect"
] as const
export const SRC_ALLOWED_TOP_LEVEL_DIRS = [
  "app",
  "base",
  "cli",
  "common",
  "contracts",
  "core",
  "feedback",
  "helper",
  "observability",
  "platform",
  "runtime",
  "utils",
  "vpn_engine"
] as const
export const SRC_FORBIDDEN_PATTERNS = [
  "src/*.hpp",
  "src/*.cpp",
  "src/core_api",
  "*.inc.cpp",
  "#include \"*.inc.cpp\"",
  "src/webui_assets.hpp",
  "platform include logger.hpp",
  "platform include vpn.hpp",
  "platform include tunnel.hpp",
  "platform include openconnect_log.hpp",
  "platform include virtual_network.hpp",
  "platform include core/*"
] as const

export type DesktopRpcAction = (typeof DESKTOP_RPC_ACTIONS)[number]
export type ConfigAction = (typeof CONFIG_ACTIONS)[number]
export type HelperOp = (typeof HELPER_OPS)[number]
export type TunnelPhase = (typeof TUNNEL_PHASE_CONTRACTS)[number]['name']
export type TunnelEvent = (typeof TUNNEL_EVENTS)[number]
