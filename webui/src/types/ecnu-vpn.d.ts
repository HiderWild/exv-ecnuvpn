import type {
  DesktopDriverInstallTarget,
  DesktopEventType,
  DesktopModalPayload,
  DesktopServiceInstallPromptResult,
  DesktopWindowMode,
} from '../../host/shared/host-contract'
import type {
  AuthConfig,
  DriverStatus,
  KeyStatus,
  RuntimeStatus,
  SettingsConfig,
} from '../stores/config'
import type {
  CliInstallStatus,
  LogEntry,
  RouteEntry,
  ServiceStatus,
  ServiceProgressEntry,
  AuthInteractionPollResponse,
  VpnStatus,
} from '../stores/vpn'

export interface EcnuVpnEvent {
  type: DesktopEventType
  data: unknown | ServiceProgressEntry
}

export type VpnErrorType =
  | 'elevation_required'
  | 'elevation_cancelled'
  | 'elevation_denied'
  | 'runtime_missing'
  | 'config_invalid'
  | 'service_missing'
  | 'auth_protocol_mismatch'
  | 'auth_failed'
  | 'auth_rejected'
  | 'auth_challenge_required'
  | 'auth_group_required'
  | 'auth_expired'
  | 'csd_required_unsupported'
  | 'tls_verify_failed'
  | 'wintun_missing'
  | 'utun_permission_denied'
  | 'dtls_unavailable'
  | 'unsupported_dtls'
  | 'session_timeout'
  | 'idle_timeout'
  | 'unsupported_extra_args'
  | 'permission_denied'
  | 'helper_unavailable'
  | 'network_unreachable'
  | 'user_cancelled'
  | 'invalid_request'
  | 'connection_failed'
  | 'native_failure'
  | 'parse_failure'
  | 'unknown_action'

export interface VpnError {
  ok: false
  error_type: VpnErrorType
  message: string
  recoverable: boolean
  recommended_action: string
  timestamp?: number
}

export type ConnectMode = 'helper' | 'elevated' | 'direct'

export interface EcnuVpnApi {
  status: {
    get(): Promise<VpnStatus>
  }
  vpn: {
    connect(password?: string): Promise<VpnStatus | { status: 'connecting' }>
    disconnect(): Promise<VpnStatus | { status: 'disconnecting' }>
    connectElevated(password?: string): Promise<VpnStatus | VpnError>
    disconnectElevated(backend?: unknown): Promise<VpnStatus | VpnError>
    authInteraction(): Promise<AuthInteractionPollResponse>
    respondAuthInteraction(id: string, value: string): Promise<{ ok: true }>
  }
  config: {
    getAuth(): Promise<AuthConfig>
    saveAuth(input: Partial<AuthConfig>): Promise<AuthConfig>
    getSettings(): Promise<SettingsConfig>
    saveSettings(input: Partial<SettingsConfig>): Promise<SettingsConfig>
    getKey(): Promise<KeyStatus>
    importConfig(input: { format: 'protected' | 'unprotected'; data: string; password?: string }): Promise<{ ok: true }>
    exportConfig(input: { protected: boolean; password?: string }): Promise<{ format: 'protected' | 'unprotected'; data: string }>
    reset(confirm: boolean): Promise<{ ok: true }>
  }
  key: {
    reset(confirm: boolean): Promise<{ ok: true }>
  }
  maintenance: {
    inspectCore(): Promise<{ state: string; risk: 'unknown' | 'low' | 'medium' | 'high'; pid?: number; ipc_path?: string }>
    killStaleCore(confirm: boolean): Promise<{ ok: true }>
  }
  routes: {
    list(): Promise<RouteEntry[]>
    add(cidr: string): Promise<RouteEntry[]>
    remove(cidr: string): Promise<RouteEntry[]>
    reset(): Promise<RouteEntry[]>
  }
  service: {
    status(): Promise<ServiceStatus>
    install(): Promise<ServiceStatus>
    uninstall(): Promise<ServiceStatus>
  }
  cli: {
    status(): Promise<CliInstallStatus>
    install(): Promise<CliInstallStatus>
    uninstall(): Promise<CliInstallStatus>
  }
  logs: {
    list(options?: { lines?: number; filter?: string }): Promise<LogEntry[]>
  }
  runtime: {
    status(): Promise<RuntimeStatus>
  }
  drivers: {
    status(): Promise<DriverStatus>
    install(driver: DesktopDriverInstallTarget): Promise<DriverStatus>
  }
  window: {
    setMode(mode: DesktopWindowMode): Promise<{ ok: true; mode: DesktopWindowMode }>
    resolveClosePrompt(result: unknown): Promise<{ ok: true }>
  }
  modal: {
    serviceInstallPrompt(): Promise<DesktopServiceInstallPromptResult>
    passwordPrompt(message: string): Promise<string | null>
    confirmPrompt(message: string): Promise<boolean>
    getPayload(): Promise<DesktopModalPayload | null>
    resolve(result: unknown): Promise<{ ok: true }>
  }
  events: {
    subscribe(handler: (event: EcnuVpnEvent) => void): () => void
  }
  core: {
    restart(): Promise<{ ok: true }>
    quit(): Promise<{ ok: true }>
  }
}

declare global {
  interface Window {
    ecnuVpn?: EcnuVpnApi
  }
}
