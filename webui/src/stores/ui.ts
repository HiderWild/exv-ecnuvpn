import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useConfigStore } from './config'

export interface ToastMessage {
  id: number
  text: string
  type: 'success' | 'error' | 'warning' | 'info'
}

export interface ErrorModalState {
  visible: boolean
  title: string
  message: string
  primaryLabel: string
  secondaryLabel: string
  onPrimary: (() => void | Promise<void>) | null
  onClose: (() => void | Promise<void>) | null
}

export const useUiStore = defineStore('ui', () => {
  const toasts = ref<ToastMessage[]>([])
  const showConfirm = ref(false)
  const confirmMessage = ref('')
  const confirmCallback = ref<(() => void) | null>(null)
  const errorModal = ref<ErrorModalState>({
    visible: false,
    title: '',
    message: '',
    primaryLabel: '重试',
    secondaryLabel: '取消',
    onPrimary: null,
    onClose: null,
  })
  const showPasswordPrompt = ref(false)
  const passwordPromptMessage = ref('')
  const passwordPromptResolver = ref<((value: string | null) => void) | null>(null)
  let nextId = 1

  function addToast(text: string, type: ToastMessage['type'] = 'info') {
    if (type === 'error') {
      requestError({ message: text })
      return
    }
    const id = nextId++
    toasts.value.push({ id, text, type })
    setTimeout(() => removeToast(id), 4000)
  }

  function removeToast(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id)
  }

  function requestConfirm(message: string, onConfirm: () => void) {
    const config = useConfigStore()
    if (config.settings.minimal_mode && window.ecnuVpn?.modal) {
      void window.ecnuVpn.modal.confirmPrompt(message).then((confirmed) => {
        if (confirmed) onConfirm()
      })
      return
    }
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

  function requestError(options: {
    title?: string
    message: string
    primaryLabel?: string
    secondaryLabel?: string
    onPrimary?: () => void | Promise<void>
    onClose?: () => void | Promise<void>
  }) {
    errorModal.value = {
      visible: true,
      title: options.title || '操作失败',
      message: options.message,
      primaryLabel: options.primaryLabel || '重试',
      secondaryLabel: options.secondaryLabel || '取消',
      onPrimary: options.onPrimary || null,
      onClose: options.onClose || null,
    }
  }

  function closeError() {
    const callback = errorModal.value.onClose
    errorModal.value.visible = false
    errorModal.value.onPrimary = null
    errorModal.value.onClose = null
    void callback?.()
  }

  function onErrorPrimary() {
    const callback = errorModal.value.onPrimary
    errorModal.value.visible = false
    errorModal.value.onPrimary = null
    errorModal.value.onClose = null
    void callback?.()
  }

  function requestPassword(message: string) {
    passwordPromptResolver.value?.(null)
    const config = useConfigStore()
    if (config.settings.minimal_mode && window.ecnuVpn?.modal) {
      return window.ecnuVpn.modal.passwordPrompt(message)
    }
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
    errorModal,
    showPasswordPrompt, passwordPromptMessage,
    addToast, removeToast,
    requestConfirm, closeConfirm, onConfirm,
    requestError, closeError, onErrorPrimary,
    requestPassword, submitPasswordPrompt, closePasswordPrompt,
  }
})
