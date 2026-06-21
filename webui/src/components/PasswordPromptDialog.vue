<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { Eye, KeyRound } from 'lucide-vue-next'
import ModalShell from './ModalShell.vue'
import { useUiStore } from '../stores/ui'

const props = withDefaults(defineProps<{
  compact?: boolean
}>(), {
  compact: false,
})
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
  <ModalShell
    :open="ui.showPasswordPrompt"
    :title="props.compact ? '输入口令' : ui.passwordPromptMessage"
    :description="props.compact ? '' : ui.passwordPromptDescription"
    :compact="props.compact"
    size="sm"
    @close="cancel"
  >
    <template #icon>
      <KeyRound class="h-4 w-4" />
    </template>

    <form
      id="legacy-password-prompt-form"
      :class="props.compact ? 'modal-compact-form' : 'space-y-3'"
      @submit.prevent="submit"
    >
      <div class="relative">
        <input
          ref="inputRef"
          v-model="password"
          :type="revealing ? 'text' : 'password'"
          autocomplete="one-time-code"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 pr-11 text-sm text-foreground outline-none transition-colors focus:border-primary"
          :placeholder="props.compact ? ui.passwordPromptMessage : '密码或验证码'"
          @input="error = ''"
          @keydown.esc.prevent="cancel"
        />
        <button
          v-if="!props.compact"
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
      <p v-if="error" :class="props.compact ? 'modal-compact-error' : 'text-xs text-destructive'">{{ error }}</p>
    </form>

    <template #actions>
      <button
        type="button"
        class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80"
        @click="cancel"
      >
        {{ ui.passwordPromptCancelLabel }}
      </button>
      <button
        type="submit"
        form="legacy-password-prompt-form"
        class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90"
      >
        {{ ui.passwordPromptSubmitLabel }}
      </button>
    </template>
  </ModalShell>
</template>
