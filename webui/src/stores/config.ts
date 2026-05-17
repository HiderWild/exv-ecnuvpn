import { defineStore } from 'pinia'
import { ref } from 'vue'
import api from '../api/desktop'

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
  openconnect_runtime: 'bundled' | 'system' | 'auto'
}

export interface HelperStatus {
  installed: boolean
  running: boolean
  available: boolean
  socket_path: string
  label: string
}

export interface KeyStatus {
  present: boolean
  fingerprint: string | null
  status: string
}

export interface RuntimeStatus {
  mode: string
  available: boolean
  source: 'bundled' | 'system' | 'missing'
  path: string
  bundled_path: string
  system_path: string
  version: string
  bundled_runtime_dir: string
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
    openconnect_runtime: 'bundled',
  })

  const keyStatus = ref<KeyStatus>({ present: false, fingerprint: null, status: 'missing' })
  const runtimeStatus = ref<RuntimeStatus | null>(null)
  const helperStatus = ref<HelperStatus | null>(null)

  async function fetchAuthConfig() {
    try {
      const { data } = await api.get<AuthConfig>('/config/auth')
      authConfig.value = { ...authConfig.value, ...data }
    } catch (e) { console.error('[config] fetchAuthConfig failed:', e) }
  }

  async function saveAuthConfig(config: Partial<AuthConfig>) {
    const { data } = await api.put<AuthConfig>('/config/auth', config)
    authConfig.value = { ...authConfig.value, ...data }
  }

  async function fetchSettings() {
    try {
      const { data } = await api.get<SettingsConfig>('/config/settings')
      settings.value = { ...settings.value, ...data }
    } catch (e) { console.error('[config] fetchSettings failed:', e) }
  }

  async function saveSettings(s: Partial<SettingsConfig>) {
    const { data } = await api.put<SettingsConfig>('/config/settings', s)
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

  async function fetchHelperStatus() {
    const { data } = await api.get<HelperStatus>('/helper')
    helperStatus.value = data
  }

  return {
    authConfig, settings, keyStatus, runtimeStatus, helperStatus,
    fetchAuthConfig, saveAuthConfig,
    fetchSettings, saveSettings,
    fetchKeyStatus,
    fetchRuntimeStatus, fetchHelperStatus,
  }
})
