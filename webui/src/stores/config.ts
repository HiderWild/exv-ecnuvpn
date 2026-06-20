import { defineStore } from 'pinia'
import { ref } from 'vue'
import api from '../api/host'

export interface AuthConfig {
  server: string
  username: string
  // Empty string means "do not change the stored password". The UI uses
  // password_stored to know whether the backend currently holds an
  // encrypted password.
  password: string
  password_stored?: boolean
  user_agent: string
  remember_password: boolean
}

export interface SettingsConfig {
  mtu: number
  dtls: boolean
  extra_args: string
  log_path: string
  webui_port: number
  webui_host: string
  webui_enabled: boolean
  vpn_engine: 'native'
  windows_tunnel_driver: 'auto' | 'wintun' | 'tap'
  windows_tap_interface: string
  auto_reconnect: boolean
  retry_limit: number
  minimal_mode: boolean
  service_install_prompt_seen: boolean
  minimal_install_service_before_connect: boolean
}

export interface KeyStatus {
  available: boolean
  present: boolean
  status: string
}

export interface CoreInspection {
  state: string
  risk: 'unknown' | 'low' | 'medium' | 'high'
  pid?: number
  ipc_path?: string
}

export interface RuntimeStatus {
  mode: 'native'
  engine: 'native'
  available: boolean
  source: 'native'
  path: string
  version: string
  bundled_runtime_dir: string
  wintun_path?: string
  tap_installer_path?: string
  missing_what?: string
  recommended_action?: string
  effect_on_connect?: string
  wintun_missing?: boolean
  tap_missing?: boolean
}

export interface DriverStatus {
  preferred: 'auto' | 'wintun' | 'tap'
  tap_interface: string
  supported: boolean
  effective_driver?: 'wintun' | 'tap'
  wintun_bundled?: boolean
  wintun_path?: string
  wintun_adapters?: string[]
  wintun_missing?: boolean
  wintun_missing_reason?: string
  wintun_recommended_action?: string
  tap_installer_path?: string
  tap_can_install?: boolean
  tap_adapters?: string[]
  tap_available?: boolean
  tap_missing?: boolean
  tap_missing_reason?: string
  tap_recommended_action?: string
  effective_driver_status?: 'ready' | 'degraded' | 'unavailable'
}

export const useConfigStore = defineStore('config', () => {
  function readLocalBool(key: 'exv:minimal-mode', fallback: boolean) {
    if (typeof localStorage === 'undefined') return fallback
    if (key !== 'exv:minimal-mode') return fallback
    const value = localStorage.getItem('exv:minimal-mode')
    if (value === 'true') return true
    if (value === 'false') return false
    return fallback
  }

  function writeLocalBool(key: 'exv:minimal-mode', value: boolean) {
    if (typeof localStorage === 'undefined') return
    if (key !== 'exv:minimal-mode') return
    localStorage.setItem('exv:minimal-mode', value ? 'true' : 'false')
  }

  function applyFrontendLocalSettings(next: SettingsConfig) {
    return {
      ...next,
      minimal_mode: readLocalBool('exv:minimal-mode', next.minimal_mode),
    }
  }

  function persistFrontendLocalSettings(s: Partial<SettingsConfig>) {
    if (Object.prototype.hasOwnProperty.call(s, 'minimal_mode') && s.minimal_mode != null) {
      writeLocalBool('exv:minimal-mode', s.minimal_mode)
    }
  }

  const authConfig = ref<AuthConfig>({
    server: '',
    username: '',
    password: '',
    password_stored: false,
    user_agent: '',
    remember_password: true,
  })

  const settings = ref<SettingsConfig>({
    mtu: 1400,
    dtls: true,
    extra_args: '',
    log_path: '',
    webui_port: 18080,
    webui_host: '127.0.0.1',
    webui_enabled: true,
    vpn_engine: 'native',
    windows_tunnel_driver: 'auto',
    windows_tap_interface: '',
    auto_reconnect: true,
    retry_limit: -1,
    minimal_mode: readLocalBool('exv:minimal-mode', false),
    service_install_prompt_seen: false,
    minimal_install_service_before_connect: true,
  })

  const keyStatus = ref<KeyStatus>({ available: false, present: false, status: 'missing' })
  const runtimeStatus = ref<RuntimeStatus | null>(null)
  const driverStatus = ref<DriverStatus | null>(null)

  async function fetchAuthConfig() {
    try {
      const { data } = await api.get<AuthConfig>('/config/auth')
      authConfig.value = { ...authConfig.value, ...data }
    } catch (e) { console.error('[config] fetchAuthConfig failed:', e) }
  }

  async function saveAuthConfig(config: Partial<AuthConfig>) {
    const payload = {
      server: config.server ?? '',
      username: config.username ?? '',
      password: config.password ?? '',
      remember_password: config.remember_password ?? true,
      user_agent: config.user_agent ?? '',
    }
    const { data } = await api.put<AuthConfig>('/config/auth', payload)
    authConfig.value = { ...authConfig.value, ...data }
  }

  async function fetchSettings() {
    try {
      const { data } = await api.get<SettingsConfig>('/config/settings')
      settings.value = applyFrontendLocalSettings({ ...settings.value, ...data })
    } catch (e) { console.error('[config] fetchSettings failed:', e) }
  }

  async function saveSettings(s: Partial<SettingsConfig>) {
    const previous = settings.value
    settings.value = { ...settings.value, ...s }
    persistFrontendLocalSettings(s)
    const remoteSettings = { ...s }
    delete remoteSettings.minimal_mode
    if (Object.keys(remoteSettings).length === 0) return

    try {
      const { data } = await api.put<SettingsConfig>('/config/settings', remoteSettings)
      settings.value = applyFrontendLocalSettings({ ...settings.value, ...data })
    } catch (error) {
      settings.value = previous
      persistFrontendLocalSettings({
        minimal_mode: previous.minimal_mode,
      })
      throw error
    }
  }

  async function fetchKeyStatus() {
    try {
      const { data } = await api.get<KeyStatus>('/config/key')
      keyStatus.value = data
    } catch (e) { console.error('[config] fetchKeyStatus failed:', e) }
  }

  async function fetchRuntimeStatus() {
    const { data } = await api.get<RuntimeStatus>('/runtime')
    runtimeStatus.value = data
  }

  async function fetchDriverStatus() {
    const { data } = await api.get<DriverStatus>('/drivers')
    driverStatus.value = data
  }

  async function installDriver(driver: 'wintun' | 'tap') {
    const { data } = await api.post<{
      ok: boolean
      message: string
      takes_effect?: 'next_connect' | 'immediately'
      status: DriverStatus
    }>('/drivers/install', { driver })
    if (data.status) {
      driverStatus.value = data.status
    }
    return data
  }

  async function importConfig(payload: { format: 'protected' | 'unprotected'; data: string; password?: string }) {
    const { data } = await api.post<{ ok: true }>('/config/import', payload)
    await Promise.all([fetchAuthConfig(), fetchSettings()])
    return data
  }

  async function exportConfig(payload: { protected: boolean; password?: string }) {
    const { data } = await api.post<{ format: 'protected' | 'unprotected'; data: string }>(
      '/config/export', payload,
    )
    return data
  }

  async function resetConfig(confirm: boolean) {
    const { data } = await api.post<{ ok: true }>('/config/reset', { confirm })
    await Promise.all([fetchAuthConfig(), fetchSettings(), fetchKeyStatus()])
    return data
  }

  async function resetKey(confirm: boolean) {
    const { data } = await api.post<{ ok: true }>('/key/reset', { confirm })
    await fetchKeyStatus()
    return data
  }

  async function inspectCore() {
    const { data } = await api.get<CoreInspection>('/maintenance/core')
    return data
  }

  async function killStaleCore(confirm: boolean) {
    const { data } = await api.post<{ ok: true }>('/maintenance/core/kill', { confirm })
    return data
  }

  return {
    authConfig, settings, keyStatus, runtimeStatus, driverStatus,
    fetchAuthConfig, saveAuthConfig,
    fetchSettings, saveSettings,
    fetchKeyStatus,
    fetchRuntimeStatus, fetchDriverStatus, installDriver,
    importConfig, exportConfig, resetConfig,
    resetKey,
    inspectCore, killStaleCore,
  }
})
