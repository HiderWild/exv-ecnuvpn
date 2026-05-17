import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import api from '../api/desktop'
import { errorMessage } from '../utils/errors'
import type { SessionMode, VpnError, VpnErrorType } from '../types/ecnu-vpn'

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
  session_mode: SessionMode
  cleanup_pending: boolean
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

export type DashboardState =
  | 'helper_ready'
  | 'helper_missing'
  | 'elevated_connecting'
  | 'direct_connected'
  | 'helper_connected'
  | 'cleanup_pending'
  | 'runtime_missing'
  | 'authorization_denied'
  | 'disconnected'

export const useVpnStore = defineStore('vpn', () => {
  const status = ref<VpnStatus | null>(null)
  const routes = ref<RouteEntry[]>([])
  const logs = ref<LogEntry[]>([])
  const serviceStatus = ref<ServiceStatus | null>(null)
  const loading = ref(false)
  const lastError = ref<VpnError | null>(null)

  const sessionMode = computed<SessionMode>(() => status.value?.session_mode ?? 'disconnected')
  const cleanupPending = computed(() => status.value?.cleanup_pending ?? false)

  const dashboardState = computed<DashboardState>(() => {
    if (cleanupPending.value) return 'cleanup_pending'
    if (lastError.value?.type === 'elevation_denied') return 'authorization_denied'
    if (lastError.value?.type === 'runtime_missing') return 'runtime_missing'
    if (!status.value) return 'disconnected'
    if (status.value.connected && sessionMode.value === 'helper') return 'helper_connected'
    if (status.value.connected && sessionMode.value === 'elevated') return 'direct_connected'
    if (status.value.connected && sessionMode.value === 'direct') return 'direct_connected'
    if (loading.value && sessionMode.value !== 'helper') return 'elevated_connecting'
    if (serviceStatus.value?.installed && serviceStatus.value?.running) return 'helper_ready'
    return 'helper_missing'
  })

  function classifyError(error: unknown): VpnError {
    const msg = errorMessage(error).toLowerCase()
    let type: VpnErrorType = 'unknown'
    let recoverable = true
    let recovery_hint: string | undefined

    if (msg.includes('administrator') || msg.includes('denied') || msg.includes('cancel') || msg.includes('not allowed')) {
      type = 'elevation_denied'
      recoverable = true
      recovery_hint = '请重新尝试并输入管理员密码，或安装 launchd 辅助服务以避免每次授权。'
    } else if (msg.includes('openconnect') || msg.includes('runtime') || msg.includes('not found')) {
      type = 'runtime_missing'
      recoverable = true
      recovery_hint = '请在设置中选择正确的 OpenConnect 运行时来源，或安装 openconnect。'
    } else if (msg.includes('config') || msg.includes('no server') || msg.includes('no username')) {
      type = 'config_missing'
      recoverable = true
      recovery_hint = '请先在认证页面填写服务器地址和用户名。'
    } else if (msg.includes('helper_missing')) {
      type = 'helper_missing'
      recoverable = true
      recovery_hint = '辅助服务未安装，将尝试一次性管理员授权连接...'
    } else if (msg.includes('helper') || msg.includes('socket') || msg.includes('launchd') || msg.includes('daemon')) {
      type = 'helper_unavailable'
      recoverable = true
      recovery_hint = '辅助服务不可用，请尝试安装服务或使用一次性授权连接。'
    } else if (msg.includes('route') || msg.includes('cleanup')) {
      type = 'cleanup_failed'
      recoverable = false
      recovery_hint = '路由清理失败，请手动检查系统路由表或重启应用。'
    } else if (msg.includes('connect') || msg.includes('timeout')) {
      type = 'connect_failed'
      recoverable = true
      recovery_hint = '连接失败，请检查网络和服务器地址后重试。'
    }

    return { type, message: errorMessage(error), recoverable, recovery_hint }
  }

  async function fetchStatus() {
    try {
      const { data } = await api.get<VpnStatus>('/status')
      status.value = data
      if (data.connected) lastError.value = null
    } catch (e) { console.error('[vpn] fetchStatus failed:', e) }
  }

  async function connect() {
    loading.value = true
    lastError.value = null
    try {
      const { data } = await api.post<VpnStatus>('/connect')
      status.value = data
    } catch (error: unknown) {
      const err = classifyError(error)
      // On macOS, if helper is missing, try elevated connection via osascript
      if (err.type === 'helper_missing' && window.ecnuVpn?.vpn?.connectElevated) {
        try {
          const result = await window.ecnuVpn.vpn.connectElevated()
          if (result && typeof result === 'object' && 'connected' in result) {
            status.value = result as VpnStatus
            lastError.value = null
            return
          }
        } catch (elevatedError) {
          lastError.value = classifyError(elevatedError)
          return
        }
      }
      lastError.value = err
    } finally {
      loading.value = false
    }
  }

  async function disconnect() {
    loading.value = true
    try {
      const { data } = await api.post<VpnStatus>('/disconnect')
      status.value = data
      lastError.value = null
    } catch (error) {
      lastError.value = classifyError(error)
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
    status, loading, routes, logs, serviceStatus, lastError,
    sessionMode, cleanupPending, dashboardState,
    fetchStatus, connect, disconnect,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, installService, uninstallService,
    addLog, clearLogs, setLogs,
  }
})
