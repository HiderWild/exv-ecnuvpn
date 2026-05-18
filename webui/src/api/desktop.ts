import httpApi from './client'

type ApiResponse<T> = Promise<{ data: T }>

function desktopAvailable() {
  return typeof window !== 'undefined' && !!window.ecnuVpn
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
    if (!desktopAvailable()) return httpApi.get(path)

    switch (path) {
      case '/status':
        return wrap(window.ecnuVpn!.status.get()) as ApiResponse<T>
      case '/config/auth':
        return wrap(window.ecnuVpn!.config.getAuth()) as ApiResponse<T>
      case '/config/settings':
        return wrap(window.ecnuVpn!.config.getSettings()) as ApiResponse<T>
      case '/config/key':
      case '/key':
        return wrap(window.ecnuVpn!.config.getKey()) as ApiResponse<T>
      case '/routes':
        return wrap(window.ecnuVpn!.routes.list()) as ApiResponse<T>
      case '/service':
        return wrap(window.ecnuVpn!.service.status()) as ApiResponse<T>
      case '/runtime':
        return wrap(window.ecnuVpn!.runtime.status()) as ApiResponse<T>
      case '/drivers':
        return wrap(window.ecnuVpn!.drivers.status()) as ApiResponse<T>
      case '/logs':
        return wrap(window.ecnuVpn!.logs.list()) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  post<T = unknown>(path: string, body?: any): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.post(path, body)

    switch (path) {
      case '/connect':
        return wrap(
          window.ecnuVpn!.vpn.connect(body?.password).catch((error) => {
            const message = error?.message || String(error)
            if (message.includes('Helper daemon is not available')) {
              return window.ecnuVpn!.vpn.connectElevated(body?.password)
            }
            throw error
          }),
        ) as ApiResponse<T>
      case '/connect/elevated':
        return wrap(window.ecnuVpn!.vpn.connectElevated(plainPayload(body)?.password)) as ApiResponse<T>
      case '/disconnect':
        return wrap(window.ecnuVpn!.vpn.disconnect()) as ApiResponse<T>
      case '/connect/elevated':
        return wrap(window.ecnuVpn!.vpn.connectElevated(body?.password)) as ApiResponse<T>
      case '/disconnect/elevated':
        return wrap(window.ecnuVpn!.vpn.disconnectElevated()) as ApiResponse<T>
      case '/routes':
        return wrap(window.ecnuVpn!.routes.add(plainPayload(body)?.cidr ?? '')) as ApiResponse<T>
      case '/routes/reset':
        return wrap(window.ecnuVpn!.routes.reset()) as ApiResponse<T>
      case '/service/install':
        return wrap(window.ecnuVpn!.service.install()) as ApiResponse<T>
      case '/service/uninstall':
        return wrap(window.ecnuVpn!.service.uninstall()) as ApiResponse<T>
      case '/drivers/install':
        return wrap(window.ecnuVpn!.drivers.install(plainPayload(body)?.driver)) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  put<T = unknown>(path: string, body?: any): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.put(path, body)

    switch (path) {
      case '/config/auth':
        return wrap(window.ecnuVpn!.config.saveAuth(plainPayload(body ?? {}))) as ApiResponse<T>
      case '/config/settings':
        return wrap(window.ecnuVpn!.config.saveSettings(plainPayload(body ?? {}))) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  delete<T = unknown>(path: string, options?: { data?: any }): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.delete(path, options)

    switch (path) {
      case '/routes':
        return wrap(window.ecnuVpn!.routes.remove(plainPayload(options?.data)?.cidr ?? '')) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },
}

export default desktopApi
