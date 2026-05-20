export const desktopIpcChannels = {
  rpc: 'ecnu-vpn:rpc',
  rpcElevated: 'ecnu-vpn:rpc-elevated',
  serviceCommand: 'ecnu-vpn:service-command',
  driverInstall: 'ecnu-vpn:driver-install',
  event: 'ecnu-vpn:event',
} as const

export const desktopApiPaths = {
  status: '/status',
  connect: '/connect',
  disconnect: '/disconnect',
  configAuth: '/config/auth',
  configSettings: '/config/settings',
  configKey: '/config/key',
  configKeyAlias: '/key',
  routes: '/routes',
  routesReset: '/routes/reset',
  service: '/service',
  serviceInstall: '/service/install',
  serviceUninstall: '/service/uninstall',
  runtime: '/runtime',
  drivers: '/drivers',
  driversInstall: '/drivers/install',
  logs: '/logs',
} as const

export const desktopRpcActions = [
  'status.get',
  'vpn.connect',
  'vpn.disconnect',
  'config.getAuth',
  'config.saveAuth',
  'config.getSettings',
  'config.saveSettings',
  'config.getKey',
  'routes.list',
  'routes.add',
  'routes.remove',
  'routes.reset',
  'service.status',
  'runtime.status',
  'drivers.status',
  'drivers.install',
  'logs.list',
] as const

export const desktopEventTypes = [
  'log',
  'status',
  'heartbeat',
  'service-progress',
] as const

export const desktopServiceCommands = ['install', 'uninstall'] as const
export const desktopDriverInstallTargets = ['wintun', 'tap'] as const

export const desktopRpcErrorCodes = {
  helperUnavailable: 'helper_unavailable',
} as const

export type DesktopRpcAction = (typeof desktopRpcActions)[number]
export type DesktopEventType = (typeof desktopEventTypes)[number]
export type DesktopServiceCommand = (typeof desktopServiceCommands)[number]
export type DesktopDriverInstallTarget =
  (typeof desktopDriverInstallTargets)[number]
export type DesktopRpcErrorCode =
  (typeof desktopRpcErrorCodes)[keyof typeof desktopRpcErrorCodes]