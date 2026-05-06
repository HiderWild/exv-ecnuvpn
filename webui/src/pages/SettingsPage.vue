<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useConfigStore, type SettingsConfig } from '../stores/config'
import { Save } from 'lucide-vue-next'

const config = useConfigStore()
const saving = ref(false)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)

const form = ref<SettingsConfig>({
  mtu: 1400,
  dtls: true,
  extra_args: '',
  log_path: '',
  webui_port: 18080,
  webui_host: '127.0.0.1',
  webui_enabled: true,
})

onMounted(async () => {
  await config.fetchSettings()
  form.value = { ...config.settings }
})

async function save() {
  saving.value = true
  message.value = null
  try {
    await config.saveSettings(form.value)
    message.value = { type: 'success', text: 'Settings saved' }
  } catch (e: any) {
    message.value = { type: 'error', text: e?.response?.data?.error || 'Failed to save' }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">Settings</h1>

    <div class="space-y-6 max-w-2xl">
      <!-- Connection Settings -->
      <div class="bg-surface border border-border rounded-xl p-6">
        <h2 class="text-sm font-medium text-foreground mb-4">Connection</h2>
        <div class="space-y-4">
          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">MTU</label>
            <input
              v-model.number="form.mtu"
              type="number"
              min="576"
              max="1500"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>

          <div class="flex items-center justify-between py-1">
            <div>
              <p class="text-sm text-foreground">DTLS</p>
              <p class="text-xs text-muted">Enable DTLS encryption for better performance</p>
            </div>
            <input
              v-model="form.dtls"
              type="checkbox"
              class="w-4 h-4 rounded border-border accent-accent"
            />
          </div>

          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">Extra Arguments</label>
            <input
              v-model="form.extra_args"
              type="text"
              placeholder="Additional openconnect arguments"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>

          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">Log File Path</label>
            <input
              v-model="form.log_path"
              type="text"
              placeholder="~/.ecnuvpn/ecnuvpn.log"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>
        </div>
      </div>

      <!-- WebUI Settings -->
      <div class="bg-surface border border-border rounded-xl p-6">
        <h2 class="text-sm font-medium text-foreground mb-4">WebUI</h2>
        <div class="space-y-4">
          <div class="flex items-center justify-between py-1">
            <div>
              <p class="text-sm text-foreground">Enable WebUI</p>
              <p class="text-xs text-muted">Start WebUI automatically with VPN</p>
            </div>
            <input
              v-model="form.webui_enabled"
              type="checkbox"
              class="w-4 h-4 rounded border-border accent-accent"
            />
          </div>

          <div class="grid grid-cols-2 gap-4">
            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">Host</label>
              <input
                v-model="form.webui_host"
                type="text"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              />
            </div>
            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">Port</label>
              <input
                v-model.number="form.webui_port"
                type="number"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              />
            </div>
          </div>
        </div>
      </div>

      <!-- Save button -->
      <button
        :disabled="saving"
        class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
        @click="save"
      >
        <Save class="w-4 h-4" />
        {{ saving ? 'Saving...' : 'Save Settings' }}
      </button>

      <!-- Message -->
      <div
        v-if="message"
        :class="[
          'text-sm rounded-lg px-4 py-2.5',
          message.type === 'success' ? 'bg-green-500/10 text-green-400' : 'bg-red-500/10 text-red-400'
        ]"
      >
        {{ message.text }}
      </div>
    </div>
  </div>
</template>