<script setup lang="ts">
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch, type Component } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  AlertTriangle,
  CheckCircle,
  Download,
  EthernetPort,
  Fingerprint,
  HardDriveDownload,
  Key,
  Plus,
  RefreshCcw,
  Route as RouteIcon,
  Save,
  Server,
  Settings,
  Shield,
  Trash2,
  Upload,
  User,
} from 'lucide-vue-next'
import { useConfigStore, type AuthConfig, type SettingsConfig, type CoreInspection } from '../stores/config'
import { normalizeError, useVpnStore } from '../stores/vpn'
import { useUiStore } from '../stores/ui'
import ToggleSwitch from '../components/ToggleSwitch.vue'

defineOptions({ name: 'SettingsPage' })

type SectionKey = 'auth' | 'connection' | 'system' | 'routes'

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const vpn = useVpnStore()
const ui = useUiStore()
const isDesktop = typeof window !== 'undefined' && !!window.ecnuVpn

const sections: Array<{ key: SectionKey; label: string; icon: Component }> = [
  { key: 'auth', label: '认证', icon: Shield },
  { key: 'connection', label: '连接', icon: EthernetPort },
  { key: 'system', label: '系统', icon: Settings },
  { key: 'routes', label: '路由', icon: RouteIcon },
]

const scrollRoot = ref<HTMLElement | null>(null)
const authSection = ref<HTMLElement | null>(null)
const connectionSection = ref<HTMLElement | null>(null)
const systemSection = ref<HTMLElement | null>(null)
const routesSection = ref<HTMLElement | null>(null)
const activeSection = ref<SectionKey>('auth')
const savedScrollTop = ref(0)

const authSaving = ref(false)
const systemSaving = ref(false)
const routesBusy = ref(false)
const busyDriver = ref<'wintun' | 'tap' | null>(null)
const authMessage = ref<{ text: string } | null>(null)
const systemMessage = ref<{ text: string } | null>(null)
const routeMessage = ref<{ text: string } | null>(null)
const installFeedback = ref<{ driver: 'wintun' | 'tap'; takes_effect: string } | null>(null)

const serverOptions = [
  'vpn-ct.ecnu.edu.cn',
  'vpn-cn.ecnu.edu.cn',
  'vpn-lt.ecnu.edu.cn',
]
const serverChoice = ref(serverOptions[0])
const customServer = ref('')

const authForm = ref<AuthConfig>({
  server: '',
  username: '',
  password: '',
  password_stored: false,
  user_agent: '',
  remember_password: true,
})

const settingsForm = ref<SettingsConfig>({
  mtu: 1400,
  dtls: true,
  extra_args: '',
  log_path: '',
  webui_port: 18080,
  webui_host: '127.0.0.1',
  webui_enabled: true,
  vpn_engine: 'native',
  windows_tunnel_driver: 'auto',
  windows_tap_interface: '',
  auto_reconnect: true,
  retry_limit: -1,
  minimal_mode: false,
  service_install_prompt_seen: false,
  minimal_install_service_before_connect: true,
})
const nativeEngineSelected = computed(() => settingsForm.value.vpn_engine === 'native')

const routes = ref<string[]>([])
const newRoute = ref('')

const importFileInput = ref<HTMLInputElement | null>(null)
const coreInspection = ref<CoreInspection | null>(null)
const coreMaintenanceBusy = ref(false)

// Warning copy for password-protected export. Verbatim from the plan
// (docs/superpowers/plans/2026-06-16-cli-core-ui-contract-refactor-plan.md
// Task 9 Step 3) — keep this string in sync with the plan text exactly.
const PROTECTED_EXPORT_WARNING =
  'The exported configuration contains a recoverable VPN password. Treat it as a sensitive file. Do not share it through untrusted channels or copy it to public machines. The password is not stored in plaintext, but a weak export password can still be attacked offline. Destroy the file after use when possible.'

const showCoreMaintenanceBanner = computed(() => {
  const inspection = coreInspection.value
  if (!inspection) return false
  return inspection.state === 'broken' || inspection.risk === 'high'
})

const passwordPlaceholder = computed(() =>
  authForm.value.password_stored
    ? '留空表示保留原密码，输入新密码覆盖'
    : '请输入密码',
)

const driverSupported = computed(() => !!config.driverStatus?.supported)
const tapAdapters = computed(() => config.driverStatus?.tap_adapters || [])
const activeRuntimeStatus = computed(() => config.runtimeStatus)
const runtimeMissing = computed(() => !!activeRuntimeStatus.value && !activeRuntimeStatus.value.available)
const wintunReady = computed(() => {
  const status = config.driverStatus
  if (!status) return false
  return status.wintun_missing === false ||
    status.wintun_bundled === true ||
    (status.wintun_adapters?.length ?? 0) > 0
})
const tapReady = computed(() => {
  const status = config.driverStatus
  if (!status) return false
  return status.tap_missing === false ||
    status.tap_available === true ||
    (status.tap_adapters?.length ?? 0) > 0
})
const wintunStatusText = computed(() => {
  const status = config.driverStatus
  if (!status) return '未检测到 Wintun'
  if (status.wintun_bundled) return '已检测到内置 wintun.dll'
  if ((status.wintun_adapters?.length ?? 0) > 0) return '已检测到 Wintun 适配器'
  return '未检测到内置 wintun.dll'
})
const wintunMissing = computed(() => config.driverStatus?.wintun_missing ?? !wintunReady.value)
const tapMissing = computed(() => config.driverStatus?.tap_missing ?? !tapReady.value)
const driverReadiness = computed(() => {
  const status = config.driverStatus
  if (!status) return 'unavailable'
  if (status.effective_driver_status) return status.effective_driver_status
  return wintunReady.value || tapReady.value ? 'ready' : 'unavailable'
})
const runtimeReady = computed(() => activeRuntimeStatus.value?.available ?? true)
const runtimeSourceText = computed(() => {
  const status = activeRuntimeStatus.value
  if (!status) return '未知'
  return status.source
})
const runtimePathText = computed(() => {
  const status = activeRuntimeStatus.value
  if (status?.path) return status.path
  return '原生引擎由应用内置提供'
})
const runtimeDirectoryText = computed(() => {
  const status = activeRuntimeStatus.value
  if (status?.bundled_runtime_dir) return status.bundled_runtime_dir
  return '原生引擎随应用提供'
})

const anyDriverAvailable = computed(() => {
  if (!config.driverStatus) return false
  return wintunReady.value || tapReady.value
})

const driverReadinessLabel = computed(() => {
  switch (driverReadiness.value) {
    case 'ready': return '驱动就绪'
    case 'degraded': return '驱动降级'
    case 'unavailable': return '驱动不可用'
    default: return '未知'
  }
})

const driverReadinessColor = computed(() => {
  switch (driverReadiness.value) {
    case 'ready': return 'text-green-400'
    case 'degraded': return 'text-yellow-400'
    case 'unavailable': return 'text-red-400'
    default: return 'text-muted'
  }
})

const serviceButtonLabel = computed(() => {
  if (vpn.serviceOperation === 'install') return '安装中...'
  if (vpn.serviceOperation === 'uninstall') return '卸载中...'
  return vpn.serviceInstalled ? '卸载服务' : '安装服务'
})

const serviceButtonInstalled = computed(() => {
  if (vpn.serviceOperation === 'uninstall') return true
  if (vpn.serviceOperation === 'install') return false
  return vpn.serviceInstalled
})

const cliButtonLabel = computed(() => {
  if (vpn.cliOperation === 'install') return '安装中...'
  if (vpn.cliOperation === 'uninstall') return '卸载中...'
  return vpn.cliStatus?.installed ? '卸载 CLI' : '安装 CLI'
})

const cliButtonInstalled = computed(() => {
  if (vpn.cliOperation === 'uninstall') return true
  if (vpn.cliOperation === 'install') return false
  return vpn.cliStatus?.installed ?? false
})

function sectionElement(key: SectionKey) {
  if (key === 'auth') return authSection.value
  if (key === 'connection') return connectionSection.value
  if (key === 'system') return systemSection.value
  return routesSection.value
}

function sectionHash(key: SectionKey) {
  return `#settings-${key}`
}

function sectionFromHash(hash: unknown): SectionKey | null {
  if (hash === '#settings-auth' || hash === '#auth') return 'auth'
  if (hash === '#settings-connection' || hash === '#connection') return 'connection'
  if (hash === '#settings-system' || hash === '#system') return 'system'
  if (hash === '#settings-routes' || hash === '#routes') return 'routes'
  return null
}

function sectionFromQuery(value: unknown): SectionKey | null {
  const section = Array.isArray(value) ? value[0] : value
  return section === 'auth' || section === 'connection' || section === 'system' || section === 'routes'
    ? section
    : null
}

function updateActiveSection() {
  const root = scrollRoot.value
  if (!root) return

  const rootRect = root.getBoundingClientRect()
  const threshold = rootRect.top + Math.min(180, root.clientHeight * 0.35)
  let next = activeSection.value

  for (const section of sections) {
    const el = sectionElement(section.key)
    if (!el) continue
    if (el.getBoundingClientRect().top <= threshold) {
      next = section.key
    }
  }
  if (root.scrollTop + root.clientHeight >= root.scrollHeight - 12) {
    next = sections[sections.length - 1].key
  }
  activeSection.value = next
}

let scrollListenerAttached = false

function explicitSection(): SectionKey | null {
  return sectionFromHash(route.hash) || sectionFromQuery(route.query.section)
}

function attachScrollListener() {
  if (scrollListenerAttached) return
  if (!scrollRoot.value) return
  scrollRoot.value.addEventListener('scroll', updateActiveSection, { passive: true })
  scrollListenerAttached = true
}

function detachScrollListener() {
  if (!scrollListenerAttached) return
  scrollRoot.value?.removeEventListener('scroll', updateActiveSection)
  scrollListenerAttached = false
}

async function scrollToSection(
  key: SectionKey,
  updateRoute = true,
  behavior: ScrollBehavior = 'smooth',
) {
  await nextTick()
  sectionElement(key)?.scrollIntoView({ behavior, block: 'start' })
  activeSection.value = key
  if (updateRoute) {
    router.replace({ path: '/settings', hash: sectionHash(key) })
  }
}

function applyServerChoice(server: string) {
  if (serverOptions.includes(server)) {
    serverChoice.value = server
    customServer.value = ''
  } else if (server) {
    serverChoice.value = 'custom'
    customServer.value = server
  } else {
    serverChoice.value = serverOptions[0]
    customServer.value = ''
  }
}

function extractErrorText(err: any): string {
  return (
    err?.response?.data?.error ||
    err?.message ||
    err?.toString?.() ||
    '保存失败'
  )
}

async function refreshRuntime() {
  if (!isDesktop) return
  await Promise.all([
    config.fetchRuntimeStatus(),
    config.fetchDriverStatus(),
  ])
}

async function loadAuth() {
  await config.fetchAuthConfig()
  authForm.value = {
    ...config.authConfig,
    password: '',
  }
  applyServerChoice(authForm.value.server)
}

async function loadRoutes() {
  await vpn.fetchRoutes()
  routes.value = vpn.routes.map((r) => r.cidr)
}

onMounted(async () => {
  await Promise.all([
    loadAuth(),
    config.fetchSettings().then(() => {
      settingsForm.value = { ...config.settings }
    }),
    loadRoutes(),
    vpn.fetchServiceStatus(),
    vpn.fetchCliStatus(),
  ])

  if (isDesktop) {
    try {
      await refreshRuntime()
    } catch (error) {
      ui.requestError({ title: '刷新运行时失败', message: normalizeError(error).message })
    }
    void inspectCoreSilently()
  }

  attachScrollListener()
  const section = explicitSection()
  if (section) {
    await scrollToSection(section, false, 'auto')
  } else {
    activeSection.value = 'auth'
    scrollRoot.value?.scrollTo({ top: 0, behavior: 'auto' })
  }
  updateActiveSection()
})

onActivated(() => {
  attachScrollListener()
  void nextTick(() => {
    if (explicitSection()) return
    if (scrollRoot.value) {
      scrollRoot.value.scrollTop = savedScrollTop.value
      updateActiveSection()
    }
  })
})

onDeactivated(() => {
  savedScrollTop.value = scrollRoot.value?.scrollTop ?? savedScrollTop.value
  detachScrollListener()
})

onBeforeUnmount(() => {
  detachScrollListener()
})

watch(
  () => [route.hash, route.query.section],
  () => {
    if (route.path !== '/settings') return
    const section = explicitSection()
    if (section) {
      scrollToSection(section, false, 'auto')
    }
  },
)

watch(
  () => authForm.value.remember_password,
  (remember) => {
    if (!remember) {
      authForm.value.password = ''
    }
  },
)

async function saveAuth() {
  authSaving.value = true
  authMessage.value = null
  try {
    authForm.value.server = serverChoice.value === 'custom'
      ? customServer.value.trim()
      : serverChoice.value
    if (!authForm.value.remember_password) {
      authForm.value.password = ''
    }
    await config.saveAuthConfig(authForm.value)
    authForm.value.password = ''
    authForm.value.password_stored = config.authConfig.password_stored ?? authForm.value.password_stored
    authMessage.value = { text: '认证设置已保存' }
  } catch (e: any) {
    ui.requestError({ title: '保存认证设置失败', message: extractErrorText(e) })
  } finally {
    authSaving.value = false
  }
}

async function saveSystem() {
  systemSaving.value = true
  systemMessage.value = null
  try {
    await config.saveSettings(settingsForm.value)
    if (isDesktop) {
      await refreshRuntime()
    }
    systemMessage.value = { text: '设置已保存' }
  } catch (error) {
    ui.requestError({ title: '保存设置失败', message: normalizeError(error).message })
  } finally {
    systemSaving.value = false
  }
}

async function toggleService() {
  systemMessage.value = null
  const installed = vpn.serviceInstalled
  if (vpn.status?.connected && installed) {
    ui.requestError({
      title: '请先断开 VPN',
      message: '卸载 helper 服务前必须先断开当前 VPN 连接。',
      primaryLabel: '知道了',
      secondaryLabel: '取消',
    })
    return
  }
  const ok = installed ? await vpn.uninstallService() : await vpn.installService()
  if (ok) {
    ui.addToast(installed ? '辅助服务已卸载' : '辅助服务已安装', 'success')
  }
}

async function toggleCli() {
  systemMessage.value = null
  const installed = vpn.cliStatus?.installed ?? false
  const ok = installed ? await vpn.uninstallCli() : await vpn.installCli()
  if (ok) {
    ui.addToast(installed ? 'CLI 已卸载' : 'CLI 已安装；请打开新终端使用 exv', 'success')
  }
}

async function installDriver(driver: 'wintun' | 'tap') {
  busyDriver.value = driver
  systemMessage.value = null
  installFeedback.value = null
  try {
    const result = await config.installDriver(driver)
    await config.fetchDriverStatus()
    if (driver === 'tap' && !settingsForm.value.windows_tap_interface && tapAdapters.value.length > 0) {
      settingsForm.value.windows_tap_interface = tapAdapters.value[0]
    }
    const takesEffect = result.takes_effect || (driver === 'wintun' ? 'next_connect' : 'immediately')
    installFeedback.value = { driver, takes_effect: takesEffect }
    systemMessage.value = {
      text: takesEffect === 'next_connect'
        ? 'Wintun 运行时已就绪，下次连接时生效'
        : 'TAP 安装完成，立即生效。如已连接 VPN，建议断开重连。',
    }
  } catch (error) {
    ui.requestError({ title: '安装驱动失败', message: normalizeError(error).message })
  } finally {
    busyDriver.value = null
  }
}

async function addRoute() {
  const cidr = newRoute.value.trim()
  if (!cidr || routes.value.includes(cidr) || routesBusy.value) return

  routesBusy.value = true
  routeMessage.value = null
  try {
    await vpn.addRoute(cidr)
    routes.value = vpn.routes.map((entry) => entry.cidr)
    newRoute.value = ''
    routeMessage.value = { text: '路由已添加' }
  } catch (e: any) {
    ui.requestError({ title: '添加路由失败', message: e?.message || '添加失败' })
  } finally {
    routesBusy.value = false
  }
}

async function removeRoute(index: number) {
  if (routesBusy.value) return
  const cidr = routes.value[index]
  if (!cidr) return

  routesBusy.value = true
  routeMessage.value = null
  try {
    await vpn.removeRoute(cidr)
    routes.value = vpn.routes.map((entry) => entry.cidr)
    routeMessage.value = { text: '路由已删除' }
  } catch (e: any) {
    ui.requestError({ title: '删除路由失败', message: e?.message || '删除失败' })
  } finally {
    routesBusy.value = false
  }
}

async function inspectCoreSilently() {
  try {
    coreInspection.value = await config.inspectCore()
  } catch {
    coreInspection.value = null
  }
}

function triggerImportConfig() {
  importFileInput.value?.click()
}

function readFileAsText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(reader.error || new Error('读取文件失败'))
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '')
    reader.readAsText(file)
  })
}

async function handleImportFileChange(event: Event) {
  const target = event.target as HTMLInputElement
  const file = target.files?.[0]
  target.value = ''
  if (!file) return
  try {
    const text = await readFileAsText(file)
    const password = await ui.requestPassword('请输入导入文件的密码（如有）')
    if (password === null) return
    const isProtected = password.length > 0
    await config.importConfig({
      format: isProtected ? 'protected' : 'unprotected',
      data: text,
      password: isProtected ? password : undefined,
    })
    ui.addToast('配置已导入', 'success')
    authForm.value = { ...config.authConfig, password: '' }
    applyServerChoice(authForm.value.server)
    settingsForm.value = { ...config.settings }
  } catch (error) {
    ui.requestError({ title: '导入配置失败', message: normalizeError(error).message })
  }
}

function downloadExport(filename: string, payload: string) {
  const blob = new Blob([payload], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
  setTimeout(() => URL.revokeObjectURL(url), 0)
}

async function exportConfigUnprotected() {
  try {
    const result = await config.exportConfig({ protected: false })
    downloadExport('ecnu-vpn-config.json', result.data)
    ui.addToast('配置已导出', 'success')
  } catch (error) {
    ui.requestError({ title: '导出配置失败', message: normalizeError(error).message })
  }
}

async function exportConfigProtected() {
  try {
    const password = await ui.requestPassword('设置导出密码')
    if (password === null) return
    if (!password) {
      ui.requestError({ title: '密码不能为空', message: '请提供一个非空的导出密码。' })
      return
    }
    const result = await config.exportConfig({ protected: true, password })
    downloadExport('ecnu-vpn-config.protected', result.data)
    ui.requestError({
      title: '导出已完成',
      message: PROTECTED_EXPORT_WARNING,
      primaryLabel: '知道了',
    })
  } catch (error) {
    ui.requestError({ title: '导出配置失败', message: normalizeError(error).message })
  }
}

function resetConfigAction() {
  ui.requestConfirm('此操作会清空所有 VPN 配置，是否继续？', async () => {
    try {
      await config.resetConfig(true)
      ui.addToast('配置已重置', 'success')
      authForm.value = { ...config.authConfig, password: '' }
      applyServerChoice(authForm.value.server)
      settingsForm.value = { ...config.settings }
    } catch (error) {
      ui.requestError({ title: '重置配置失败', message: normalizeError(error).message })
    }
  })
}

function resetKeyAction() {
  ui.requestConfirm('此操作会清除本地保存的加密密码，下次连接需重新输入。是否继续？', async () => {
    try {
      await config.resetKey(true)
      ui.addToast('已重置加密密钥', 'success')
    } catch (error) {
      ui.requestError({ title: '重置加密密钥失败', message: normalizeError(error).message })
    }
  })
}

function killStaleCoreAction() {
  ui.requestConfirm('确认终止该内核进程？此操作不可撤销。', async () => {
    if (coreMaintenanceBusy.value) return
    coreMaintenanceBusy.value = true
    try {
      await config.killStaleCore(true)
      ui.addToast('已清理', 'success')
      await inspectCoreSilently()
    } catch (error) {
      ui.requestError({ title: '清理残留进程失败', message: normalizeError(error).message })
    } finally {
      coreMaintenanceBusy.value = false
    }
  })
}
</script>

<template>
  <div ref="scrollRoot" class="settings-scroll h-full overflow-y-auto py-4 pr-12">
    <div class="mx-auto max-w-4xl space-y-5">
      <header>
        <h1 class="text-xl font-semibold text-foreground">设置</h1>
      </header>

      <section
        id="settings-auth"
        ref="authSection"
        class="scroll-mt-4 rounded-xl border border-border bg-surface p-5"
      >
        <h2 class="mb-4 flex items-center gap-2 text-base font-semibold text-foreground">
          <Shield class="h-5 w-5 text-accent" />
          认证
        </h2>
        <form class="space-y-4" @submit.prevent="saveAuth">
          <div>
            <label class="mb-1.5 block text-sm text-muted">VPN 服务器</label>
            <div class="space-y-2">
              <div class="relative">
                <Server class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
                <select
                  v-model="serverChoice"
                  class="w-full appearance-none rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground focus:border-accent focus:outline-none"
                >
                  <option v-for="server in serverOptions" :key="server" :value="server">
                    {{ server }}
                  </option>
                  <option value="custom">自定义</option>
                </select>
              </div>
              <div v-if="serverChoice === 'custom'" class="relative">
                <Server class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
                <input
                  v-model="customServer"
                  type="text"
                  placeholder="请输入 VPN 服务器地址"
                  class="w-full rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground placeholder:text-muted/50 focus:border-accent focus:outline-none"
                />
              </div>
            </div>
          </div>

          <div>
            <label class="mb-1.5 block text-sm text-muted">用户名</label>
            <div class="relative">
              <User class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <input
                v-model="authForm.username"
                type="text"
                placeholder="ECNU 用户名"
                class="w-full rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground placeholder:text-muted/50 focus:border-accent focus:outline-none"
              />
            </div>
          </div>

          <div class="flex items-center gap-3">
            <input
              id="remember-password"
              v-model="authForm.remember_password"
              type="checkbox"
              class="h-4 w-4 rounded border-border accent-accent"
            />
            <label for="remember-password" class="text-sm text-foreground">
              记住密码（加密存储）
            </label>
          </div>

          <div v-if="authForm.remember_password">
            <label class="mb-1.5 block text-sm text-muted">密码</label>
            <div class="relative">
              <Key class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <input
                v-model="authForm.password"
                type="password"
                autocomplete="new-password"
                :placeholder="passwordPlaceholder"
                class="w-full rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground placeholder:text-muted/50 focus:border-accent focus:outline-none"
              />
            </div>
            <p v-if="authForm.password_stored" class="mt-1 text-xs text-muted">
              已保存加密密码，仅在需要修改时输入。
            </p>
          </div>

          <div>
            <label class="mb-1.5 block text-sm text-muted">客户端标识</label>
            <div class="relative">
              <Fingerprint class="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <input
                v-model="authForm.user_agent"
                type="text"
                placeholder="AnyConnect"
                class="w-full rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground placeholder:text-muted/50 focus:border-accent focus:outline-none"
              />
            </div>
            <p class="mt-1 text-xs text-muted">发送到 VPN 服务器的客户端标识字符串</p>
          </div>

          <div class="flex items-center gap-3 pt-1">
            <button
              type="submit"
              :disabled="authSaving"
              class="rounded-lg bg-accent px-6 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent/90 disabled:opacity-50"
            >
              {{ authSaving ? '保存中...' : '保存认证设置' }}
            </button>
            <div
              v-if="authMessage"
              :class="[
                'rounded-lg px-4 py-2.5 text-sm',
                'bg-green-500/10 text-green-400',
              ]"
            >
              {{ authMessage.text }}
            </div>
          </div>
        </form>
      </section>

      <section
        id="settings-connection"
        ref="connectionSection"
        class="scroll-mt-4 rounded-xl border border-border bg-surface p-5"
      >
        <h2 class="mb-4 flex items-center gap-2 text-base font-semibold text-foreground">
          <EthernetPort class="h-5 w-5 text-accent" />
          连接
        </h2>

        <div
          v-if="showCoreMaintenanceBanner"
          class="mb-4 rounded-lg border border-yellow-500/30 bg-yellow-500/10 px-4 py-3 text-sm text-yellow-300"
        >
          <p class="font-medium">检测到残留 / 异常的 VPN 内核进程</p>
          <p class="mt-1 text-xs text-yellow-300/80">
            已发现可能无响应或卡死的内核（pid {{ coreInspection?.pid ?? '-' }}）。是否清理？
          </p>
          <button
            :disabled="coreMaintenanceBusy"
            class="mt-3 rounded-lg border border-yellow-400/50 px-4 py-2 text-xs font-medium text-yellow-200 transition-colors hover:bg-yellow-500/20 disabled:opacity-50"
            @click="killStaleCoreAction"
          >
            清理残留进程
          </button>
        </div>

        <div class="space-y-5">
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted">MTU</label>
            <input
              v-model.number="settingsForm.mtu"
              type="number"
              min="576"
              max="1500"
              class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
            />
          </div>

          <div class="flex items-center justify-between rounded-lg border border-border bg-bg/40 px-4 py-3">
            <div>
              <p class="text-sm text-foreground">DTLS</p>
              <p class="text-xs text-muted">当前原生连接使用 CSTP-only；DTLS 后端加入前不会启用。</p>
            </div>
            <ToggleSwitch
              v-model="settingsForm.dtls"
            />
          </div>

          <div class="flex items-center justify-between rounded-lg border border-border bg-bg/40 px-4 py-3">
            <div>
              <p class="text-sm text-foreground">断线重连</p>
              <p class="text-xs text-muted">连接进程意外退出后自动尝试重新连接</p>
            </div>
            <ToggleSwitch
              v-model="settingsForm.auto_reconnect"
            />
          </div>

          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted">重连尝试次数</label>
            <input
              v-model.number="settingsForm.retry_limit"
              type="number"
              min="-1"
              class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
            />
            <p class="mt-1 text-xs text-muted">-1 表示无限重连；0 表示不重连；正整数表示最大次数。</p>
          </div>

          <div class="flex items-center gap-3 pt-1">
            <button
              :disabled="systemSaving"
              class="flex items-center gap-2 rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent/90 disabled:opacity-50"
              @click="saveSystem"
            >
              <Save class="h-4 w-4" />
              {{ systemSaving ? '保存中...' : '保存连接设置' }}
            </button>
            <div
              v-if="systemMessage"
              :class="[
                'rounded-lg px-4 py-2.5 text-sm',
                'bg-green-500/10 text-green-400',
              ]"
            >
              {{ systemMessage.text }}
            </div>
          </div>
        </div>
      </section>

      <section
        id="settings-system"
        ref="systemSection"
        class="scroll-mt-4 rounded-xl border border-border bg-surface p-5"
      >
        <div class="mb-4 flex items-start justify-between gap-4">
          <h2 class="flex items-center gap-2 text-base font-semibold text-foreground">
            <Settings class="h-5 w-5 text-accent" />
            系统
          </h2>
          <button
            v-if="isDesktop"
            class="flex items-center gap-2 rounded-lg border border-border px-3 py-2 text-xs text-foreground transition-colors hover:border-accent/50"
            @click="refreshRuntime"
          >
            <RefreshCcw class="h-3.5 w-3.5" />
            刷新
          </button>
        </div>

        <div class="space-y-5">
          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted">日志文件路径</label>
            <input
              v-model="settingsForm.log_path"
              type="text"
              placeholder="~/.ecnuvpn/ecnuvpn.log"
              class="w-full rounded-lg border border-border bg-bg px-3 py-2 font-mono text-sm text-foreground placeholder:text-muted transition-colors focus:border-accent/50 focus:outline-none"
            />
          </div>

          <div>
            <label class="mb-1.5 block text-xs font-medium text-muted">额外参数</label>
            <input
              v-model="settingsForm.extra_args"
              type="text"
              :placeholder="nativeEngineSelected ? '--no-dtls --authgroup=students' : '额外兼容参数'"
              class="w-full rounded-lg border border-border bg-bg px-3 py-2 font-mono text-sm text-foreground placeholder:text-muted transition-colors focus:border-accent/50 focus:outline-none"
            />
            <p v-if="nativeEngineSelected" class="mt-1 text-xs text-muted">
              原生模式仅支持 --no-dtls、--useragent=...、--authgroup=...、--csd-wrapper=...；其他参数会在连接前明确拒绝。
            </p>
          </div>

          <div class="border-t border-border pt-4">
            <div class="mb-3">
              <p class="text-sm font-medium text-foreground">Helper 服务</p>
              <p class="text-xs text-muted">安装后连接和断开不需要每次提权。</p>
            </div>
            <button
              :disabled="vpn.serviceBusy"
              :class="[
                'inline-flex items-center gap-2 rounded-lg px-5 py-2.5 text-sm font-medium text-white transition-colors disabled:opacity-50',
                serviceButtonInstalled ? 'bg-destructive hover:bg-destructive/90' : 'bg-accent hover:bg-accent/90',
              ]"
              @click="toggleService"
            >
              <Trash2 v-if="serviceButtonInstalled" class="h-4 w-4" />
              <Download v-else class="h-4 w-4" />
              {{ serviceButtonLabel }}
            </button>
          </div>

          <div class="border-t border-border pt-4">
            <div class="mb-3">
              <p class="text-sm font-medium text-foreground">配置导入 / 导出 / 重置</p>
              <p class="text-xs text-muted">导入或导出 VPN 配置；密码保护导出的文件可在不同设备间安全迁移。</p>
            </div>
            <input
              ref="importFileInput"
              type="file"
              class="hidden"
              accept=".json,.protected,.txt,application/json,application/octet-stream,text/plain"
              @change="handleImportFileChange"
            />
            <div class="flex flex-wrap gap-3">
              <button
                class="inline-flex items-center gap-2 rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50"
                @click="triggerImportConfig"
              >
                <Upload class="h-4 w-4" />
                导入配置
              </button>
              <button
                class="inline-flex items-center gap-2 rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50"
                @click="exportConfigUnprotected"
              >
                <Download class="h-4 w-4" />
                导出配置（不含密码）
              </button>
              <button
                class="inline-flex items-center gap-2 rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50"
                @click="exportConfigProtected"
              >
                <Download class="h-4 w-4" />
                导出配置（密码保护）
              </button>
              <button
                class="inline-flex items-center gap-2 rounded-lg bg-destructive px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-destructive/90"
                @click="resetConfigAction"
              >
                <Trash2 class="h-4 w-4" />
                重置配置
              </button>
              <button
                class="inline-flex items-center gap-2 rounded-lg border border-destructive/50 px-4 py-2 text-sm text-destructive transition-colors hover:bg-destructive/10"
                @click="resetKeyAction"
              >
                <Key class="h-4 w-4" />
                重置加密密钥
              </button>
            </div>
          </div>

          <div v-if="isDesktop" class="border-t border-border pt-4">
            <div class="mb-3">
              <p class="text-sm font-medium text-foreground">终端 CLI</p>
              <p class="text-xs text-muted">
                {{ vpn.cliStatus?.installed ? '已安装全局 exv 命令。' : '未安装全局命令；仍可从应用包内 bin 目录直接运行 exv。' }}
              </p>
              <p class="mt-1 break-all font-mono text-[11px] text-muted">
                {{ vpn.cliStatus?.installed ? vpn.cliStatus.installPath : vpn.cliStatus?.targetPath }}
              </p>
              <p v-if="vpn.cliStatus?.warning" class="mt-1 text-xs text-warning">
                {{ vpn.cliStatus.warning }}
              </p>
              <p v-else-if="vpn.cliStatus?.installed && !vpn.cliStatus.availableInPath" class="mt-1 text-xs text-warning">
                已安装；请打开新终端让 PATH 生效。
              </p>
            </div>
            <div class="flex flex-wrap gap-3">
              <button
                :disabled="!!vpn.cliOperation"
                :class="[
                  'inline-flex items-center gap-2 rounded-lg px-5 py-2.5 text-sm font-medium text-white transition-colors disabled:opacity-50',
                  cliButtonInstalled ? 'bg-destructive hover:bg-destructive/90' : 'bg-accent hover:bg-accent/90',
                ]"
                @click="toggleCli"
              >
                <Trash2 v-if="cliButtonInstalled" class="h-4 w-4" />
                <Download v-else class="h-4 w-4" />
                {{ cliButtonLabel }}
              </button>
              <button
                :disabled="!!vpn.cliOperation"
                class="inline-flex items-center gap-2 rounded-lg border border-border px-4 py-2.5 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
                @click="vpn.fetchCliStatus()"
              >
                <RefreshCcw class="h-4 w-4" />
                刷新
              </button>
            </div>
          </div>

          <div v-if="isDesktop" class="border-t border-border pt-4">
            <div class="mb-4 flex items-center gap-2">
              <span :class="runtimeReady ? 'text-green-400' : 'text-red-400'" class="text-lg leading-none">&#9679;</span>
              <span class="text-sm font-medium" :class="runtimeReady ? 'text-foreground' : 'text-red-400'">
                {{ runtimeReady ? '运行时就绪' : '运行时缺失，需要修复' }}
              </span>
            </div>

            <div class="space-y-4">
              <div class="grid gap-4 text-sm md:grid-cols-2">
                <div class="rounded-lg border border-border bg-bg p-4">
                  <p class="mb-2 text-xs text-muted">当前 VPN 引擎</p>
                  <p class="font-medium text-foreground">
                    {{ runtimeSourceText }}
                  </p>
                  <p class="mt-2 break-all text-xs text-muted">
                    {{ runtimePathText }}
                  </p>
                </div>
                <div class="rounded-lg border border-border bg-bg p-4">
                  <p class="mb-2 text-xs text-muted">版本</p>
                  <p class="font-medium text-foreground">
                    {{ config.runtimeStatus?.version || '未知' }}
                  </p>
                  <p class="mt-2 break-all text-xs text-muted">
                    {{ runtimeDirectoryText }}
                  </p>
                </div>
              </div>

              <div v-if="runtimeMissing" class="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300">
                <p class="font-medium">原生运行时不可用</p>
                <p class="mt-1 text-xs text-red-300/80">
                  {{ config.runtimeStatus?.missing_what || 'VPN 连接需要原生运行时组件。' }}
                </p>
                <p v-if="config.runtimeStatus?.recommended_action" class="mt-1 text-xs text-red-300/80">
                  {{ config.runtimeStatus.recommended_action }}
                </p>
                <p v-if="config.runtimeStatus?.effect_on_connect" class="mt-1 text-xs text-red-300/60">
                  {{ config.runtimeStatus.effect_on_connect }}
                </p>
              </div>

              <div v-if="driverSupported" class="space-y-4">
                <div class="flex items-center gap-2 text-sm">
                  <CheckCircle v-if="driverReadiness === 'ready'" class="h-4 w-4 text-green-400" />
                  <AlertTriangle v-else class="h-4 w-4" :class="driverReadinessColor" />
                  <span :class="driverReadinessColor">{{ driverReadinessLabel }}</span>
                  <span v-if="driverReadiness === 'unavailable'" class="text-xs text-red-400/80">- 需要至少一个可用隧道驱动</span>
                  <span v-else-if="driverReadiness === 'degraded'" class="text-xs text-yellow-400/80">- 首选驱动缺失，回退可用</span>
                </div>

                <div v-if="!anyDriverAvailable" class="rounded-lg border border-yellow-500/30 bg-yellow-500/10 px-4 py-3 text-sm text-yellow-300">
                  <p class="font-medium">隧道驱动缺失</p>
                  <p class="mt-1 text-xs text-yellow-300/80">
                    需要至少一个可用的隧道驱动（Wintun 或 TAP）才能建立 VPN 连接。
                  </p>
                </div>

                <div class="grid gap-4 md:grid-cols-2">
                  <div>
                    <label class="mb-1.5 block text-xs font-medium text-muted">Windows 隧道驱动</label>
                    <select
                      v-model="settingsForm.windows_tunnel_driver"
                      class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
                    >
                      <option value="auto">自动（优先 Wintun）</option>
                      <option value="wintun">Wintun</option>
                      <option value="tap">TAP</option>
                    </select>
                  </div>

                  <div>
                    <label class="mb-1.5 block text-xs font-medium text-muted">TAP 接口</label>
                    <select
                      v-model="settingsForm.windows_tap_interface"
                      :disabled="tapAdapters.length === 0"
                      class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none disabled:opacity-50"
                    >
                      <option value="">选择 TAP 适配器</option>
                      <option v-for="adapter in tapAdapters" :key="adapter" :value="adapter">
                        {{ adapter }}
                      </option>
                    </select>
                  </div>
                </div>

                <div class="grid gap-4 text-sm md:grid-cols-2">
                  <div class="rounded-lg border border-border bg-bg p-4">
                    <div class="mb-2 flex items-center gap-2">
                      <p class="text-xs text-muted">Wintun</p>
                      <span v-if="!wintunMissing" class="rounded bg-green-500/10 px-2 py-0.5 text-xs text-green-400">就绪</span>
                      <span v-else class="rounded bg-yellow-500/10 px-2 py-0.5 text-xs text-yellow-400">缺失</span>
                    </div>
                    <p class="font-medium text-foreground">{{ wintunStatusText }}</p>
                    <p v-if="wintunMissing && config.driverStatus?.wintun_recommended_action" class="mt-2 text-xs text-yellow-400/80">
                      {{ config.driverStatus.wintun_recommended_action }}
                    </p>
                  </div>

                  <div class="rounded-lg border border-border bg-bg p-4">
                    <div class="mb-2 flex items-center gap-2">
                      <p class="text-xs text-muted">TAP</p>
                      <span v-if="config.driverStatus?.tap_available" class="rounded bg-green-500/10 px-2 py-0.5 text-xs text-green-400">已安装</span>
                      <span v-else-if="config.driverStatus?.tap_can_install" class="rounded bg-yellow-500/10 px-2 py-0.5 text-xs text-yellow-400">可安装</span>
                      <span v-else class="rounded bg-red-500/10 px-2 py-0.5 text-xs text-red-400">不可用</span>
                    </div>
                    <p class="font-medium text-foreground">
                      {{ config.driverStatus?.tap_available ? '已检测到安装的适配器' : '未检测到 TAP 适配器' }}
                    </p>
                    <p v-if="tapMissing && config.driverStatus?.tap_recommended_action" class="mt-2 text-xs text-yellow-400/80">
                      {{ config.driverStatus.tap_recommended_action }}
                    </p>
                  </div>
                </div>

                <div class="flex flex-wrap gap-3">
                  <button
                    :disabled="busyDriver === 'wintun'"
                    class="flex items-center gap-2 rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
                    @click="installDriver('wintun')"
                  >
                    <HardDriveDownload class="h-4 w-4" />
                    {{ busyDriver === 'wintun' ? '准备中...' : '准备 Wintun' }}
                  </button>
                  <button
                    :disabled="busyDriver === 'tap'"
                    class="flex items-center gap-2 rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
                    @click="installDriver('tap')"
                  >
                    <HardDriveDownload class="h-4 w-4" />
                    {{ busyDriver === 'tap' ? '安装中...' : '安装 TAP' }}
                  </button>
                </div>

                <div v-if="installFeedback" class="rounded-lg border border-border bg-bg p-3 text-xs text-muted">
                  <template v-if="installFeedback.takes_effect === 'next_connect'">
                    Wintun 运行时已就绪，下次连接时生效。
                  </template>
                  <template v-else>
                    TAP 安装完成，立即生效。如已连接 VPN，建议断开重连。
                  </template>
                </div>
              </div>
            </div>
          </div>

          <div class="flex items-center gap-3 pt-1">
            <button
              :disabled="systemSaving"
              class="flex items-center gap-2 rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-accent/90 disabled:opacity-50"
              @click="saveSystem"
            >
              <Save class="h-4 w-4" />
              {{ systemSaving ? '保存中...' : '保存系统设置' }}
            </button>
            <div
              v-if="systemMessage"
              :class="[
                'rounded-lg px-4 py-2.5 text-sm',
                'bg-green-500/10 text-green-400',
              ]"
            >
              {{ systemMessage.text }}
            </div>
          </div>
        </div>
      </section>

      <section
        id="settings-routes"
        ref="routesSection"
        class="scroll-mt-4 rounded-xl border border-border bg-surface p-5"
      >
        <h2 class="mb-4 flex items-center gap-2 text-base font-semibold text-foreground">
          <RouteIcon class="h-5 w-5 text-accent" />
          路由
        </h2>

        <p class="mb-4 text-sm text-muted">
          配置分流路由，仅将指定网段的流量通过 VPN 隧道。
        </p>

        <div class="mb-4 flex items-center gap-3">
          <input
            v-model="newRoute"
            type="text"
            placeholder="例如 10.0.0.0/8"
            class="min-w-0 flex-1 rounded-lg border border-border bg-bg px-4 py-2.5 font-mono text-sm text-foreground placeholder:text-muted transition-colors focus:border-accent/50 focus:outline-none"
            @keyup.enter="addRoute"
          />
          <button
            :disabled="routesBusy"
            class="flex items-center gap-2 rounded-lg border border-border px-4 py-2.5 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
            @click="addRoute"
          >
            <Plus class="h-4 w-4" />
            添加
          </button>
        </div>

        <div class="space-y-3">
          <div
            v-for="(routeItem, index) in routes"
            :key="routeItem"
            class="flex items-center gap-3 rounded-lg border border-border bg-bg px-4 py-2.5"
          >
            <span class="min-w-0 flex-1 truncate font-mono text-sm text-foreground">{{ routeItem }}</span>
            <button
              :disabled="routesBusy"
              class="text-muted transition-colors hover:text-destructive disabled:opacity-50"
              @click="removeRoute(index)"
            >
              <Trash2 class="h-4 w-4" />
            </button>
          </div>
        </div>

        <div
          v-if="routeMessage"
          :class="[
            'mt-4 rounded-lg px-4 py-2.5 text-sm',
            'bg-green-500/10 text-green-400',
          ]"
        >
          {{ routeMessage.text }}
        </div>
        <p v-else class="mt-4 text-xs text-muted">
          点击添加或垃圾桶后立即写入配置，无需再保存。
        </p>
      </section>
    </div>

    <nav class="fixed bottom-6 right-6 z-30 flex flex-col items-center gap-2 rounded-full border border-border bg-surface/90 p-2 shadow-xl shadow-black/25 backdrop-blur">
      <button
        v-for="section in sections"
        :key="section.key"
        type="button"
        :title="section.label"
        :aria-label="section.label"
        :class="[
          'group flex items-center gap-2 rounded-full border transition-all duration-200',
          activeSection === section.key
            ? 'h-9 border-accent bg-accent/15 px-3 text-foreground'
            : 'h-7 border-transparent px-2 text-muted hover:border-accent/50 hover:text-foreground',
        ]"
        @click="scrollToSection(section.key)"
      >
        <component
          :is="section.icon"
          :class="[
            'shrink-0 transition-all duration-200',
            activeSection === section.key ? 'h-4 w-4 text-accent' : 'h-3.5 w-3.5 text-muted group-hover:text-foreground',
          ]"
        />
        <span
          :class="[
            'overflow-hidden whitespace-nowrap text-xs transition-all duration-200',
            activeSection === section.key ? 'max-w-12 opacity-100' : 'max-w-0 opacity-0',
          ]"
        >
          {{ section.label }}
        </span>
      </button>
    </nav>
  </div>
</template>

<style scoped>
.settings-scroll {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.settings-scroll::-webkit-scrollbar {
  display: none;
}
</style>
