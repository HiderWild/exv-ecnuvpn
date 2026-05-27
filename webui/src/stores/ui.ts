import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface ToastMessage {
  id: number
  text: string
  type: 'success' | 'error' | 'warning' | 'info'
}

export const useUiStore = defineStore('ui', () => {
  const toasts = ref<ToastMessage[]>([])
  const showConfirm = ref(false)
  const confirmMessage = ref('')
  const confirmCallback = ref<(() => void) | null>(null)
  const showPasswordPrompt = ref(false)
  const passwordPromptMessage = ref('')
  const passwordPromptResolver = ref<((value: string | null) => void) | null>(null)
  let nextId = 1

  function addToast(text: string, type: ToastMessage['type'] = 'info') {
    const id = nextId++
    toasts.value.push({ id, text, type })
    setTimeout(() => removeToast(id), 4000)
  }

  function removeToast(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id)
  }

  function requestConfirm(message: string, onConfirm: () => void) {
    confirmMessage.value = message
    confirmCallback.value = onConfirm
    showConfirm.value = true
  }

  function closeConfirm() {
    showConfirm.value = false
    confirmCallback.value = null
  }

  function onConfirm() {
    confirmCallback.value?.()
    closeConfirm()
  }

  function requestPassword(message: string) {
    passwordPromptResolver.value?.(null)
    passwordPromptMessage.value = message
    showPasswordPrompt.value = true
    return new Promise<string | null>((resolve) => {
      passwordPromptResolver.value = resolve
    })
  }

  function submitPasswordPrompt(password: string) {
    const resolver = passwordPromptResolver.value
    showPasswordPrompt.value = false
    passwordPromptMessage.value = ''
    passwordPromptResolver.value = null
    resolver?.(password)
  }

  function closePasswordPrompt() {
    const resolver = passwordPromptResolver.value
    showPasswordPrompt.value = false
    passwordPromptMessage.value = ''
    passwordPromptResolver.value = null
    resolver?.(null)
  }

  return {
    toasts, showConfirm, confirmMessage, confirmCallback,
    showPasswordPrompt, passwordPromptMessage,
    addToast, removeToast,
    requestConfirm, closeConfirm, onConfirm,
    requestPassword, submitPasswordPrompt, closePasswordPrompt,
  }
})
