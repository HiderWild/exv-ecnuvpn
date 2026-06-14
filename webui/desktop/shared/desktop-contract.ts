import {
  DESKTOP_RPC_ACTIONS,
  DESKTOP_RPC_ERROR_CODE_MAP,
  DESKTOP_RPC_EVENT_TYPES,
} from './generated/system-contract.js'

export const desktopIpcChannels = {
  rpc: 'ecnu-vpn:rpc',
  rpcElevated: 'ecnu-vpn:rpc-elevated',
  serviceCommand: 'ecnu-vpn:service-command',
  cliCommand: 'ecnu-vpn:cli-command',
  driverInstall: 'ecnu-vpn:driver-install',
  windowMode: 'ecnu-vpn:window-mode',
  serviceInstallPrompt: 'ecnu-vpn:service-install-prompt',
  passwordPrompt: 'ecnu-vpn:password-prompt',
  confirmPrompt: 'ecnu-vpn:confirm-prompt',
  modalPayload: 'ecnu-vpn:modal-payload',
  modalResult: 'ecnu-vpn:modal-result',
  closePromptResult: 'ecnu-vpn:close-prompt-result',
  event: 'ecnu-vpn:event',
} as const

export const desktopApiPaths = {
  status: '/status',
  connect: '/connect',
  connectElevated: '/connect/elevated',
  disconnect: '/disconnect',
  disconnectElevated: '/disconnect/elevated',
  configAuth: '/config/auth',
  configSettings: '/config/settings',
  configKey: '/config/key',
  configKeyAlias: '/key',
  routes: '/routes',
  routesReset: '/routes/reset',
  service: '/service',
  serviceInstall: '/service/install',
  serviceUninstall: '/service/uninstall',
  cli: '/cli',
  cliInstall: '/cli/install',
  cliUninstall: '/cli/uninstall',
  runtime: '/runtime',
  drivers: '/drivers',
  driversInstall: '/drivers/install',
  logs: '/logs',
} as const

export const desktopRpcActions = DESKTOP_RPC_ACTIONS

export const desktopEventTypes = DESKTOP_RPC_EVENT_TYPES

export const desktopServiceCommands = ['install', 'uninstall'] as const
export const desktopCliCommands = ['status', 'install', 'uninstall'] as const
export const desktopDriverInstallTargets = ['wintun', 'tap'] as const

export const desktopRpcErrorCodes = DESKTOP_RPC_ERROR_CODE_MAP

export type DesktopRpcAction = (typeof desktopRpcActions)[number]
export type DesktopEventType = (typeof desktopEventTypes)[number]
export type DesktopServiceCommand = (typeof desktopServiceCommands)[number]
export type DesktopCliCommand = (typeof desktopCliCommands)[number]
export type DesktopDriverInstallTarget =
  (typeof desktopDriverInstallTargets)[number]
export type DesktopRpcErrorCode =
  (typeof desktopRpcErrorCodes)[keyof typeof desktopRpcErrorCodes]
export type DesktopWindowMode = 'minimal' | 'advanced'
export type DesktopServiceInstallPromptResult = 'install' | 'dismiss'
export type DesktopModalKind = 'service-install' | 'password' | 'confirm' | 'close-app'
export interface DesktopModalPayload {
  kind: DesktopModalKind
  message?: string
}

// =========================================================================
// RPC V2 Contract Types (Phase 4-6 hardening)
//
// These types mirror the C++ JSON payloads produced by core_api action
// handlers.  Field names and value conventions MUST stay in sync with:
//   - src/core/tunnel_state.hpp      (TunnelPhase, ErrorInfo, ReconnectInfo, TunnelStatusSnapshot)
//   - src/feedback/error_contract.hpp (ErrorInfo serialization)
//   - src/core_api/vpn_actions.cpp   (phase_to_string, status JSON)
//   - src/core_api/config_actions.cpp
//   - src/core_api/service_actions.cpp
//   - src/core_api/route_actions.cpp
// =========================================================================

// TunnelPhase values match the C++ phase_to_string() output in vpn_actions.cpp.
// NOTE: C++ outputs lowercase_with_underscores (e.g., "preparing_helper"),
// NOT PascalCase.  This is the canonical wire format.
export type TunnelPhase =
  | 'idle'
  | 'preparing_helper'
  | 'authenticating'
  | 'connecting_cstp'
  | 'applying_network_config'
  | 'opening_packet_device'
  | 'connected'
  | 'reconnecting'
  | 'disconnecting'
  | 'cleaning_up'
  | 'failed'

// HelperMode values match the helper_status field in TunnelStatusSnapshot.
export type HelperMode = 'transient' | 'resident' | 'unknown'

// ErrorInfo matches the C++ ErrorInfo struct serialized by vpn_actions.cpp.
export interface ErrorInfo {
  domain: string // transport|auth|helper|os.route|os.dns|packet|config
  code: string // Domain-specific error code
  message: string // Human-readable message
  native_code?: number // OS/protocol error code (optional)
  native_api?: string // Which API failed (optional, empty string omitted)
  recoverable: boolean // Should retry?
  recommended_action: string // What user should do
}

// ReconnectInfo matches the C++ ReconnectInfo struct in tunnel_state.hpp.
export interface ReconnectInfo {
  attempt: number
  next_retry_ms: number
}

// VpnStatusResponse matches the JSON produced by vpn_actions::status().
export interface VpnStatusResponse {
  phase: TunnelPhase
  desired_connected: boolean
  auto_reconnect: boolean
  helper_mode: string
  helper_status: string // connected|unavailable|version_mismatch
  network_ready: boolean
  server: string
  interface_name: string
  last_error?: ErrorInfo
  reconnect?: ReconnectInfo
}

// VpnConnectRequest matches the payload parsed by vpn_actions::connect().
export interface VpnConnectRequest {
  profile_id: string
  auto_reconnect: boolean
}

// VpnDisconnectRequest — vpn_actions::disconnect() takes no payload.
export interface VpnDisconnectRequest {
  reason?: string
}

// SetAutoReconnectRequest matches vpn_actions::set_auto_reconnect().
export interface SetAutoReconnectRequest {
  enabled: boolean
}

// CredentialStoreStatus — placeholder for future credential store integration.
export interface CredentialStoreStatus {
  available: boolean
  backend: string // windows-credential-manager|macos-keychain|unsupported
  error?: ErrorInfo
}

// HelperV2Status matches the stub returned by service_actions::helper_status().
export interface HelperV2Status {
  installed: boolean
  status: string
  version: string
}

// DriverStatus matches the stub returned by service_actions::driver_status().
export interface DriverStatus {
  installed: boolean
  status: string
}

// UserRoute matches the route objects returned by route_actions::list().
export interface UserRoute {
  destination: string
  gateway: string
  metric: number
  enabled: boolean
}

// ActionResult<T> — Generic action result envelope.
export interface ActionResult<T> {
  success: boolean
  data?: T
  error?: ErrorInfo
  request_id?: string
}

// RpcResponse matches the C++ RpcResponse struct in app_rpc_dispatcher.hpp.
export interface RpcResponse {
  success: boolean
  payload_json: string
  error_code: string
  error_message: string
  request_id: string
}

// VPN action name constants — match handler registrations in vpn_actions.cpp.
export const VPN_ACTIONS = {
  CONNECT: 'vpn.connect',
  DISCONNECT: 'vpn.disconnect',
  STATUS: 'vpn.status',
  SET_AUTO_RECONNECT: 'vpn.set_auto_reconnect',
} as const

// Config action name constants — match handler registrations in config_actions.cpp.
export const CONFIG_ACTIONS = {
  GET: 'config.get',
  SAVE: 'config.save',
  GET_PROFILE: 'config.get_profile',
  SAVE_PROFILE: 'config.save_profile',
} as const

// Service action name constants — match handler registrations in service_actions.cpp.
export const SERVICE_ACTIONS = {
  HELPER_STATUS: 'service.helper_status',
  INSTALL: 'service.install',
  UNINSTALL: 'service.uninstall',
  DRIVER_STATUS: 'service.driver_status',
} as const

// Route action name constants — match handler registrations in route_actions.cpp.
export const ROUTE_ACTIONS = {
  LIST: 'route.list',
  ADD: 'route.add',
  REMOVE: 'route.remove',
  ENABLE: 'route.enable',
  DISABLE: 'route.disable',
} as const
