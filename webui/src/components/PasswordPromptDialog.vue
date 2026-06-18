<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { Eye, KeyRound } from 'lucide-vue-next'
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
const password = ref('')
const error = ref('')
const revealing = ref(false)
const inputRef = ref<HTMLInputElement | null>(null)

watch(
  () => ui.showPasswordPrompt,
  async (visible) => {
    if (!visible) {
      password.value = ''
      error.value = ''
      revealing.value = false
      return
    }
    password.value = ''
    error.value = ''
    revealing.value = false
    await nextTick()
    inputRef.value?.focus()
  },
)

function submit() {
  if (!password.value) {
    error.value = '请输入验证内容'
    return
  }
  const value = password.value
  password.value = ''
  revealing.value = false
  ui.submitPasswordPrompt(value)
}

function cancel() {
  password.value = ''
  revealing.value = false
  ui.closePasswordPrompt()
}

function showPassword() {
  revealing.value = true
}

function hidePassword() {
  revealing.value = false
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="ui.showPasswordPrompt"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      @click.self="cancel"
    >
      <form
        class="w-full max-w-sm rounded-xl border border-border bg-surface p-6 shadow-xl mx-4"
        @submit.prevent="submit"
      >
        <div class="mb-4 flex items-start gap-3">
          <KeyRound class="mt-0.5 h-5 w-5 shrink-0 text-primary" />
          <div class="min-w-0">
            <p class="text-sm font-semibold text-foreground">{{ ui.passwordPromptMessage }}</p>
            <p class="mt-1 text-xs text-muted">输入内容仅用于本次验证，不会写入设置。</p>
          </div>
        </div>

        <div class="relative">
          <input
            ref="inputRef"
            v-model="password"
            :type="revealing ? 'text' : 'password'"
            autocomplete="one-time-code"
            class="w-full rounded-lg border border-border bg-bg px-3 py-2 pr-11 text-sm text-foreground outline-none transition-colors focus:border-primary"
            placeholder="密码或验证码"
            @input="error = ''"
            @keydown.esc.prevent="cancel"
          />
          <button
            type="button"
            class="absolute right-2 top-1/2 flex h-8 w-8 -translate-y-1/2 items-center justify-center rounded-md text-muted transition-colors hover:bg-surface/80 hover:text-foreground"
            title="按住显示密码"
            aria-label="按住显示密码"
            @pointerdown.prevent="showPassword"
            @pointerup="hidePassword"
            @pointercancel="hidePassword"
            @pointerleave="hidePassword"
            @blur="hidePassword"
            @keydown.space.prevent="showPassword"
            @keyup.space.prevent="hidePassword"
            @keydown.enter.prevent="showPassword"
            @keyup.enter.prevent="hidePassword"
          >
            <Eye class="h-4 w-4" />
          </button>
        </div>
        <p v-if="error" class="mt-2 text-xs text-destructive">{{ error }}</p>

        <div class="mt-5 flex justify-end gap-3">
          <button
            type="button"
            class="rounded-lg border border-border px-4 py-2 text-sm text-muted transition-colors hover:bg-surface/80"
            @click="cancel"
          >
            取消
          </button>
          <button
            type="submit"
            class="rounded-lg bg-primary px-4 py-2 text-sm text-white transition-colors hover:bg-primary/90"
          >
            连接
          </button>
        </div>
      </form>
    </div>
  </Teleport>
</template>
