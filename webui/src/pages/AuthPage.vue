<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useConfigStore, type AuthConfig } from '../stores/config'
import { Shield, User, Key, Fingerprint } from 'lucide-vue-next'

const config = useConfigStore()

const saving = ref(false)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)

const form = ref<AuthConfig>({
  server: '',
  username: '',
  password: '',
  user_agent: 'AnyConnect',
  remember_password: false,
})

onMounted(async () => {
  await config.fetchAuthConfig()
  form.value = { ...config.authConfig }
})

async function save() {
  saving.value = true
  message.value = null
  try {
    await config.saveAuthConfig(form.value)
    message.value = { type: 'success', text: 'Authentication settings saved' }
  } catch (e: any) {
    message.value = { type: 'error', text: e?.response?.data?.error || 'Failed to save' }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6 flex items-center gap-2">
      <Shield class="w-5 h-5 text-accent" />
      Authentication
    </h1>

    <div class="bg-surface border border-border rounded-xl p-6 max-w-2xl">
      <form @submit.prevent="save" class="space-y-5">
        <!-- Server -->
        <div>
          <label class="block text-sm text-muted mb-1.5">VPN Server</label>
          <div class="relative">
            <User class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.server"
              type="text"
              placeholder="vpn.ecnu.edu.cn"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
        </div>

        <!-- Username -->
        <div>
          <label class="block text-sm text-muted mb-1.5">Username</label>
          <div class="relative">
            <User class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.username"
              type="text"
              placeholder="Your ECNU username"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
        </div>

        <!-- Password -->
        <div>
          <label class="block text-sm text-muted mb-1.5">Password</label>
          <div class="relative">
            <Key class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.password"
              type="password"
              placeholder="Leave blank to enter on connect"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
        </div>

        <!-- Remember Password -->
        <div class="flex items-center gap-3">
          <input
            v-model="form.remember_password"
            type="checkbox"
            id="remember-password"
            class="w-4 h-4 rounded border-border accent-accent"
          />
          <label for="remember-password" class="text-sm text-foreground">
            Remember password (encrypted)
          </label>
        </div>

        <!-- User Agent -->
        <div>
          <label class="block text-sm text-muted mb-1.5">User Agent</label>
          <div class="relative">
            <Fingerprint class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.user_agent"
              type="text"
              placeholder="AnyConnect"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
          <p class="text-xs text-muted mt-1">Client identification string sent to the VPN server</p>
        </div>

        <!-- Save -->
        <div class="pt-2">
          <button
            type="submit"
            :disabled="saving"
            class="bg-accent text-white rounded-lg px-6 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
          >
            {{ saving ? 'Saving...' : 'Save Authentication Settings' }}
          </button>
        </div>

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
      </form>
    </div>
  </div>
</template>