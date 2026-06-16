import { computed, nextTick, ref } from 'vue'
import { defineStore } from 'pinia'
import api from '../api/host'
import { useUiStore } from './ui'
import { useConfigStore } from './config'

export interface UpstreamVirtualAdapter {
  name: string
  detail: string
  kind?: string
  role?: string
  if_index?: string
  route_reason?: string
}

export interface VpnStatus {
  connected: boolean
  process_running?: boolean
  server: string
  username: string
  pid: number
  supervisor_pid: number
  network_ready: boolean
  interface: string
  internal_ip: string
  route_count: number
  mtu: number
  uptime_seconds: number
  rx_bytes: number
  tx_bytes: number
  upstream_virtual_detected: boolean
  upstream_virtual_adapters: UpstreamVirtualAdapter[]
  upstream_virtual_message: string
  route_policy: string
  log_path?: string
  mode?: 'helper' | 'direct' | 'elevated' | 'disconnected'
  backend?: unknown
}

export interface RouteEntry {
  cidr: string
}

export interface LogEntry {
  timestamp: string
  level: 'info' | 'warn' | 'error' | 'debug'
  message: string
}

export interface ServiceStatus {
  installed: boolean
  running: boolean
  path: string
  available: boolean
  capabilities?: {
    service_mode?: boolean
    oneshot_mode?: boolean
    temporary_connect?: boolean
    direct_fallback?: boolean
    helper_binary?: boolean
  }
  mode?: string
  endpoint?: string
  label?: string
  binary_path?: string
  service_state?: number
  warning?: string
}

export interface ServiceProgressEntry {
  command: 'install' | 'uninstall'
  message: string
  timestamp: string
}

export interface CliInstallStatus {
  installed: boolean
  installPath: string
  targetPath: string
  availableInPath: boolean
  warning?: string
}

export interface ConnectionProgressStage {
  key: string
  label: string
  description: string
}

export type VpnErrorType =
  | 'elevation_required'
  | 'elevation_cancelled'
  | 'elevation_denied'
  | 'runtime_missing'
  | 'config_invalid'
  | 'service_missing'
  | 'auth_failed'
  | 'tls_verify_failed'
  | 'wintun_missing'
  | 'utun_permission_denied'
  | 'unsupported_dtls'
  | 'permission_denied'
  | 'helper_unavailable'
  | 'network_unreachable'
  | 'user_cancelled'
  | 'invalid_request'
  | 'connection_failed'
  | 'native_failure'
  | 'parse_failure'
  | 'unknown_action'

export interface VpnError {
  ok: false
  error_type: VpnErrorType
  message: string
  recoverable: boolean
  recommended_action: string
  timestamp?: number
}

export type ConnectMode = 'helper' | 'elevated' | 'direct'

export type DashboardState =
  | 'service-ready disconnected'
  | 'service-missing disconnected'
  | 'helper connected'
  | 'direct connected'
  | 'elevated connected'
  | 'elevated connecting'
  | 'error recoverable'
  | 'error blocking'
  | 'loading'

export interface DashboardAction {
  label: string
  action: () => void
  variant?: 'primary' | 'secondary' | 'destructive'
}

export function isVpnError(data: unknown): data is VpnError {
  return data != null && typeof data === 'object' && 'error_type' in (data as object)
}

function isElevationCancelledMessage(message: string) {
  const normalized = message.toLowerCase()
  return message.includes('用户已取消') ||
    message.includes('使用者已取消') ||
    message.includes('操作已取消') ||
    normalized.includes('user canceled') ||
    normalized.includes('user cancelled') ||
    normalized.includes('operation was canceled') ||
    normalized.includes('operation was cancelled') ||
    normalized.includes('cancelled by the user') ||
    normalized.includes('canceled by the user') ||
    normalized.includes('error 1223') ||
    message.includes('(-128)')
}

function isAuthFailureMessage(message: string) {
  const normalized = message.toLowerCase()
  return normalized.includes('auth_failed') ||
    normalized.includes('login failed') ||
    normalized.includes('authentication failed') ||
    normalized.includes('invalid password') ||
    normalized.includes('password is incorrect')
}

type NativeErrorDescriptor = {
  error_type: VpnErrorType
  message: string
  recommended_action: string
  recoverable: boolean
}

// Single source of truth: every canonical backend code maps to one user-facing
// descriptor. The backend (feedback module) always sends `code`, `recoverable`
// and `recommended_action`; this table supplies the localized label and a
// sensible default action when the backend omits one.
const contractErrorMap: Record<string, NativeErrorDescriptor> = {
  auth_failed: {
    error_type: 'auth_failed',
    message: 'VPN 密码错误，请重新输入密码。',
    recommended_action: 'retry_password',
    recoverable: true,
  },
  tls_verify_failed: {
    error_type: 'tls_verify_failed',
    message: '无法验证 VPN 服务器证书，连接已中止。',
    recommended_action: 'check_server_certificate',
    recoverable: true,
  },
  wintun_missing: {
    error_type: 'wintun_missing',
    message: '缺少 Wintun 网络驱动，无法建立隧道。请安装网络驱动后重试。',
    recommended_action: 'install_wintun_driver',
    recoverable: true,
  },
  utun_permission_denied: {
    error_type: 'utun_permission_denied',
    message: '没有创建虚拟网卡的权限，请以管理员身份运行后重试。',
    recommended_action: 'retry_with_elevation',
    recoverable: true,
  },
  unsupported_dtls: {
    error_type: 'unsupported_dtls',
    message: '服务器要求的 DTLS 模式暂不受支持，已回退到 TLS。',
    recommended_action: 'retry_connection',
    recoverable: false,
  },
  permission_denied: {
    error_type: 'permission_denied',
    message: '需要管理员权限才能继续，请在弹出的授权窗口中点击允许。',
    recommended_action: 'retry_with_elevation',
    recoverable: true,
  },
  helper_unavailable: {
    error_type: 'helper_unavailable',
    message: 'VPN 助手服务不可用，请启动或重新安装助手后重试。',
    recommended_action: 'reinstall_helper',
    recoverable: true,
  },
  network_unreachable: {
    error_type: 'network_unreachable',
    message: '无法连接到 VPN 服务器，请检查网络连接或服务器地址。',
    recommended_action: 'check_network',
    recoverable: true,
  },
  user_cancelled: {
    error_type: 'user_cancelled',
    message: '操作已取消。',
    recommended_action: '',
    recoverable: true,
  },
  invalid_request: {
    error_type: 'invalid_request',
    message: '请求无效，请检查配置后重试。',
    recommended_action: '',
    recoverable: false,
  },
  connection_failed: {
    error_type: 'connection_failed',
    message: '连接失败，请打开日志（exv logs）查看详细原因后重试。',
    recommended_action: 'view_logs',
    recoverable: true,
  },
}

export function normalizeError(raw: unknown): VpnError {
  if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>
    if (typeof obj.error_type === 'string') {
      return {
        ok: false,
        error_type: obj.error_type as VpnErrorType,
        message: String(obj.message || 'Operation failed'),
        recoverable: obj.recoverable !== undefined ? !!obj.recoverable : true,
        recommended_action: String(obj.recommended_action || ''),
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : undefined,
      }
    }
    // Canonical backend code is the single source of truth. The feedback module
    // guarantees a non-empty code plus recoverable/recommended_action; this
    // table only supplies a localized label and default action fallback.
    if (typeof obj.code === 'string' && obj.code in contractErrorMap) {
      const descriptor = contractErrorMap[obj.code]
      return {
        ok: false,
        error_type: descriptor.error_type,
        message: String(obj.message || obj.error || descriptor.message),
        recoverable: obj.recoverable !== undefined ? !!obj.recoverable : descriptor.recoverable,
        recommended_action: String(obj.recommended_action || descriptor.recommended_action),
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : Date.now(),
      }
    }
    if (obj.ok === false && obj.message) {
      return {
        ok: false,
        error_type: 'native_failure',
        message: String(obj.message),
        recoverable: true,
        recommended_action: 'Retry the operation',
        timestamp: typeof obj.timestamp === 'number' ? obj.timestamp : undefined,
      }
    }
  }

  const message = raw instanceof Error ? raw.message : raw ? String(raw) : 'Unknown error'
  if (isElevationCancelledMessage(message)) {
    return {
      ok: false,
      error_type: 'elevation_cancelled',
      message: '提权失败：用户已取消授权。',
      recoverable: true,
      recommended_action: '',
      timestamp: Date.now(),
    }
  }
  if (isAuthFailureMessage(message)) {
    return {
      ok: false,
      error_type: 'auth_failed',
      message: 'VPN 密码错误，请重新输入密码。',
      recoverable: true,
      recommended_action: 'retry_password',
      timestamp: Date.now(),
    }
  }
  return {
    ok: false,
    error_type: message.includes('elevation_denied') ? 'elevation_denied' : 'native_failure',
    message,
    recoverable: true,
    recommended_action: 'Retry the operation',
    timestamp: Date.now(),
  }
}

function summarizeError(message: string) {
  const normalized = message
    .replace(/^Error invoking remote method '[^']+':\s*/i, '')
    .replace(/^Error:\s*/i, '')
    .trim()
  if (!normalized) return '操作失败，请查看日志。'
  return normalized.length > 96 ? `${normalized.slice(0, 96)}...` : normalized
}

export const useVpnStore = defineStore('vpn', () => {
  let progressTimer: ReturnType<typeof setInterval> | null = null
  let uptimeTimer: ReturnType<typeof setInterval> | null = null
  const ui = useUiStore()
  const config = useConfigStore()

  const status = ref<VpnStatus | null>(null)
  const routes = ref<RouteEntry[]>([])
  const logs = ref<LogEntry[]>([])
  const serviceStatus = ref<ServiceStatus | null>(null)
  const cliStatus = ref<CliInstallStatus | null>(null)
  const serviceProgress = ref<ServiceProgressEntry[]>([])
  const serviceBusy = ref(false)
  const serviceOverlayOperation = ref<'install' | 'uninstall' | null>(null)
  const serviceOperation = ref<'install' | 'uninstall' | null>(null)
  const cliOperation = ref<'install' | 'uninstall' | null>(null)
  const loading = ref(false)
  const lastError = ref<string | null>(null)
  const lastErrorType = ref<VpnErrorType | null>(null)
  const lastRecoverable = ref(true)
  const lastRecommendedAction = ref('')
  const lastErrorTime = ref<number | null>(null)
  const lastActionWasElevatedConnect = ref(false)
  const lastFailedConnectMode = ref<'helper' | 'elevated' | null>(null)
  const lastMutatingAction = ref<(() => Promise<unknown>) | null>(null)
  const activeTemporaryBackend = ref<unknown | null>(null)
  const connectInFlight = ref(false)
  const disconnectInFlight = ref(false)
  const connectionProgressStartedAt = ref<number | null>(null)
  const connectionProgressStageOffset = ref(0)
  const connectionProgressMaxIndex = ref(0)
  const progressTick = ref(0)
  const uptimeBaseSeconds = ref(0)
  const uptimeStartedAt = ref<number | null>(null)
  const uptimeTick = ref(0)

  const connectionProgressStages: ConnectionProgressStage[] = [
    {
      key: 'authorization',
      label: '等待授权',
      description: '请在系统弹窗中确认本次提权请求',
    },
    {
      key: 'oneshot-helper',
      label: '正在启动临时 helper',
      description: '授权通过后会创建本次连接专用的本地控制通道',
    },
    {
      key: 'vpn-server',
      label: '正在连接 VPN 服务器',
      description: '正在启动 VPN 引擎并完成认证握手',
    },
    {
      key: 'adapter',
      label: '正在创建虚拟网卡',
      description: '正在准备 Wintun/TAP 隧道接口',
    },
    {
      key: 'routes',
      label: '正在写入路由',
      description: '正在配置校园网路由和本机接口地址',
    },
    {
      key: 'network-ready',
      label: '等待网络就绪',
      description: '正在确认内网地址、接口和路由已经生效',
    },
  ]

  const serviceInstalled = computed(() => serviceStatus.value?.installed ?? false)
  const serviceRunning = computed(() => serviceStatus.value?.running ?? false)
  const serviceAvailable = computed(() => serviceStatus.value?.available ?? false)
  const isDesktop = computed(() => typeof window !== 'undefined' && !!window.ecnuVpn)
  const canUseElevatedFallback = computed(() => {
    const capabilities = serviceStatus.value?.capabilities
    return isDesktop.value && Boolean(
      capabilities?.temporary_connect || capabilities?.oneshot_mode,
    )
  })
  const recommendedConnectMode = computed<ConnectMode>(() => {
    if (serviceAvailable.value) return 'helper'
    if (canUseElevatedFallback.value) return 'elevated'
    return 'helper'
  })
  const currentSessionMode = computed(() => {
    if (!status.value?.connected) return 'disconnected'
    if (status.value.mode) return status.value.mode
    if (activeTemporaryBackend.value || !serviceRunning.value) return 'elevated'
    return 'helper'
  })

  async function showServiceOverlay(operation: 'install' | 'uninstall') {
    serviceOverlayOperation.value = operation
    await nextTick()
    await new Promise<void>((resolve) => {
      if (typeof requestAnimationFrame === 'function') {
        requestAnimationFrame(() => resolve())
      } else {
        setTimeout(resolve, 0)
      }
    })
  }

  const displayUptimeSeconds = computed(() => {
    void uptimeTick.value
    if (!status.value?.connected || !uptimeStartedAt.value) return 0
    const elapsed = Math.max(0, Math.floor((Date.now() - uptimeStartedAt.value) / 1000))
    return uptimeBaseSeconds.value + elapsed
  })

  function startUptimeTimer() {
    if (uptimeTimer) return
    uptimeTimer = setInterval(() => {
      uptimeTick.value++
    }, 1000)
  }

  function stopUptimeTimer() {
    if (uptimeTimer) {
      clearInterval(uptimeTimer)
      uptimeTimer = null
    }
  }

  function syncUptime(nextStatus: VpnStatus) {
    if (!nextStatus.connected) {
      uptimeBaseSeconds.value = 0
      uptimeStartedAt.value = null
      stopUptimeTimer()
      return
    }

    const reported = Math.max(0, nextStatus.uptime_seconds || 0)
    if (!uptimeStartedAt.value) {
      uptimeBaseSeconds.value = reported
      uptimeStartedAt.value = Date.now()
    } else if (reported > displayUptimeSeconds.value) {
      uptimeBaseSeconds.value = reported
      uptimeStartedAt.value = Date.now()
    }
    startUptimeTimer()
  }

  function applyStatus(nextStatus: VpnStatus) {
    status.value = nextStatus
    syncUptime(nextStatus)
  }

  const connectionProgress = computed<ConnectionProgressStage>(() => {
    void progressTick.value
    if (!connectionProgressStartedAt.value) return connectionProgressStages[0]

    const elapsed = Date.now() - connectionProgressStartedAt.value
    const elapsedStage = elapsed < 1500
      ? 0
      : elapsed < 3500
        ? 1
        : elapsed < 5500
          ? 2
          : elapsed < 7500
            ? 3
            : elapsed < 9500
              ? 4
              : 5
    const stageIndex = Math.min(
      connectionProgressStages.length - 1,
      connectionProgressMaxIndex.value,
      connectionProgressStageOffset.value + elapsedStage,
    )
    return connectionProgressStages[stageIndex]
  })

  const dashboardState = computed<DashboardState>(() => {
    if (lastError.value && lastErrorType.value) {
      if (lastErrorType.value !== 'elevation_required' && lastErrorType.value !== 'service_missing') {
        if (lastErrorType.value === 'runtime_missing') return 'error blocking'
        return lastRecoverable.value ? 'error recoverable' : 'error blocking'
      }
    }

    if (connectInFlight.value) return 'elevated connecting'

    if (status.value?.connected) {
      const mode = currentSessionMode.value
      if (mode === 'helper') return 'helper connected'
      if (mode === 'elevated') return 'elevated connected'
      return 'direct connected'
    }

    if (serviceAvailable.value) return 'service-ready disconnected'
    return 'service-missing disconnected'
  })

  const recoverableErrorAction = computed<DashboardAction | null>(() => {
    switch (lastErrorType.value) {
      case 'elevation_denied':
        return { label: '安装服务', action: () => installService(), variant: 'primary' }
      case 'config_invalid':
        return { label: '前往设置', action: () => {}, variant: 'primary' }
      case 'auth_failed':
        return { label: '重新输入密码', action: () => retryLastAction(), variant: 'primary' }
      case 'native_failure':
      case 'parse_failure':
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
      default:
        return { label: '重试', action: () => retryLastAction(), variant: 'primary' }
    }
  })

  const dashboardPrimaryAction = computed<DashboardAction | null>(() => {
    switch (dashboardState.value) {
      case 'service-ready disconnected':
        return { label: '连接', action: () => connect(), variant: 'primary' }
      case 'service-missing disconnected':
        return { label: '安装服务（推荐）', action: () => installService(), variant: 'primary' }
      case 'helper connected':
        return { label: '断开连接', action: () => disconnect(), variant: 'destructive' }
      case 'direct connected':
      case 'elevated connected':
        return { label: '断开连接', action: () => disconnectElevated(), variant: 'destructive' }
      case 'elevated connecting':
        return null
      case 'error recoverable':
        return recoverableErrorAction.value
      case 'error blocking':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  const dashboardSecondaryAction = computed<DashboardAction | null>(() => {
    switch (dashboardState.value) {
      case 'service-missing disconnected':
        return canUseElevatedFallback.value
          ? { label: '仅本次连接', action: () => connectElevated(), variant: 'secondary' }
          : null
      case 'direct connected':
      case 'elevated connected':
        return { label: '安装服务', action: () => installService(), variant: 'secondary' }
      case 'error recoverable':
        return { label: '关闭', action: () => clearError(), variant: 'secondary' }
      default:
        return null
    }
  })

  function errorPresentation(err: VpnError) {
    switch (err.error_type) {
      case 'auth_failed':
        return {
          title: '密码错误',
          primaryLabel: '重新输入密码',
          onPrimary: () => retryConnectAfterAuthFailure(lastFailedConnectMode.value ?? 'helper'),
        }
      case 'config_invalid':
        return {
          title: '配置不完整',
          primaryLabel: '前往设置',
          onPrimary: () => {
            clearError()
            window.location.hash = '#/settings'
          },
        }
      case 'runtime_missing':
        return {
          title: '缺少运行时',
          primaryLabel: '打开设置',
          onPrimary: () => {
            clearError()
            window.location.hash = '#/settings#settings-system'
          },
        }
      case 'elevation_denied':
        return {
          title: '授权被拒绝',
          primaryLabel: '重试',
          onPrimary: () => retryLastAction(),
        }
      case 'tls_verify_failed':
        return {
          title: '证书验证失败',
          primaryLabel: '重试',
          onPrimary: () => retryLastAction(),
        }
      case 'wintun_missing':
        return {
          title: '缺少网络驱动',
          primaryLabel: '重试',
          onPrimary: () => retryLastAction(),
        }
      case 'utun_permission_denied':
        return {
          title: '权限不足',
          primaryLabel: '重试',
          onPrimary: () => retryLastAction(),
        }
      case 'unsupported_dtls':
        return {
          title: '协议不受支持',
          primaryLabel: '重试',
          onPrimary: () => retryLastAction(),
        }
      default:
        return {
          title: err.recoverable ? '操作失败' : '严重错误',
          primaryLabel: err.recoverable ? '重试' : '知道了',
          onPrimary: err.recoverable ? () => retryLastAction() : undefined,
        }
    }
  }

  function setError(err: VpnError) {
    if (err.error_type === 'elevation_cancelled') {
      clearError()
      ui.addToast(err.message || '提权失败：用户已取消授权。', 'warning')
      return
    }
    if (err.error_type === 'elevation_required' || err.error_type === 'service_missing') {
      clearError()
      return
    }
    lastError.value = err.message
    lastErrorType.value = err.error_type
    lastRecoverable.value = err.recoverable
    lastRecommendedAction.value = err.recommended_action
    lastErrorTime.value = Date.now()
    const presentation = errorPresentation(err)
    ui.requestError({
      title: presentation.title,
      message: summarizeError(err.message),
      primaryLabel: presentation.primaryLabel,
      secondaryLabel: err.recoverable ? '取消' : '知道了',
      onPrimary: presentation.onPrimary,
      onClose: () => clearError(),
    })
  }

  function clearError() {
    lastError.value = null
    lastErrorType.value = null
    lastRecoverable.value = true
    lastRecommendedAction.value = ''
    lastErrorTime.value = null
    lastFailedConnectMode.value = null
  }

  function retryLastAction() {
    clearError()
    lastMutatingAction.value?.()
  }

  function startConnectionProgress(stageOffset = 0, maxStageIndex = 2) {
    connectionProgressStageOffset.value = stageOffset
    connectionProgressMaxIndex.value = Math.max(stageOffset, Math.min(connectionProgressStages.length - 1, maxStageIndex))
    connectionProgressStartedAt.value = Date.now()
    progressTick.value++
    if (progressTimer) clearInterval(progressTimer)
    progressTimer = setInterval(() => {
      progressTick.value++
    }, 500)
  }

  function stopConnectionProgress() {
    connectionProgressStartedAt.value = null
    connectionProgressStageOffset.value = 0
    connectionProgressMaxIndex.value = 0
    if (progressTimer) {
      clearInterval(progressTimer)
      progressTimer = null
    }
  }

  async function fetchStatus() {
    try {
      const { data } = await api.get<VpnStatus>('/status')
      const previous = status.value
      const inferredMode = data.mode
        ?? (data.connected && previous?.connected ? previous.mode : undefined)
        ?? (data.connected && (activeTemporaryBackend.value || !serviceRunning.value) ? 'elevated' : undefined)
      applyStatus({
        ...data,
        mode: inferredMode,
        backend: data.backend ?? (data.connected ? activeTemporaryBackend.value ?? previous?.backend : undefined),
      })
      if (data.connected) clearError()
    } catch (e) {
      console.error('[vpn] fetchStatus failed:', e)
    }
  }

  async function fetchAppShellState() {
    await Promise.allSettled([fetchStatus(), fetchServiceStatus()])
  }

  function buildPasswordPromptMessage(prefix = '') {
    const auth = config.authConfig
    const username = auth.username || status.value?.username
    const base = username ? `请输入用户 ${username} 的密码` : '请输入 VPN 密码'
    return prefix ? `${prefix}${base}` : base
  }

  async function resolveConnectPassword(messagePrefix = ''): Promise<string | undefined | null> {
    await config.fetchAuthConfig()
    const auth = config.authConfig
    if (auth.remember_password && auth.password_stored) return undefined

    return ui.requestPassword(buildPasswordPromptMessage(messagePrefix))
  }

  async function retryConnectAfterAuthFailure(mode: 'helper' | 'elevated'): Promise<boolean> {
    await config.fetchAuthConfig()
    const password = await ui.requestPassword(
      buildPasswordPromptMessage('密码不正确，请重新输入。'),
    )
    if (password === null) return false
    clearError()
    return mode === 'helper' ? connect(password) : connectElevated(password)
  }

  async function connect(providedPassword?: string): Promise<boolean> {
    const password = providedPassword !== undefined
      ? providedPassword
      : await resolveConnectPassword()
    if (password === null) return false

    loading.value = true
    clearError()
    lastActionWasElevatedConnect.value = false
    lastMutatingAction.value = connect
    connectInFlight.value = true
    startConnectionProgress(2)
    try {
      const { data } = await api.post<VpnStatus>(
        '/connect',
        password === undefined ? undefined : { password },
      )
      applyStatus(data)
      await fetchAppShellState()
      return true
    } catch (error) {
      const normalized = normalizeError(error)
      if (normalized.error_type === 'auth_failed') lastFailedConnectMode.value = 'helper'
      setError(normalized)
    } finally {
      connectInFlight.value = false
      stopConnectionProgress()
      loading.value = false
    }

    return false
  }

  async function disconnect() {
    loading.value = true
    disconnectInFlight.value = true
    lastActionWasElevatedConnect.value = false
    lastMutatingAction.value = disconnect
    try {
      const { data } = await api.post<VpnStatus>('/disconnect')
      applyStatus(data)
      clearError()
      await fetchAppShellState()
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      disconnectInFlight.value = false
      loading.value = false
    }
  }

  async function connectElevated(providedPassword?: string): Promise<boolean> {
    const password = providedPassword !== undefined
      ? providedPassword
      : await resolveConnectPassword()
    if (password === null) return false

    loading.value = true
    clearError()
    lastActionWasElevatedConnect.value = true
    lastMutatingAction.value = connectElevated
    connectInFlight.value = true
    startConnectionProgress(0, 2)
    try {
      const { data } = await api.post<VpnStatus | VpnError>(
        '/connect/elevated',
        password === undefined ? undefined : { password },
      )
      if (isVpnError(data)) {
        if (data.error_type === 'auth_failed') lastFailedConnectMode.value = 'elevated'
        setError(data)
      } else {
        const elevatedStatus = { ...data, mode: data.mode ?? 'elevated' }
        applyStatus(elevatedStatus)
        activeTemporaryBackend.value = elevatedStatus.backend ?? null
        await fetchAppShellState()
        if (status.value?.connected && !status.value.mode) {
          applyStatus({ ...status.value, mode: 'elevated', backend: activeTemporaryBackend.value })
        }
        return true
      }
    } catch (error) {
      const normalized = normalizeError(error)
      if (normalized.error_type === 'auth_failed') lastFailedConnectMode.value = 'elevated'
      setError(normalized)
    } finally {
      lastActionWasElevatedConnect.value = false
      connectInFlight.value = false
      stopConnectionProgress()
      loading.value = false
    }

    return false
  }

  async function connectFromDashboard(installServiceFirst: boolean) {
    if (status.value?.connected) {
      if (currentSessionMode.value === 'helper') {
        await disconnect()
      } else {
        await disconnectElevated()
      }
      return
    }

    if (serviceAvailable.value) {
      await connect()
      return
    }

    if (installServiceFirst) {
      const installed = await installService()
      if (!installed) return
      await connect()
      return
    }

    await connectElevated()
  }

  async function disconnectElevated() {
    loading.value = true
    disconnectInFlight.value = true
    lastActionWasElevatedConnect.value = false
    lastMutatingAction.value = disconnectElevated
    try {
      const { data } = await api.post<VpnStatus | VpnError>('/disconnect/elevated', {
        backend: activeTemporaryBackend.value,
      })
      if (isVpnError(data)) {
        setError(data)
      } else {
        applyStatus(data)
        activeTemporaryBackend.value = null
        clearError()
        await fetchAppShellState()
      }
    } catch (error) {
      setError(normalizeError(error))
    } finally {
      disconnectInFlight.value = false
      loading.value = false
    }
  }

  function rejectActiveVpnForServiceUninstall() {
    if (!status.value?.connected) return false
    setError({
      ok: false,
      error_type: 'native_failure',
      message: 'VPN 连接已建立，请先断开连接再卸载 helper 服务。',
      recoverable: true,
      recommended_action: 'disconnect_first',
      timestamp: Date.now(),
    })
    return true
  }

  async function fetchRoutes() {
    try {
      const { data } = await api.get<RouteEntry[]>('/routes')
      routes.value = data
    } catch (e) {
      console.error('[vpn] fetchRoutes failed:', e)
    }
  }

  async function addRoute(cidr: string) {
    const { data } = await api.post<RouteEntry[]>('/routes', { cidr })
    routes.value = data
  }

  async function removeRoute(cidr: string) {
    const { data } = await api.delete<RouteEntry[]>('/routes', { data: { cidr } })
    routes.value = data
  }

  async function resetRoutes() {
    const { data } = await api.post<RouteEntry[]>('/routes/reset')
    routes.value = data
  }

  async function fetchServiceStatus() {
    try {
      const { data } = await api.get<ServiceStatus>('/service')
      serviceStatus.value = data
    } catch (e) {
      console.error('[vpn] fetchServiceStatus failed:', e)
    }
  }

  async function fetchCliStatus() {
    try {
      const { data } = await api.get<CliInstallStatus>('/cli')
      cliStatus.value = data
    } catch (e) {
      console.error('[vpn] fetchCliStatus failed:', e)
    }
  }

  async function installService() {
    await showServiceOverlay('install')
    try {
      serviceBusy.value = true
      serviceOperation.value = 'install'
      serviceProgress.value = []
      clearError()
      lastMutatingAction.value = installService
      try {
        const { data } = await api.post<ServiceStatus>('/service/install')
        serviceStatus.value = data
        if (data.warning || !data.available) {
          throw new Error(data.warning || 'Helper service is not available after install.')
        }
        await fetchAppShellState()
        return true
      } catch (error) {
        setError(normalizeError(error))
        return false
      } finally {
        serviceOperation.value = null
        serviceBusy.value = false
      }
    } finally {
      serviceOverlayOperation.value = null
    }
  }

  async function uninstallService() {
    await showServiceOverlay('uninstall')
    try {
      if (rejectActiveVpnForServiceUninstall()) return false
      serviceBusy.value = true
      serviceOperation.value = 'uninstall'
      serviceProgress.value = []
      clearError()
      lastMutatingAction.value = uninstallService
      try {
        const { data } = await api.post<ServiceStatus>('/service/uninstall')
        serviceStatus.value = data
        if (data.warning || data.installed) {
          throw new Error(data.warning || 'Helper service is still installed after uninstall.')
        }
        await fetchAppShellState()
        return true
      } catch (error) {
        setError(normalizeError(error))
        return false
      } finally {
        serviceOperation.value = null
        serviceBusy.value = false
      }
    } finally {
      serviceOverlayOperation.value = null
    }
  }

  async function installCli() {
    cliOperation.value = 'install'
    clearError()
    try {
      const { data } = await api.post<CliInstallStatus>('/cli/install')
      cliStatus.value = data
      return true
    } catch (error) {
      setError(normalizeError(error))
      return false
    } finally {
      cliOperation.value = null
    }
  }

  async function uninstallCli() {
    cliOperation.value = 'uninstall'
    clearError()
    try {
      const { data } = await api.post<CliInstallStatus>('/cli/uninstall')
      cliStatus.value = data
      return true
    } catch (error) {
      setError(normalizeError(error))
      return false
    } finally {
      cliOperation.value = null
    }
  }

  function addServiceProgress(entry: ServiceProgressEntry) {
    serviceProgress.value.push(entry)
    if (serviceProgress.value.length > 200) {
      serviceProgress.value = serviceProgress.value.slice(-200)
    }
  }

  function addLog(entry: LogEntry) {
    logs.value.push(entry)
    if (logs.value.length > 1000) logs.value = logs.value.slice(-1000)
  }

  function clearLogs() {
    logs.value = []
  }

  function setLogs(entries: LogEntry[]) {
    logs.value = entries
  }

  return {
    status, loading, routes, logs, serviceStatus, cliStatus, serviceProgress, serviceBusy,
    serviceOverlayOperation,
    serviceOperation, cliOperation,
    lastError, lastErrorType, lastRecoverable, lastRecommendedAction, lastErrorTime,
    serviceInstalled, serviceRunning, serviceAvailable, canUseElevatedFallback,
    recommendedConnectMode, currentSessionMode, displayUptimeSeconds,
    connectInFlight, disconnectInFlight,
    connectionProgress,
    isDesktop, dashboardState, dashboardPrimaryAction, dashboardSecondaryAction,
    fetchStatus, fetchAppShellState, connect, disconnect, connectElevated, disconnectElevated, connectFromDashboard,
    fetchRoutes, addRoute, removeRoute, resetRoutes,
    fetchServiceStatus, fetchCliStatus, installService, uninstallService, installCli, uninstallCli,
    addLog, clearLogs, setLogs, addServiceProgress, clearError, retryLastAction,
  }
})
