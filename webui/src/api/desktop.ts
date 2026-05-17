import httpApi from './client'

type ApiResponse<T> = Promise<{ data: T }>

function desktopAvailable() {
  return typeof window !== 'undefined' && !!window.ecnuVpn
}

// Strip Vue reactive proxies by round-tripping through JSON.
// Electron IPC uses structured clone which cannot handle Vue proxy internals.
function deproxy<T>(value: T): T {
  if (value && typeof value === 'object') {
    return JSON.parse(JSON.stringify(value))
  }
  return value
}

async function wrap<T>(promise: Promise<T>): ApiResponse<T> {
  return { data: await promise }
}

function unsupported(path: string): never {
  throw new Error(`Unsupported desktop API path: ${path}`)
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
      case '/helper':
        return wrap(window.ecnuVpn!.helper.status()) as ApiResponse<T>
      case '/logs':
        return wrap(window.ecnuVpn!.logs.list()) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  post<T = unknown>(path: string, body?: any): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.post(path, deproxy(body))

    switch (path) {
      case '/connect':
        return wrap(window.ecnuVpn!.vpn.connect(deproxy(body)?.password)) as ApiResponse<T>
      case '/connect/elevated':
        return wrap(window.ecnuVpn!.vpn.connectElevated(deproxy(body)?.password)) as ApiResponse<T>
      case '/disconnect':
        return wrap(window.ecnuVpn!.vpn.disconnect()) as ApiResponse<T>
      case '/routes':
        return wrap(window.ecnuVpn!.routes.add(deproxy(body)?.cidr ?? '')) as ApiResponse<T>
      case '/routes/reset':
        return wrap(window.ecnuVpn!.routes.reset()) as ApiResponse<T>
      case '/service/install':
        return wrap(window.ecnuVpn!.service.install()) as ApiResponse<T>
      case '/service/uninstall':
        return wrap(window.ecnuVpn!.service.uninstall()) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  put<T = unknown>(path: string, body?: any): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.put(path, deproxy(body))

    switch (path) {
      case '/config/auth':
        return wrap(window.ecnuVpn!.config.saveAuth(deproxy(body) ?? {})) as ApiResponse<T>
      case '/config/settings':
        return wrap(window.ecnuVpn!.config.saveSettings(deproxy(body) ?? {})) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },

  delete<T = unknown>(path: string, options?: { data?: any }): ApiResponse<T> {
    if (!desktopAvailable()) return httpApi.delete(path, { data: deproxy(options?.data) })

    switch (path) {
      case '/routes':
        return wrap(window.ecnuVpn!.routes.remove(deproxy(options?.data)?.cidr ?? '')) as ApiResponse<T>
      default:
        unsupported(path)
    }
  },
}

export default desktopApi