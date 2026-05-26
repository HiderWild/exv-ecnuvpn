import type {
  DesktopDriverInstallTarget,
  DesktopEventType,
} from '../../desktop/shared/desktop-contract'
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
  }
  config: {
    getAuth(): Promise<AuthConfig>
    saveAuth(input: Partial<AuthConfig>): Promise<AuthConfig>
    getSettings(): Promise<SettingsConfig>
    saveSettings(input: Partial<SettingsConfig>): Promise<SettingsConfig>
    getKey(): Promise<KeyStatus>
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
  events: {
    subscribe(handler: (event: EcnuVpnEvent) => void): () => void
  }
}

declare global {
  interface Window {
    ecnuVpn?: EcnuVpnApi
  }
}
