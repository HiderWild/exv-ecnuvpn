import {
  desktopApiPaths,
} from '../../desktop/shared/desktop-contract'

type ApiResponse<T> = Promise<{ data: T }>

function desktopAvailable() {
  return typeof window !== 'undefined' && !!window.ecnuVpn
}

function requireDesktop() {
  if (!desktopAvailable()) {
    throw new Error('Desktop API is only available inside the Electron app.')
  }
}

async function wrap<T>(promise: Promise<T>): ApiResponse<T> {
  return { data: await promise }
}

function unsupported(path: string): never {
  throw new Error(`Unsupported desktop API path: ${path}`)
}

function plainPayload<T>(value: T): T {
  if (value === undefined || value === null) return value
  return JSON.parse(JSON.stringify(value)) as T
}

const desktopApi = {
  get<T = unknown>(path: string): ApiResponse<T> {
    requireDesktop()

    switch (path) {
      case desktopApiPaths.status:
        return wrap(window.ecnuVpn!.status.get()) as ApiResponse<T>
      case desktopApiPaths.configAuth:
        return wrap(window.ecnuVpn!.config.getAuth()) as ApiResponse<T>
      case desktopApiPaths.configSettings:
        return wrap(window.ecnuVpn!.config.getSettings()) as ApiResponse<T>
      case desktopApiPaths.configKey:
      case desktopApiPaths.configKeyAlias:
        return wrap(window.ecnuVpn!.config.getKey()) as ApiResponse<T>
      case desktopApiPaths.routes:
        return wrap(window.ecnuVpn!.routes.list()) as ApiResponse<T>
      case desktopApiPaths.service:
        return wrap(window.ecnuVpn!.service.status()) as ApiResponse<T>
      case desktopApiPaths.runtime:
        return wrap(window.ecnuVpn!.runtime.status()) as ApiResponse<T>
      case desktopApiPaths.drivers:
        return wrap(window.ecnuVpn!.drivers.status()) as ApiResponse<T>
      case desktopApiPaths.logs:
        return wrap(window.ecnuVpn!.logs.list()) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  post<T = unknown>(path: string, body?: any): ApiResponse<T> {
    requireDesktop()

    switch (path) {
      case desktopApiPaths.connect:
        return wrap(window.ecnuVpn!.vpn.connect(body?.password)) as ApiResponse<T>
      case desktopApiPaths.connectElevated:
        return wrap(window.ecnuVpn!.vpn.connectElevated(body?.password)) as ApiResponse<T>
      case desktopApiPaths.disconnect:
        return wrap(window.ecnuVpn!.vpn.disconnect()) as ApiResponse<T>
      case desktopApiPaths.disconnectElevated:
        return wrap(
          window.ecnuVpn!.vpn.disconnectElevated(plainPayload(body)?.backend),
        ) as ApiResponse<T>
      case desktopApiPaths.routes:
        return wrap(window.ecnuVpn!.routes.add(plainPayload(body)?.cidr ?? '')) as ApiResponse<T>
      case desktopApiPaths.routesReset:
        return wrap(window.ecnuVpn!.routes.reset()) as ApiResponse<T>
      case desktopApiPaths.serviceInstall:
        return wrap(window.ecnuVpn!.service.install()) as ApiResponse<T>
      case desktopApiPaths.serviceUninstall:
        return wrap(window.ecnuVpn!.service.uninstall()) as ApiResponse<T>
      case desktopApiPaths.driversInstall:
        return wrap(window.ecnuVpn!.drivers.install(plainPayload(body)?.driver)) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  put<T = unknown>(path: string, body?: any): ApiResponse<T> {
    requireDesktop()

    switch (path) {
      case desktopApiPaths.configAuth:
        return wrap(window.ecnuVpn!.config.saveAuth(plainPayload(body ?? {}))) as ApiResponse<T>
      case desktopApiPaths.configSettings:
        return wrap(window.ecnuVpn!.config.saveSettings(plainPayload(body ?? {}))) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  delete<T = unknown>(path: string, options?: { data?: any }): ApiResponse<T> {
    requireDesktop()

    switch (path) {
      case desktopApiPaths.routes:
        return wrap(window.ecnuVpn!.routes.remove(plainPayload(options?.data)?.cidr ?? '')) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },
}

export default desktopApi
