import type {
  AuthConfig,
  DriverStatus,
  KeyStatus,
  RuntimeStatus,
  SettingsConfig,
} from '../stores/config'
import type {
  LogEntry,
  RouteEntry,
  ServiceStatus,
  VpnStatus,
} from '../stores/vpn'

export interface EcnuVpnEvent {
  type: 'log' | 'status' | 'heartbeat'
  data: unknown
}

export interface EcnuVpnApi {
  status: {
    get(): Promise<VpnStatus>
  }
  vpn: {
    connect(password?: string): Promise<VpnStatus | { status: 'connecting' }>
    disconnect(): Promise<VpnStatus | { status: 'disconnecting' }>
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
  logs: {
    list(options?: { lines?: number; filter?: string }): Promise<LogEntry[]>
  }
  runtime: {
    status(): Promise<RuntimeStatus>
  }
  drivers: {
    status(): Promise<DriverStatus>
    install(driver: 'wintun' | 'tap'): Promise<DriverStatus>
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
