import {
  desktopApiPaths,
} from '../../host/shared/host-contract'

type ApiResponse<T> = Promise<{ data: T }>
type ApiGetOptions = { params?: any }

function hostAvailable() {
  return typeof window !== 'undefined' && !!window.exv
}

function requireHost() {
  if (!hostAvailable()) {
    throw new Error('Desktop host API is not available in this shell.')
  }
}

async function wrap<T>(promise: Promise<T>): ApiResponse<T> {
  return { data: await promise }
}

function unsupported(path: string): never {
  throw new Error(`Unsupported host API path: ${path}`)
}

function plainPayload<T>(value: T): T {
  if (value === undefined || value === null) return value
  return JSON.parse(JSON.stringify(value)) as T
}

const hostApi = {
  get<T = unknown>(path: string, options?: ApiGetOptions): ApiResponse<T> {
    requireHost()

    switch (path) {
      case desktopApiPaths.status:
        return wrap(window.exv!.status.get()) as ApiResponse<T>
      case desktopApiPaths.configAuth:
        return wrap(window.exv!.config.getAuth()) as ApiResponse<T>
      case desktopApiPaths.configSettings:
        return wrap(window.exv!.config.getSettings()) as ApiResponse<T>
      case desktopApiPaths.configKey:
      case desktopApiPaths.configKeyAlias:
        return wrap(window.exv!.config.getKey()) as ApiResponse<T>
      case desktopApiPaths.routes:
        return wrap(window.exv!.routes.list()) as ApiResponse<T>
      case desktopApiPaths.service:
        return wrap(window.exv!.service.status()) as ApiResponse<T>
      case desktopApiPaths.cli:
        return wrap(window.exv!.cli.status()) as ApiResponse<T>
      case desktopApiPaths.runtime:
        return wrap(window.exv!.runtime.status()) as ApiResponse<T>
      case desktopApiPaths.drivers:
        return wrap(window.exv!.drivers.status()) as ApiResponse<T>
      case desktopApiPaths.logs:
        return wrap(window.exv!.logs.list(plainPayload(options?.params ?? {}))) as ApiResponse<T>
      case desktopApiPaths.maintenanceCore:
        return wrap(window.exv!.maintenance.inspectCore()) as ApiResponse<T>
      case desktopApiPaths.authInteraction:
        return wrap(window.exv!.vpn.authInteraction()) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  post<T = unknown>(path: string, body?: any): ApiResponse<T> {
    requireHost()

    switch (path) {
      case desktopApiPaths.connect:
        return wrap(window.exv!.vpn.connect(body?.password)) as ApiResponse<T>
      case desktopApiPaths.connectElevated:
        return wrap(window.exv!.vpn.connectElevated(body?.password)) as ApiResponse<T>
      case desktopApiPaths.disconnect:
        return wrap(window.exv!.vpn.disconnect()) as ApiResponse<T>
      case desktopApiPaths.disconnectElevated:
        return wrap(
          window.exv!.vpn.disconnectElevated(plainPayload(body)?.backend),
        ) as ApiResponse<T>
      case desktopApiPaths.authInteractionResponse: {
        const payload = plainPayload(body ?? {})
        return wrap(
          window.exv!.vpn.respondAuthInteraction(payload.id ?? '', payload.value ?? ''),
        ) as ApiResponse<T>
      }
      case desktopApiPaths.routes:
        return wrap(window.exv!.routes.add(plainPayload(body)?.cidr ?? '')) as ApiResponse<T>
      case desktopApiPaths.routesReset:
        return wrap(window.exv!.routes.reset()) as ApiResponse<T>
      case desktopApiPaths.serviceInstall:
        return wrap(window.exv!.service.install()) as ApiResponse<T>
      case desktopApiPaths.serviceUninstall:
        return wrap(window.exv!.service.uninstall()) as ApiResponse<T>
      case desktopApiPaths.cliInstall:
        return wrap(window.exv!.cli.install()) as ApiResponse<T>
      case desktopApiPaths.cliUninstall:
        return wrap(window.exv!.cli.uninstall()) as ApiResponse<T>
      case desktopApiPaths.driversInstall:
        return wrap(window.exv!.drivers.install(plainPayload(body)?.driver)) as ApiResponse<T>
      case desktopApiPaths.configImport:
        return wrap(window.exv!.config.importConfig(plainPayload(body))) as ApiResponse<T>
      case desktopApiPaths.configExport:
        return wrap(window.exv!.config.exportConfig(plainPayload(body))) as ApiResponse<T>
      case desktopApiPaths.configReset:
        return wrap(window.exv!.config.reset(plainPayload(body)?.confirm ?? false)) as ApiResponse<T>
      case desktopApiPaths.keyReset:
        return wrap(window.exv!.key.reset(plainPayload(body)?.confirm ?? false)) as ApiResponse<T>
      case desktopApiPaths.maintenanceCoreKill:
        return wrap(window.exv!.maintenance.killStaleCore(plainPayload(body)?.confirm ?? false)) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  put<T = unknown>(path: string, body?: any): ApiResponse<T> {
    requireHost()

    switch (path) {
      case desktopApiPaths.configAuth:
        return wrap(window.exv!.config.saveAuth(plainPayload(body ?? {}))) as ApiResponse<T>
      case desktopApiPaths.configSettings:
        return wrap(window.exv!.config.saveSettings(plainPayload(body ?? {}))) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  delete<T = unknown>(path: string, options?: { data?: any }): ApiResponse<T> {
    requireHost()

    switch (path) {
      case desktopApiPaths.routes:
        return wrap(window.exv!.routes.remove(plainPayload(options?.data)?.cidr ?? '')) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },
}

export default hostApi
