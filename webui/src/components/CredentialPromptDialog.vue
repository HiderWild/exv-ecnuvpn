<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { Eye, KeyRound } from 'lucide-vue-next'
import ModalShell from './ModalShell.vue'
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
const username = ref('')
const password = ref('')
const rememberPassword = ref(true)
const error = ref('')
const revealing = ref(false)
const usernameRef = ref<HTMLInputElement | null>(null)
const passwordRef = ref<HTMLInputElement | null>(null)

watch(
  () => ui.showCredentialPrompt,
  async (visible) => {
    if (!visible || !ui.credentialPrompt) {
      username.value = ''
      password.value = ''
      error.value = ''
      revealing.value = false
      return
    }
    username.value = ui.credentialPrompt.username
    password.value = ''
    rememberPassword.value = ui.credentialPrompt.rememberPassword
    error.value = ''
    revealing.value = false
    await nextTick()
    if (ui.credentialPrompt.missingUsername) {
      usernameRef.value?.focus()
    } else {
      passwordRef.value?.focus()
    }
  },
)

function submit() {
  const request = ui.credentialPrompt
  if (!request) return
  if (request.missingUsername && !username.value.trim()) {
    error.value = '请填写用户名'
    usernameRef.value?.focus()
    return
  }
  if (request.missingPassword && !password.value) {
    error.value = '请填写密码'
    passwordRef.value?.focus()
    return
  }
  ui.submitCredentialPrompt({
    username: request.missingUsername ? username.value.trim() : undefined,
    password: request.missingPassword ? password.value : undefined,
    rememberPassword: rememberPassword.value,
  })
}

function cancel() {
  ui.closeCredentialPrompt()
}
</script>

<template>
  <ModalShell
    :open="ui.showCredentialPrompt"
    title="补全连接凭据"
    :description="ui.credentialPrompt?.message || '连接前需要补全缺少的认证信息。'"
    size="sm"
    @close="cancel"
  >
    <template #icon>
      <KeyRound class="h-4 w-4" />
    </template>

    <form class="space-y-3" @submit.prevent="submit">
      <label v-if="ui.credentialPrompt?.missingUsername" class="block">
        <span class="mb-1 block text-xs font-medium text-muted">用户名</span>
        <input
          ref="usernameRef"
          v-model="username"
          autocomplete="username"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none focus:border-primary"
          @input="error = ''"
          @keydown.esc.prevent="cancel"
        />
      </label>

      <label v-if="ui.credentialPrompt?.missingPassword" class="block">
        <span class="mb-1 block text-xs font-medium text-muted">密码</span>
        <span class="relative block">
          <input
            ref="passwordRef"
            v-model="password"
            :type="revealing ? 'text' : 'password'"
            autocomplete="current-password"
            class="w-full rounded-lg border border-border bg-bg px-3 py-2 pr-11 text-sm text-foreground outline-none focus:border-primary"
            @input="error = ''"
            @keydown.esc.prevent="cancel"
          />
          <button
            type="button"
            class="absolute right-2 top-1/2 grid h-8 w-8 -translate-y-1/2 place-items-center rounded-md text-muted hover:bg-surface/80 hover:text-foreground"
            title="按住显示密码"
            aria-label="按住显示密码"
            @pointerdown.prevent="revealing = true"
            @pointerup="revealing = false"
            @pointercancel="revealing = false"
            @pointerleave="revealing = false"
            @blur="revealing = false"
          >
            <Eye class="h-4 w-4" />
          </button>
        </span>
      </label>

      <label v-if="ui.credentialPrompt?.missingPassword" class="flex items-center gap-2 text-xs text-muted">
        <input
          v-model="rememberPassword"
          type="checkbox"
          class="h-4 w-4 rounded border-border bg-bg text-primary focus:ring-primary/40"
        />
        记住密码
      </label>

      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>

      <div class="flex justify-end gap-2 pt-1">
        <button type="button" class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80" @click="cancel">
          取消
        </button>
        <button type="submit" class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90">
          连接
        </button>
      </div>
    </form>
  </ModalShell>
</template>
