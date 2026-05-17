import { contextBridge, ipcRenderer } from 'electron'

type EventHandler = (event: { type: string; data: unknown }) => void

function rpc(action: string, payload?: unknown) {
  return ipcRenderer.invoke('ecnu-vpn:rpc', action, payload ?? {})
}

const api = {
  status: {
    get: () => rpc('status.get'),
  },
  vpn: {
    connect: (password?: string) => rpc('vpn.connect', { password }),
    connectElevated: (password?: string) => ipcRenderer.invoke('ecnu-vpn:connect-elevated', { password }),
    disconnect: () => rpc('vpn.disconnect'),
  },
  config: {
    getAuth: () => rpc('config.getAuth'),
    saveAuth: (input: unknown) => rpc('config.saveAuth', input),
    getSettings: () => rpc('config.getSettings'),
    saveSettings: (input: unknown) => rpc('config.saveSettings', input),
    getKey: () => rpc('config.getKey'),
  },
  routes: {
    list: () => rpc('routes.list'),
    add: (cidr: string) => rpc('routes.add', { cidr }),
    remove: (cidr: string) => rpc('routes.remove', { cidr }),
    reset: () => rpc('routes.reset'),
  },
  service: {
    status: () => rpc('service.status'),
    install: () => ipcRenderer.invoke('ecnu-vpn:service-command', 'install'),
    uninstall: () => ipcRenderer.invoke('ecnu-vpn:service-command', 'uninstall'),
  },
  logs: {
    list: (options?: { lines?: number; filter?: string }) => rpc('logs.list', options ?? {}),
  },
  runtime: {
    status: () => rpc('runtime.status'),
  },
  helper: {
    status: () => rpc('helper.status'),
  },
  events: {
    subscribe: (handler: EventHandler) => {
      const listener = (_event: Electron.IpcRendererEvent, payload: { type: string; data: unknown }) => {
        handler(payload)
      }
      ipcRenderer.on('ecnu-vpn:event', listener)
      return () => ipcRenderer.removeListener('ecnu-vpn:event', listener)
    },
  },
}

contextBridge.exposeInMainWorld('ecnuVpn', api)