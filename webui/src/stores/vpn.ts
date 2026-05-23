import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import api from '../api/desktop'

export interface UpstreamVirtualAdapter {
  name: string
  detail: string
}

export interface VpnStatus {
  connected: boolean
  process_running?: boolean
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
  backend?: unknown
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
  capabilities?: {
    service_mode?: boolean
    oneshot_mode?: boolean
    temporary_connect?: boolean
    direct_fallback?: boolean
    helper_binary?: boolean
  }
  mode?: string
  endpoint?: string
  label?: string
  binary_path?: string
  service_state?: number
  warning?: string
}

export interface ServiceProgressEntry {
  command: 'install' | 'uninstall'
  message: string
  timestamp: string
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

export function isVpnError(data: unknown): data is VpnError {
  return data != null && typeof data === 'object' && 'error_type' in (data as object)
}

export function normalizeError(raw: unknown): VpnError {
  if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>
    if (typeof obj.error_type === 'string') {
      return {
        ok: false,
        error_type: obj.error_type as VpnErrorType,
        message: String(obj.message || 'Operation failed'),
        recoverable: obj.recoverable !== undefined ? !!obj.recoverable : true,
        recommended_action: String(obj.recommended_action || ''),
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : undefined,
      }
    }
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

  const message = raw instanceof Error ? raw.message : raw ? String(raw) : 'Unknown error'
  return {
    ok: false,
    error_type: message.includes('elevation_denied') ? 'elevation_denied' : 'native_failure',
    message,
    recoverable: true,
    recommended_action: 'Retry the operation',
    timestamp: Date.now(),
  }
}

export const useVpnStore = defineStore('vpn', () => {
  const status = ref<VpnStatus | null>(null)
  const routes = ref<RouteEntry[]>([])
  const logs = ref<LogEntry[]>([])
  const serviceStatus = ref<ServiceStatus | null>(null)
  const serviceProgress = ref<ServiceProgressEntry[]>([])
  const serviceBusy = ref(false)
  const loading = ref(false)
  const lastError = ref<string | null>(null)
  const lastErrorType = ref<VpnErrorType | null>(null)
  const lastRecoverable = ref(true)
  const lastRecommendedAction = ref('')
  const lastErrorTime = ref<number | null>(null)
  const lastActionWasElevatedConnect = ref(false)
  const lastMutatingAction = ref<(() => Promise<void>) | null>(null)
  const activeTemporaryBackend = ref<unknown | null>(null)

  const serviceInstalled = computed(() => serviceStatus.value?.installed ?? false)
  const serviceRunning = computed(() => serviceStatus.value?.running ?? false)
  const isDesktop = computed(() => typeof window !== 'undefined' && !!window.ecnuVpn)
  const canUseElevatedFallback = computed(() => {
    const capabilities = serviceStatus.value?.capabilities
    return isDesktop.value && Boolean(
      capabilities?.temporary_connect || capabilities?.oneshot_mode,
    )
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

  const dashboardState = computed<DashboardState>(() => {
    if (lastError.value && lastErrorType.value) {
      if (lastErrorType.value !== 'elevation_required' && lastErrorType.value !== 'service_missing') {
        if (lastErrorType.value === 'runtime_missing') return 'error blocking'
        return lastRecoverable.value ? 'error recoverable' : 'error blocking'
      }
    }

    if (loading.value && lastActionWasElevatedConnect.value) return 'elevated connecting'

    if (status.value?.connected) {
      const mode = currentSessionMode.value
      if (mode === 'helper') return 'helper connected'
      if (mode === 'elevated') return 'elevated connected'
      return 'direct connected'
    }

    if (serviceInstalled.value && serviceRunning.value) return 'service-ready disconnected'
    return 'service-missing disconnected'
  })

  const recoverableErrorAction = computed<DashboardAction | null>(() => {
    switch (lastErrorType.value) {
      case 'elevation_denied':
        return { label: '安装服务', action: () => installService(), variant: 'primary' }
      case 'config_invalid':
        return { label: '前往设置', action: () => {}, variant: 'primary' }
      case 'native_failure':
      case 'parse_failure':
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
      default:
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
    }
  })

  const dashboardPrimaryAction = computed<DashboardAction | null>(() => {
    switch (dashboardState.value) {
      case 'service-ready disconnected':
        return { label: '连接', action: () => connect(), variant: 'primary' }
      case 'service-missing disconnected':
        return { label: '安装服务（推荐）', action: () => installService(), variant: 'primary' }
      case 'helper connected':
        return { label: '断开连接', action: () => disconnect(), variant: 'destructive' }
      case 'direct connected':
      case 'elevated connected':
        return { label: '断开连接', action: () => disconnectElevated(), variant: 'destructive' }
      case 'elevated connecting':
        return null
      case 'error recoverable':
        return recoverableErrorAction.value
      case 'error blocking':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  const dashboardSecondaryAction = computed<DashboardAction | null>(() => {
    switch (dashboardState.value) {
      case 'service-missing disconnected':
        return canUseElevatedFallback.value
          ? { label: '仅本次连接', action: () => connectElevated(), variant: 'secondary' }
          : null
      case 'direct connected':
      case 'elevated connected':
        return { label: '安装服务', action: () => installService(), variant: 'secondary' }
      case 'error recoverable':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  function setError(err: VpnError) {
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

  function retryLastAction() {
    clearError()
    lastMutatingAction.value?.()
  }

  async function fetchStatus() {
    try {
      const { data } = await api.get<VpnStatus>('/status')
      status.value = data
      if (data.connected) clearError()
    } catch (e) {
      console.error('[vpn] fetchStatus failed:', e)
    }
  }

  async function fetchAppShellState() {
    await Promise.allSettled([fetchStatus(), fetchServiceStatus()])
  }

  async function connect() {
    loading.value = true
    clearError()
    lastActionWasElevatedConnect.value = false
    lastMutatingAction.value = connect
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
    lastMutatingAction.value = disconnect
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
    lastMutatingAction.value = connectElevated
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/connect/elevated')
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = { ...data, mode: data.mode ?? 'elevated' }
        activeTemporaryBackend.value = status.value.backend ?? null
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
    lastMutatingAction.value = disconnectElevated
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/disconnect/elevated', {
        backend: activeTemporaryBackend.value,
      })
      if (isVpnError(data)) {
        setError(data)
      } else {
        status.value = data
        activeTemporaryBackend.value = null
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
    } catch (e) {
      console.error('[vpn] fetchRoutes failed:', e)
    }
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
    } catch (e) {
      console.error('[vpn] fetchServiceStatus failed:', e)
    }
  }

  async function installService() {
    serviceBusy.value = true
    serviceProgress.value = []
    try {
      const { data } = await api.post<ServiceStatus>('/service/install')
      serviceStatus.value = data
      if (data.warning || !data.available) {
        throw new Error(data.warning || 'Helper service is not available after install.')
      }
      await fetchAppShellState()
    } finally {
      serviceBusy.value = false
    }
  }

  async function uninstallService() {
    serviceBusy.value = true
    serviceProgress.value = []
    try {
      const { data } = await api.post<ServiceStatus>('/service/uninstall')
      serviceStatus.value = data
      if (data.warning || data.installed) {
        throw new Error(data.warning || 'Helper service is still installed after uninstall.')
      }
      await fetchAppShellState()
    } finally {
      serviceBusy.value = false
    }
  }

  function addServiceProgress(entry: ServiceProgressEntry) {
    serviceProgress.value.push(entry)
    if (serviceProgress.value.length > 200) {
      serviceProgress.value = serviceProgress.value.slice(-200)
    }
  }

  function addLog(entry: LogEntry) {
    logs.value.push(entry)
    if (logs.value.length > 1000) logs.value = logs.value.slice(-1000)
  }

  function clearLogs() {
    logs.value = []
  }

  function setLogs(entries: LogEntry[]) {
    logs.value = entries
  }

  return {
    status, loading, routes, logs, serviceStatus, serviceProgress, serviceBusy,
    lastError, lastErrorType, lastRecoverable, lastRecommendedAction, lastErrorTime,
    serviceInstalled, serviceRunning, canUseElevatedFallback,
    recommendedConnectMode, currentSessionMode,
    isDesktop, dashboardState, dashboardPrimaryAction, dashboardSecondaryAction,
    fetchStatus, fetchAppShellState, connect, disconnect, connectElevated, disconnectElevated,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, installService, uninstallService,
    addLog, clearLogs, setLogs, addServiceProgress, clearError, retryLastAction,
  }
})
