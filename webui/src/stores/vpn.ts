import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import api from '../api/desktop'

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
  log_path?: string
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
  timestamp?: number
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
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : undefined,
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
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : undefined,
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

export type DashboardState =
  | 'service-ready disconnected'
  | 'service-missing disconnected'
  | 'helper connected'
  | 'direct connected'
  | 'elevated connected'
  | 'elevated connecting'
  | 'error recoverable'
  | 'error blocking'
  | 'loading'

export interface DashboardAction {
  label: string
  action: () => void
  variant?: 'primary' | 'secondary' | 'destructive'
}

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
  const lastErrorTime = ref<number | null>(null)

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

  // ── Dashboard state machine ──────────────────────────────────────────
  const isDesktop = computed(() => {
    return typeof window !== 'undefined' && !!window.ecnuVpn
  })

  const dashboardState = computed<DashboardState>(() => {
    // Error states take priority when an error is present.
    // Note: elevation_required and service_missing are auto-cleared in setError(),
    // but we guard here as a safety net against direct ref mutation.
    if (lastError.value && lastErrorType.value) {
      if (lastErrorType.value === 'elevation_required' || lastErrorType.value === 'service_missing') {
        // These should not appear as error states — fall through to disconnected logic
      } else if (lastErrorType.value === 'runtime_missing') {
        return 'error blocking'
      } else {
        return lastRecoverable.value ? 'error recoverable' : 'error blocking'
      }
    }

    // Elevated connecting: loading and last action was elevated connect
    if (loading.value && lastActionWasElevatedConnect.value) {
      return 'elevated connecting'
    }

    // Connected states
    if (status.value?.connected) {
      const mode = currentSessionMode.value
      if (mode === 'helper') return 'helper connected'
      if (mode === 'elevated') return 'elevated connected'
      return 'direct connected'
    }

    // Disconnected states
    if (serviceInstalled.value && serviceRunning.value) {
      return 'service-ready disconnected'
    }
    return 'service-missing disconnected'
  })

  // Track whether the last action was an elevated connect (for the "connecting" state)
  const lastActionWasElevatedConnect = ref(false)

  const dashboardPrimaryAction = computed<DashboardAction | null>(() => {
    const state = dashboardState.value
    switch (state) {
      case 'service-ready disconnected':
        return { label: '连接', action: () => connect(), variant: 'primary' }
      case 'service-missing disconnected':
        return { label: '安装服务（推荐）', action: () => { /* navigated via router */ }, variant: 'primary' }
      case 'helper connected':
        return { label: '断开连接', action: () => disconnect(), variant: 'destructive' }
      case 'direct connected':
      case 'elevated connected':
        return { label: '断开连接', action: () => disconnectElevated(), variant: 'destructive' }
      case 'elevated connecting':
        return null // No CTA — just show spinner
      case 'error recoverable':
        return recoverableErrorAction.value
      case 'error blocking':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  const dashboardSecondaryAction = computed<DashboardAction | null>(() => {
    const state = dashboardState.value
    switch (state) {
      case 'service-missing disconnected':
        if (canUseElevatedFallback.value) {
          return { label: '仅本次连接', action: () => connectElevated(), variant: 'secondary' }
        }
        return null
      case 'direct connected':
      case 'elevated connected':
        return { label: '安装服务', action: () => { /* navigated via router */ }, variant: 'secondary' }
      case 'error recoverable':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  // Maps error types to specific recovery actions
  const recoverableErrorAction = computed<DashboardAction | null>(() => {
    if (!lastErrorType.value) return null
    switch (lastErrorType.value) {
      case 'elevation_denied':
        return { label: '安装服务', action: () => { /* navigated via router */ }, variant: 'primary' }
      case 'config_invalid':
        return { label: '前往设置', action: () => { /* navigated via router */ }, variant: 'primary' }
      case 'native_failure':
      case 'parse_failure':
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
      default:
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
    }
  })

  // Track the last mutating action for retry
  const _lastMutatingAction = ref<(() => Promise<void>) | null>(null)

  function retryLastAction() {
    clearError()
    if (_lastMutatingAction.value) {
      _lastMutatingAction.value()
    }
  }

  function setError(err: VpnError) {
    // elevation_required and service_missing are not error states — they
    // indicate the user should be in the service-missing flow instead.
    // Auto-clear them so the dashboard falls through to actionable CTAs.
    if (err.error_type === 'elevation_required' || err.error_type === 'service_missing') {
      clearError()
      return
    }
    lastError.value = err.message
    lastErrorType.value = err.error_type
    lastRecoverable.value = err.recoverable
    lastRecommendedAction.value = err.recommended_action
    lastErrorTime.value = Date.now()
  }

  function clearError() {
    lastError.value = null
    lastErrorType.value = null
    lastRecoverable.value = true
    lastRecommendedAction.value = ''
    lastErrorTime.value = null
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
    lastActionWasElevatedConnect.value = false
    _lastMutatingAction.value = connect
    try {
      const { data } = await api.post<VpnStatus>('/connect')
      status.value = data
      await fetchAppShellState()
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function disconnect() {
    loading.value = true
    lastActionWasElevatedConnect.value = false
    _lastMutatingAction.value = disconnect
    try {
      const { data } = await api.post<VpnStatus>('/disconnect')
      status.value = data
      clearError()
      await fetchAppShellState()
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      loading.value = false
    }
  }

  async function connectElevated() {
    loading.value = true
    clearError()
    lastActionWasElevatedConnect.value = true
    _lastMutatingAction.value = connectElevated
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/connect/elevated')
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = data
        await fetchAppShellState()
      }
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      lastActionWasElevatedConnect.value = false
      loading.value = false
    }
  }

  async function disconnectElevated() {
    loading.value = true
    lastActionWasElevatedConnect.value = false
    _lastMutatingAction.value = disconnectElevated
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/disconnect/elevated')
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = data
        clearError()
        await fetchAppShellState()
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
    await fetchAppShellState()
  }

  async function uninstallService() {
    const { data } = await api.post<ServiceStatus>('/service/uninstall')
    serviceStatus.value = data
    await fetchAppShellState()
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
    lastRecoverable, lastRecommendedAction, lastErrorTime,
    serviceInstalled, serviceRunning, canUseElevatedFallback,
    recommendedConnectMode, currentSessionMode,
    isDesktop, dashboardState, dashboardPrimaryAction, dashboardSecondaryAction,
    fetchStatus, fetchAppShellState, connect, disconnect, connectElevated, disconnectElevated,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, installService, uninstallService,
    addLog, clearLogs, setLogs, clearError, retryLastAction,
  }
})
