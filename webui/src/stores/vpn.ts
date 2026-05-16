import { defineStore } from 'pinia'
import { ref } from 'vue'
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

export const useVpnStore = defineStore('vpn', () => {
  const status = ref<VpnStatus | null>(null)
  const routes = ref<RouteEntry[]>([])
  const logs = ref<LogEntry[]>([])
  const serviceStatus = ref<ServiceStatus | null>(null)
  const loading = ref(false)
  const lastError = ref<string | null>(null)

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
    } catch (error) {
      lastError.value = errorMessage(error)
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
      lastError.value = errorMessage(error)
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
    fetchStatus, connect, disconnect,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, installService, uninstallService,
    addLog, clearLogs, setLogs,
  }
})
