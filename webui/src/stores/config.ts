import { defineStore } from 'pinia'
import { ref } from 'vue'
import api from '../api/client'

export interface AuthConfig {
  server: string
  username: string
  password: string
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
}

export interface KeyStatus {
  present: boolean
  fingerprint: string | null
  status: string
}

export const useConfigStore = defineStore('config', () => {
  const authConfig = ref<AuthConfig>({
    server: '',
    username: '',
    password: '',
    user_agent: 'AnyConnect',
    remember_password: false,
  })

  const settings = ref<SettingsConfig>({
    mtu: 1400,
    dtls: true,
    extra_args: '',
    log_path: '',
    webui_port: 18080,
    webui_host: '127.0.0.1',
    webui_enabled: true,
  })

  const keyStatus = ref<KeyStatus>({ present: false, fingerprint: null, status: 'missing' })

  async function fetchAuthConfig() {
    try {
      const { data } = await api.get('/config/auth')
      authConfig.value = { ...authConfig.value, ...data }
    } catch {}
  }

  async function saveAuthConfig(config: Partial<AuthConfig>) {
    const { data } = await api.put('/config/auth', config)
    authConfig.value = { ...authConfig.value, ...data }
  }

  async function fetchSettings() {
    try {
      const { data } = await api.get('/config/settings')
      settings.value = { ...settings.value, ...data }
    } catch {}
  }

  async function saveSettings(s: Partial<SettingsConfig>) {
    const { data } = await api.put('/config/settings', s)
    settings.value = { ...settings.value, ...data }
  }

  async function fetchKeyStatus() {
    try {
      const { data } = await api.get('/config/key')
      keyStatus.value = data
    } catch {}
  }

  return {
    authConfig, settings, keyStatus,
    fetchAuthConfig, saveAuthConfig,
    fetchSettings, saveSettings,
    fetchKeyStatus,
  }
})