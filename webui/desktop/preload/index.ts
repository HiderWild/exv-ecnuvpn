import { contextBridge, ipcRenderer } from 'electron'
import {
  desktopCliCommands,
  desktopDriverInstallTargets,
  desktopEventTypes,
  desktopIpcChannels,
  desktopServiceCommands,
  type DesktopCliCommand,
  type DesktopDriverInstallTarget,
  type DesktopEventType,
  type DesktopRpcAction,
  type DesktopServiceCommand,
} from '../shared/desktop-contract.js'

type EventHandler = (event: { type: DesktopEventType; data: unknown }) => void

void desktopDriverInstallTargets
void desktopCliCommands
void desktopEventTypes
void desktopServiceCommands

function rpc(action: DesktopRpcAction, payload?: unknown) {
  return ipcRenderer.invoke(desktopIpcChannels.rpc, action, payload ?? {})
}

const api = {
  status: {
    get: () => rpc('status.get'),
  },
  vpn: {
    connect: (password?: string) => rpc('vpn.connect', { password }),
    disconnect: () => rpc('vpn.disconnect'),
    connectElevated: (password?: string) =>
      ipcRenderer.invoke(desktopIpcChannels.rpcElevated, 'vpn.connect', {
        password,
        allow_direct_fallback: true,
      }, 'status.get'),
    disconnectElevated: (backend?: unknown) =>
      ipcRenderer.invoke(desktopIpcChannels.rpcElevated, 'vpn.disconnect', {
        allow_direct_fallback: true,
        backend,
      }, 'status.get'),
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
    install: () => ipcRenderer.invoke(desktopIpcChannels.serviceCommand, 'install' satisfies DesktopServiceCommand),
    uninstall: () => ipcRenderer.invoke(desktopIpcChannels.serviceCommand, 'uninstall' satisfies DesktopServiceCommand),
  },
  cli: {
    status: () => ipcRenderer.invoke(desktopIpcChannels.cliCommand, 'status' satisfies DesktopCliCommand),
    install: () => ipcRenderer.invoke(desktopIpcChannels.cliCommand, 'install' satisfies DesktopCliCommand),
    uninstall: () => ipcRenderer.invoke(desktopIpcChannels.cliCommand, 'uninstall' satisfies DesktopCliCommand),
  },
  logs: {
    list: (options?: { lines?: number; filter?: string }) => rpc('logs.list', options ?? {}),
  },
  runtime: {
    status: () => rpc('runtime.status'),
  },
  drivers: {
    status: () => rpc('drivers.status'),
    install: (driver: DesktopDriverInstallTarget) => ipcRenderer.invoke(desktopIpcChannels.driverInstall, driver),
  },
  events: {
    subscribe: (handler: EventHandler) => {
      const listener = (_event: Electron.IpcRendererEvent, payload: { type: DesktopEventType; data: unknown }) => {
        handler(payload)
      }
      ipcRenderer.on(desktopIpcChannels.event, listener)
      return () => ipcRenderer.removeListener(desktopIpcChannels.event, listener)
    },
  },
}

contextBridge.exposeInMainWorld('ecnuVpn', api)
