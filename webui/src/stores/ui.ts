import { defineStore } from 'pinia'
import { ref } from 'vue'

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

export interface QuickStartRequest {
  reason: 'missing' | 'invalid'
  defaults: {
    server: string
    remember_password: boolean
    install_service: boolean
  }
}

export interface CredentialPromptRequest {
  missingUsername: boolean
  missingPassword: boolean
  username: string
  rememberPassword: boolean
  message?: string
}

export interface CredentialPromptResult {
  username?: string
  password?: string
  rememberPassword: boolean
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
  const passwordPromptDescription = ref('输入内容仅用于本次验证，不会写入设置。')
  const passwordPromptSubmitLabel = ref('确认')
  const passwordPromptCancelLabel = ref('取消')
  const passwordPromptResolver = ref<((value: string | null) => void) | null>(null)
  const showQuickStart = ref(false)
  const quickStartRequest = ref<QuickStartRequest | null>(null)
  const showCredentialPrompt = ref(false)
  const credentialPrompt = ref<CredentialPromptRequest | null>(null)
  const credentialPromptResolver = ref<((value: CredentialPromptResult | null) => void) | null>(null)
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

  function requestPassword(message: string, options?: {
    description?: string
    submitLabel?: string
    cancelLabel?: string
  }) {
    passwordPromptResolver.value?.(null)
    passwordPromptMessage.value = message
    passwordPromptDescription.value = options?.description || '输入内容仅用于本次验证，不会写入设置。'
    passwordPromptSubmitLabel.value = options?.submitLabel || '确认'
    passwordPromptCancelLabel.value = options?.cancelLabel || '取消'
    showPasswordPrompt.value = true
    return new Promise<string | null>((resolve) => {
      passwordPromptResolver.value = resolve
    })
  }

  function submitPasswordPrompt(password: string) {
    const resolver = passwordPromptResolver.value
    showPasswordPrompt.value = false
    passwordPromptMessage.value = ''
    passwordPromptDescription.value = '输入内容仅用于本次验证，不会写入设置。'
    passwordPromptSubmitLabel.value = '确认'
    passwordPromptCancelLabel.value = '取消'
    passwordPromptResolver.value = null
    resolver?.(password)
  }

  function closePasswordPrompt() {
    const resolver = passwordPromptResolver.value
    showPasswordPrompt.value = false
    passwordPromptMessage.value = ''
    passwordPromptDescription.value = '输入内容仅用于本次验证，不会写入设置。'
    passwordPromptSubmitLabel.value = '确认'
    passwordPromptCancelLabel.value = '取消'
    passwordPromptResolver.value = null
    resolver?.(null)
  }

  function openQuickStart(request: QuickStartRequest) {
    quickStartRequest.value = request
    showQuickStart.value = true
  }

  function closeQuickStart() {
    showQuickStart.value = false
    quickStartRequest.value = null
  }

  function requestCredentials(request: CredentialPromptRequest) {
    credentialPromptResolver.value?.(null)
    credentialPrompt.value = request
    showCredentialPrompt.value = true
    return new Promise<CredentialPromptResult | null>((resolve) => {
      credentialPromptResolver.value = resolve
    })
  }

  function submitCredentialPrompt(value: CredentialPromptResult) {
    const resolver = credentialPromptResolver.value
    showCredentialPrompt.value = false
    credentialPrompt.value = null
    credentialPromptResolver.value = null
    resolver?.(value)
  }

  function closeCredentialPrompt() {
    const resolver = credentialPromptResolver.value
    showCredentialPrompt.value = false
    credentialPrompt.value = null
    credentialPromptResolver.value = null
    resolver?.(null)
  }

  return {
    toasts, showConfirm, confirmMessage, confirmCallback,
    errorModal,
    showPasswordPrompt, passwordPromptMessage, passwordPromptDescription,
    passwordPromptSubmitLabel, passwordPromptCancelLabel,
    showQuickStart, quickStartRequest,
    showCredentialPrompt, credentialPrompt,
    addToast, removeToast,
    requestConfirm, closeConfirm, onConfirm,
    requestError, closeError, onErrorPrimary,
    requestPassword, submitPasswordPrompt, closePasswordPrompt,
    openQuickStart, closeQuickStart,
    requestCredentials, submitCredentialPrompt, closeCredentialPrompt,
  }
})
