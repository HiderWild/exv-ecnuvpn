<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useConfigStore, type AuthConfig } from '../stores/config'
import { Shield, User, Key, Fingerprint } from 'lucide-vue-next'

const config = useConfigStore()

const saving = ref(false)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)

const form = ref<AuthConfig>({
  server: '',
  username: '',
  password: '',
  password_stored: false,
  user_agent: '',
  remember_password: true,
})

onMounted(async () => {
  await config.fetchAuthConfig()
  // Spread without copying back the dummy password value: the backend now
  // returns an empty string for password plus a password_stored boolean.
  form.value = {
    ...config.authConfig,
    password: '',
  }
})

const passwordPlaceholder = computed(() =>
  form.value.password_stored
    ? '留空表示保留原密码，输入新密码覆盖'
    : '请输入密码'
)

function extractErrorText(err: any): string {
  return (
    err?.response?.data?.error ||
    err?.message ||
    err?.toString?.() ||
    '保存失败'
  )
}

async function save() {
  saving.value = true
  message.value = null
  try {
    // Always send the form values; the backend treats an empty password as
    // "keep the existing one" and an empty user_agent as "no change".
    await config.saveAuthConfig(form.value)
    form.value.password = ''
    form.value.password_stored = config.authConfig.password_stored ?? form.value.password_stored
    message.value = { type: 'success', text: '认证设置已保存' }
  } catch (e: any) {
    message.value = { type: 'error', text: extractErrorText(e) }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6 flex items-center gap-2">
      <Shield class="w-5 h-5 text-accent" />
      认证设置
    </h1>

    <div class="bg-surface border border-border rounded-xl p-6 max-w-2xl">
      <form @submit.prevent="save" class="space-y-5">
        <!-- Server -->
        <div>
          <label class="block text-sm text-muted mb-1.5">VPN 服务器</label>
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
          <label class="block text-sm text-muted mb-1.5">用户名</label>
          <div class="relative">
            <User class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.username"
              type="text"
              placeholder="ECNU 用户名"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
        </div>

        <!-- Password -->
        <div>
          <label class="block text-sm text-muted mb-1.5">密码</label>
          <div class="relative">
            <Key class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.password"
              type="password"
              autocomplete="new-password"
              :placeholder="passwordPlaceholder"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
          <p v-if="form.password_stored" class="text-xs text-muted mt-1">
            已保存加密密码，仅在需要修改时输入。
          </p>
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
            记住密码（加密存储）
          </label>
        </div>

        <!-- User Agent -->
        <div>
          <label class="block text-sm text-muted mb-1.5">用户代理</label>
          <div class="relative">
            <Fingerprint class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted" />
            <input
              v-model="form.user_agent"
              type="text"
              placeholder="AnyConnect"
              class="w-full bg-background border border-border rounded-lg pl-10 pr-4 py-2.5 text-sm text-foreground placeholder:text-muted/50 focus:outline-none focus:border-accent"
            />
          </div>
          <p class="text-xs text-muted mt-1">发送到 VPN 服务器的客户端标识字符串</p>
        </div>

        <!-- Save -->
        <div class="pt-2">
          <button
            type="submit"
            :disabled="saving"
            class="bg-accent text-white rounded-lg px-6 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
          >
            {{ saving ? '保存中...' : '保存认证设置' }}
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