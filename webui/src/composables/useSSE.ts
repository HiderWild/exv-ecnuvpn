import { ref, onUnmounted } from 'vue'
import { useVpnStore, type LogEntry, type ServiceProgressEntry, type VpnStatus } from '../stores/vpn'

export function useSSE() {
  const connected = ref(false)
  const error = ref<string | null>(null)
  let unsubscribe: (() => void) | null = null

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
    error.value = 'Desktop event bridge is unavailable'
  }

  function disconnect() {
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
