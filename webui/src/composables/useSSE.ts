import { ref, onUnmounted } from 'vue'
import { useVpnStore, type LogEntry, type ServiceProgressEntry, type VpnStatus } from '../stores/vpn'

export function useSSE() {
  const connected = ref(false)
  const error = ref<string | null>(null)
  let eventSource: EventSource | null = null
  let unsubscribe: (() => void) | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null

  function connect() {
    if (window.ecnuVpn) {
      disconnect()
      unsubscribe = window.ecnuVpn.events.subscribe((event) => {
        connected.value = true
        error.value = null

        if (event.type === 'log') {
          const store = useVpnStore()
          const data = event.data as Partial<LogEntry> & { raw?: string }
          store.addLog({
            timestamp: data.timestamp || new Date().toISOString(),
            level: data.level || 'info',
            message: data.message || data.raw || '',
          })
        }

        if (event.type === 'status' && event.data && typeof event.data === 'object') {
          const store = useVpnStore()
          store.status = store.status
            ? { ...store.status, ...(event.data as Partial<VpnStatus>) }
            : (event.data as VpnStatus)
        }

        if (event.type === 'service-progress' && event.data && typeof event.data === 'object') {
          const store = useVpnStore()
          store.addServiceProgress(event.data as ServiceProgressEntry)
        }
      })
      return
    }

    if (eventSource) {
      eventSource.close()
    }

    const base = window.location.origin
    eventSource = new EventSource(`${base}/api/events`)

    eventSource.onopen = () => {
      connected.value = true
      error.value = null
    }

    eventSource.onerror = () => {
      connected.value = false
      error.value = 'SSE connection lost'
      // Auto-reconnect after 5 seconds
      if (reconnectTimer) clearTimeout(reconnectTimer)
      reconnectTimer = setTimeout(() => connect(), 5000)
    }

    eventSource.addEventListener('log', (e: MessageEvent) => {
      try {
        const store = useVpnStore()
        const data = JSON.parse(e.data)
        const entry: LogEntry = {
          timestamp: data.timestamp || new Date().toISOString(),
          level: data.level || 'info',
          message: data.message || data.raw || e.data,
        }
        store.addLog(entry)
      } catch {}
    })

    eventSource.addEventListener('status', (e: MessageEvent) => {
      try {
        const store = useVpnStore()
        const data = JSON.parse(e.data)
        if (data && typeof data === 'object') {
          store.status = store.status
            ? { ...store.status, ...(data as Partial<VpnStatus>) }
            : (data as VpnStatus)
        }
      } catch {}
    })

    eventSource.addEventListener('heartbeat', () => {
      // Heartbeat received, connection is alive
    })
  }

  function disconnect() {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (eventSource) {
      eventSource.close()
      eventSource = null
    }
    if (unsubscribe) {
      unsubscribe()
      unsubscribe = null
    }
    connected.value = false
  }

  onUnmounted(() => {
    disconnect()
  })

  return { connected, error, connect, disconnect }
}
