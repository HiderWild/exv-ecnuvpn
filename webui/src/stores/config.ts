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
  vpn_engine: 'native' | 'legacy_openconnect'
  openconnect_runtime: 'bundled' | 'system' | 'auto'
  windows_tunnel_driver: 'auto' | 'wintun' | 'tap'
  windows_tap_interface: string
  auto_reconnect: boolean
  minimal_mode: boolean
  service_install_prompt_seen: boolean
  minimal_install_service_before_connect: boolean
}

export interface KeyStatus {
  present: boolean
  fingerprint: string | null
  status: string
}

export interface RuntimeStatus {
  mode: string
  engine?: 'native' | 'legacy_openconnect'
  available: boolean
  source: 'native' | 'bundled' | 'system' | 'missing' | 'custom'
  path: string
  bundled_path?: string
  system_path?: string
  version: string
  bundled_runtime_dir: string
  wintun_path?: string
  tap_installer_path?: string
  missing_what?: string
  recommended_action?: string
  effect_on_connect?: string
  wintun_missing?: boolean
  tap_missing?: boolean
  legacy_openconnect?: RuntimeStatus
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
    openconnect_runtime: 'bundled',
    windows_tunnel_driver: 'auto',
    windows_tap_interface: '',
    auto_reconnect: true,
    minimal_mode: false,
    service_install_prompt_seen: false,
    minimal_install_service_before_connect: true,
  })

  const keyStatus = ref<KeyStatus>({ present: false, fingerprint: null, status: 'missing' })
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
      settings.value = { ...settings.value, ...data }
    } catch (e) { console.error('[config] fetchSettings failed:', e) }
  }

  async function saveSettings(s: Partial<SettingsConfig>) {
    if (typeof s.minimal_mode === 'boolean') {
      settings.value = { ...settings.value, minimal_mode: s.minimal_mode }
      if (typeof window !== 'undefined' && window.ecnuVpn?.window) {
        void window.ecnuVpn.window.setMode(s.minimal_mode ? 'minimal' : 'advanced')
      }
    }
    const { data } = await api.put<SettingsConfig>('/config/settings', { ...s })
    settings.value = { ...settings.value, ...data }
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

  return {
    authConfig, settings, keyStatus, runtimeStatus, driverStatus,
    fetchAuthConfig, saveAuthConfig,
    fetchSettings, saveSettings,
    fetchKeyStatus,
    fetchRuntimeStatus, fetchDriverStatus, installDriver,
  }
})
