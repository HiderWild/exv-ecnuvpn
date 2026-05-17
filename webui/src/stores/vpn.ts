import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import api from '../api/desktop'
import { errorMessage } from '../utils/errors'

export interface VpnStatus {
  connected: boolean
  server: string
  username: string
  pid: number
  supervisor_pid: number
  network_ready: boolean
  interface: string
  internal_ip: string
  route_count: number
  mtu: number
  uptime_seconds: number
  rx_bytes: number
  tx_bytes: number
  upstream_virtual_detected: boolean
  upstream_virtual_adapters: UpstreamVirtualAdapter[]
  upstream_virtual_message: string
  route_policy: string
  mode?: 'helper' | 'direct' | 'elevated' | 'disconnected'
}

export interface UpstreamVirtualAdapter {
  name: string
  detail: string
}

export interface RouteEntry {
  cidr: string
}

export interface LogEntry {
  timestamp: string
  level: 'info' | 'warn' | 'error' | 'debug'
  message: string
}

export interface ServiceStatus {
  installed: boolean
  running: boolean
  path: string
  available: boolean
}

export type VpnErrorType =
  | 'elevation_required'
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
}

export function normalizeError(raw: unknown): VpnError {
  if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>
    // If the error has error_type, use it directly — no string matching needed
    if (obj.error_type && typeof obj.error_type === 'string') {
      return {
        ok: false,
        error_type: obj.error_type as VpnErrorType,
        message: String(obj.message || 'Operation failed'),
        recoverable: obj.recoverable !== undefined ? !!obj.recoverable : true,
        recommended_action: String(obj.recommended_action || ''),
      }
    }
    // If the error has ok:false with a message, wrap as native_failure
    if (obj.ok === false && obj.message) {
      return {
        ok: false,
        error_type: 'native_failure',
        message: String(obj.message),
        recoverable: true,
        recommended_action: 'Retry the operation',
      }
    }
  }
  // Final fallback for truly unexpected values — wrap as native_failure
  const msg = raw instanceof Error ? raw.message : (raw ? String(raw) : 'Unknown error')
  return { ok: false, error_type: 'native_failure', message: msg, recoverable: true, recommended_action: 'Retry the operation' }
}

export function isVpnError(data: unknown): data is VpnError {
  return data != null && typeof data === 'object' && 'error_type' in (data as object)
}

export type ConnectMode = 'helper' | 'elevated' | 'direct'

export const useVpnStore = defineStore('vpn', () => {
  const status = ref<VpnStatus | null>(null)
  const routes = ref<RouteEntry[]>([])
  const logs = ref<LogEntry[]>([])
  const serviceStatus = ref<ServiceStatus | null>(null)
  const loading = ref(false)
  const lastError = ref<string | null>(null)
  const lastErrorType = ref<VpnErrorType | null>(null)
  const lastRecoverable = ref(true)
  const lastRecommendedAction = ref('')

  // Derived state for service-first UX
  const serviceInstalled = computed(() => serviceStatus.value?.installed ?? false)
  const serviceRunning = computed(() => serviceStatus.value?.running ?? false)
  const canUseElevatedFallback = computed(() => {
    // Elevated fallback is available on Windows and macOS
    return typeof window !== 'undefined' && !!window.ecnuVpn &&
      (navigator.platform?.includes('Win') || navigator.platform?.includes('Mac'))
  })
  const recommendedConnectMode = computed<ConnectMode>(() => {
    if (serviceInstalled.value && serviceRunning.value) return 'helper'
    if (canUseElevatedFallback.value) return 'elevated'
    return 'helper'
  })
  const currentSessionMode = computed(() => {
    if (!status.value?.connected) return 'disconnected'
    return status.value.mode ?? 'helper'
  })

  function setError(err: VpnError) {
    lastError.value = err.message
    lastErrorType.value = err.error_type
    lastRecoverable.value = err.recoverable
    lastRecommendedAction.value = err.recommended_action
  }

  function clearError() {
    lastError.value = null
    lastErrorType.value = null
    lastRecoverable.value = true
    lastRecommendedAction.value = ''
  }

  async function fetchStatus() {
    try {
      const { data } = await api.get<VpnStatus>('/status')
      status.value = data
      if (data.connected) clearError()
    } catch (e) { console.error('[vpn] fetchStatus failed:', e) }
  }

  async function fetchAppShellState() {
    // Coordinate status + service status on page entry
    await Promise.allSettled([fetchStatus(), fetchServiceStatus()])
  }

  async function connect() {
    loading.value = true
    clearError()
    try {
      const { data } = await api.post<VpnStatus>('/connect')
      status.value = data
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function disconnect() {
    loading.value = true
    try {
      const { data } = await api.post<VpnStatus>('/disconnect')
      status.value = data
      clearError()
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function connectElevated() {
    loading.value = true
    clearError()
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/connect/elevated')
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = data
      }
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function disconnectElevated() {
    loading.value = true
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/disconnect/elevated')
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = data
        clearError()
      }
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function fetchRoutes() {
    try {
      const { data } = await api.get<RouteEntry[]>('/routes')
      routes.value = data
    } catch (e) { console.error('[vpn] fetchRoutes failed:', e) }
  }

  async function addRoute(cidr: string) {
    const { data } = await api.post<RouteEntry[]>('/routes', { cidr })
    routes.value = data
  }

  async function removeRoute(cidr: string) {
    const { data } = await api.delete<RouteEntry[]>('/routes', { data: { cidr } })
    routes.value = data
  }

  async function resetRoutes() {
    const { data } = await api.post<RouteEntry[]>('/routes/reset')
    routes.value = data
  }

  async function fetchServiceStatus() {
    try {
      const { data } = await api.get<ServiceStatus>('/service')
      serviceStatus.value = data
    } catch (e) { console.error('[vpn] fetchServiceStatus failed:', e) }
  }

  async function installService() {
    const { data } = await api.post<ServiceStatus>('/service/install')
    serviceStatus.value = data
  }

  async function uninstallService() {
    const { data } = await api.post<ServiceStatus>('/service/uninstall')
    serviceStatus.value = data
  }

  function addLog(entry: LogEntry) {
    logs.value.push(entry)
    if (logs.value.length > 1000) {
      logs.value = logs.value.slice(-1000)
    }
  }

  function clearLogs() {
    logs.value = []
  }

  function setLogs(entries: LogEntry[]) {
    logs.value = entries
  }

  return {
    status, loading, routes, logs, serviceStatus, lastError, lastErrorType,
    lastRecoverable, lastRecommendedAction,
    serviceInstalled, serviceRunning, canUseElevatedFallback,
    recommendedConnectMode, currentSessionMode,
    fetchStatus, fetchAppShellState, connect, disconnect, connectElevated, disconnectElevated,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, installService, uninstallService,
    addLog, clearLogs, setLogs,
  }
})
