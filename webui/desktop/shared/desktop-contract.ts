export const desktopIpcChannels = {
  rpc: 'ecnu-vpn:rpc',
  rpcElevated: 'ecnu-vpn:rpc-elevated',
  serviceCommand: 'ecnu-vpn:service-command',
  cliCommand: 'ecnu-vpn:cli-command',
  driverInstall: 'ecnu-vpn:driver-install',
  windowMode: 'ecnu-vpn:window-mode',
  serviceInstallPrompt: 'ecnu-vpn:service-install-prompt',
  passwordPrompt: 'ecnu-vpn:password-prompt',
  confirmPrompt: 'ecnu-vpn:confirm-prompt',
  modalPayload: 'ecnu-vpn:modal-payload',
  modalResult: 'ecnu-vpn:modal-result',
  closePromptResult: 'ecnu-vpn:close-prompt-result',
  event: 'ecnu-vpn:event',
} as const

export const desktopApiPaths = {
  status: '/status',
  connect: '/connect',
  connectElevated: '/connect/elevated',
  disconnect: '/disconnect',
  disconnectElevated: '/disconnect/elevated',
  configAuth: '/config/auth',
  configSettings: '/config/settings',
  configKey: '/config/key',
  configKeyAlias: '/key',
  routes: '/routes',
  routesReset: '/routes/reset',
  service: '/service',
  serviceInstall: '/service/install',
  serviceUninstall: '/service/uninstall',
  cli: '/cli',
  cliInstall: '/cli/install',
  cliUninstall: '/cli/uninstall',
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
  'helper.status',
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
  'close-request',
] as const

export const desktopServiceCommands = ['install', 'uninstall'] as const
export const desktopCliCommands = ['status', 'install', 'uninstall'] as const
export const desktopDriverInstallTargets = ['wintun', 'tap'] as const

export const desktopRpcErrorCodes = {
  helperUnavailable: 'helper_unavailable',
  serviceNotInstalled: 'service_not_installed',
  serviceInstalledNotRunning: 'service_installed_not_running',
  serviceStartFailed: 'service_start_failed',
  oneshotNotSupported: 'oneshot_not_supported',
  oneshotElevationDenied: 'oneshot_elevation_denied',
  helperRpcFailed: 'helper_rpc_failed',
  authFailed: 'auth_failed',
  vpnStartFailed: 'vpn_start_failed',
} as const

export type DesktopRpcAction = (typeof desktopRpcActions)[number]
export type DesktopEventType = (typeof desktopEventTypes)[number]
export type DesktopServiceCommand = (typeof desktopServiceCommands)[number]
export type DesktopCliCommand = (typeof desktopCliCommands)[number]
export type DesktopDriverInstallTarget =
  (typeof desktopDriverInstallTargets)[number]
export type DesktopRpcErrorCode =
  (typeof desktopRpcErrorCodes)[keyof typeof desktopRpcErrorCodes]
export type DesktopWindowMode = 'minimal' | 'advanced'
export type DesktopServiceInstallPromptResult = 'install' | 'dismiss'
export type DesktopModalKind = 'service-install' | 'password' | 'confirm' | 'close-app'
export interface DesktopModalPayload {
  kind: DesktopModalKind
  message?: string
}
