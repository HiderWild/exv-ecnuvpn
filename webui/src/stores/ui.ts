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

  return {
    toasts, showConfirm, confirmMessage, confirmCallback,
    addToast, removeToast,
    requestConfirm, closeConfirm, onConfirm,
  }
})
