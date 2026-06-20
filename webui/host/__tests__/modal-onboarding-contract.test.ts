import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()

function readSource(...parts: string[]) {
  return readFileSync(join(webuiRoot, ...parts), 'utf8')
}

describe('modal onboarding and credential contracts', () => {
  it('handles core-owned quick-start-request events in the renderer', () => {
    const exvTypes = readSource('src', 'types', 'exv.d.ts')
    const uiStore = readSource('src', 'stores', 'ui.ts')
    const useSse = readSource('src', 'composables', 'useSSE.ts')

    assert.match(exvTypes, /interface QuickStartRequestEvent/)
    assert.match(exvTypes, /reason:\s*'missing'\s*\|\s*'invalid'/)
    assert.match(uiStore, /showQuickStart/)
    assert.match(uiStore, /openQuickStart\(/)
    assert.match(useSse, /event\.type === 'quick-start-request'/)
    assert.match(useSse, /ui\.openQuickStart/)
  })

  it('exposes a typed credential prompt instead of password-only connect resolution', () => {
    const uiStore = readSource('src', 'stores', 'ui.ts')
    const vpnStore = readSource('src', 'stores', 'vpn.ts')

    assert.match(uiStore, /interface CredentialPromptRequest/)
    assert.match(uiStore, /requestCredentials\(/)
    assert.match(uiStore, /rememberPassword/)
    assert.match(vpnStore, /resolveConnectCredentials/)
    assert.doesNotMatch(vpnStore, /resolveConnectPassword\(/)
  })

  it('keeps frontend from owning service_install_prompt_seen', () => {
    const configStore = readSource('src', 'stores', 'config.ts')
    const app = readSource('src', 'App.vue')

    assert.doesNotMatch(configStore, /exv:service-install-prompt-seen/)
    assert.doesNotMatch(configStore, /delete remoteSettings\.service_install_prompt_seen/)
    assert.doesNotMatch(app, /markServicePromptSeen/)
    assert.doesNotMatch(app, /serviceInstallPrompt\(/)
  })

  it('mounts all in-window modals through the shared clipped modal shell', () => {
    const globalStack = readSource('src', 'windows', 'GlobalWindowStack.vue')
    const modalShell = readSource('src', 'components', 'ModalShell.vue')
    const modalConsumers = [
      'ErrorDialog.vue',
      'ConfirmDialog.vue',
      'AuthContinuationDialog.vue',
      'CoreCrashed.vue',
      'ServiceInstallLoadingOverlay.vue',
    ]

    assert.match(modalShell, /class="modal-shell__scrim"/)
    assert.match(modalShell, /position:\s*absolute/)
    assert.match(modalShell, /inset:\s*0/)
    assert.doesNotMatch(modalShell, /Teleport/)
    assert.match(globalStack, /<ErrorDialog \/>[\s\S]*<\/template>/)

    for (const file of modalConsumers) {
      const source = readSource('src', 'components', file)
      assert.match(source, /ModalShell/)
      assert.doesNotMatch(source, /fixed inset-0/)
      assert.doesNotMatch(source, /Teleport/)
    }
  })

  it('credential prompt supports missing username, missing password, and remembered password saves', () => {
    const dialog = readSource('src', 'components', 'CredentialPromptDialog.vue')
    const vpnStore = readSource('src', 'stores', 'vpn.ts')

    assert.match(dialog, /missingUsername/)
    assert.match(dialog, /missingPassword/)
    assert.match(dialog, /rememberPassword/)
    assert.match(dialog, /credentialPromptTitle/)
    assert.match(dialog, /请输入用户名和密码/)
    assert.match(dialog, /请输入用户名/)
    assert.match(dialog, /请输入密码/)
    assert.match(dialog, /ui\.submitCredentialPrompt/)
    assert.doesNotMatch(dialog, /title="补全连接凭据"/)
    assert.doesNotMatch(dialog, /:description=/)
    assert.match(vpnStore, /const missingUsername = !auth\.username\.trim\(\)/)
    assert.match(vpnStore, /const missingPassword = !\(auth\.remember_password && auth\.password_stored\)/)
    assert.match(vpnStore, /remember_password: credentials\.rememberPassword/)
    assert.match(vpnStore, /password: credentials\.rememberPassword \? credentials\.password/)
    assert.doesNotMatch(vpnStore, /请补全 VPN 连接凭据/)
    assert.match(vpnStore, /密码不正确，请重新输入密码/)
  })

  it('quick start supports quick/custom modes, import, skip, and service install default', () => {
    const dialog = readSource('src', 'components', 'QuickStartDialog.vue')
    const globalStack = readSource('src', 'windows', 'GlobalWindowStack.vue')

    assert.match(dialog, /mode = ref<'quick' \| 'custom'>\('quick'\)/)
    assert.match(dialog, /vpn-ct\.ecnu\.edu\.cn/)
    assert.match(dialog, /rememberPassword = ref\(false\)/)
    assert.match(dialog, /rememberPasswordEnabled/)
    assert.match(dialog, /remember_password:\s*rememberPassword\.value/)
    assert.match(dialog, /password:\s*rememberPassword\.value \? password\.value : ''/)
    assert.doesNotMatch(dialog, /remember_password:\s*true/)
    assert.match(dialog, /installService/)
    assert.match(dialog, /TokenInput/)
    assert.match(dialog, /settingsForm\.mtu/)
    assert.match(dialog, /settingsForm\.dtls/)
    assert.match(dialog, /importConfig/)
    assert.match(dialog, /skip\(\)/)
    assert.match(globalStack, /<QuickStartDialog \/>/)
  })

  it('styles native select options with readable dark theme colors', () => {
    const settings = [
      readSource('src', 'pages', 'SettingsPage.vue'),
      readSource('src', 'pages', 'settings', 'SettingsAuthSection.vue'),
      readSource('src', 'pages', 'settings', 'SettingsSystemSection.vue'),
    ].join('\n')
    const css = readSource('src', 'style.css')

    assert.match(settings, /class="[^"]*exv-select/)
    assert.match(css, /\.exv-select option/)
    assert.match(css, /background:\s*var\(--select-option-bg\)/)
    assert.match(css, /color:\s*var\(--select-option-fg\)/)
  })

  it('centralizes route pages and global window stack in the windows registry', () => {
    const router = readSource('src', 'router', 'index.ts')
    const app = readSource('src', 'App.vue')
    const windowsIndex = readSource('src', 'windows', 'index.ts')
    const routes = readSource('src', 'windows', 'routes.ts')
    const globalStack = readSource('src', 'windows', 'GlobalWindowStack.vue')

    assert.match(router, /import \{ routeRecords \} from '\.\.\/windows'/)
    assert.match(router, /routes:\s*routeRecords/)
    assert.match(app, /import \{ GlobalWindowStack \} from '\.\/windows'/)
    assert.match(app, /<GlobalWindowStack/)
    assert.match(windowsIndex, /export \{ default as GlobalWindowStack \}/)
    assert.match(routes, /DashboardPage/)
    assert.match(routes, /SettingsPage/)
    assert.match(routes, /LogsPage/)
    assert.match(globalStack, /CredentialPromptDialog/)
    assert.match(globalStack, /QuickStartDialog/)
  })

  it('splits settings into section components and keeps the shell as orchestration only', () => {
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const auth = readSource('src', 'pages', 'settings', 'SettingsAuthSection.vue')
    const connection = readSource('src', 'pages', 'settings', 'SettingsConnectionSection.vue')
    const personalization = readSource('src', 'pages', 'settings', 'SettingsPersonalizationSection.vue')
    const system = readSource('src', 'pages', 'settings', 'SettingsSystemSection.vue')
    const routes = readSource('src', 'pages', 'settings', 'SettingsRoutesSection.vue')
    const axis = readSource('src', 'pages', 'settings', 'SettingsSectionAxis.vue')

    assert.match(settings, /SettingsAuthSection/)
    assert.match(settings, /SettingsConnectionSection/)
    assert.match(settings, /SettingsPersonalizationSection/)
    assert.match(settings, /SettingsSystemSection/)
    assert.match(settings, /SettingsRoutesSection/)
    assert.match(settings, /SettingsSectionAxis/)
    assert.doesNotMatch(settings, /VPN 服务器/)
    assert.doesNotMatch(settings, /Helper 服务/)
    assert.doesNotMatch(settings, /点击添加或垃圾桶后立即写入配置/)
    const changeRegistry = readSource('src', 'pages', 'settings', 'changeRegistry.ts')
    const backendMapSource = changeRegistry.slice(
      changeRegistry.indexOf('export const frontendToBackendFieldMap'),
      changeRegistry.indexOf('export const frontendOnlySettingsFields'),
    )

    assert.match(settings, /请输入用户名后再开启记住密码/)
    assert.match(auth, /rememberPasswordDisabled/)
    assert.match(auth, /remember_password:\s*false/)
    assert.doesNotMatch(auth, /saveAuth\(/)
    assert.doesNotMatch(auth, /保存认证设置/)
    assert.doesNotMatch(connection, /saveConnection\(/)
    assert.doesNotMatch(connection, /保存连接设置/)
    assert.match(personalization, /ThemeMode/)
    assert.match(personalization, /theme\.setThemeMode\(mode\)/)
    assert.match(personalization, /theme\.setAccent\(activeAccentTheme\.value/)
    assert.match(system, /Helper 服务/)
    assert.doesNotMatch(system, /saveSystem\(/)
    assert.doesNotMatch(system, /保存系统设置/)
    assert.match(routes, /include_class_a_private_routes/)
    assert.match(routes, /include_class_b_private_routes/)
    assert.match(routes, /将大段A类\/B类内网地址经由 exv 处理可能导致本机物理局域网内某些资源无法访问/)
    assert.match(axis, /activationToken/)
    assert.match(axis, /setTimeout/)
    assert.match(axis, /settings-axis/)
    assert.match(changeRegistry, /frontendToBackendFieldMap/)
    assert.match(changeRegistry, /frontendOnlySettingsFields/)
    assert.match(changeRegistry, /auth\.username/)
    assert.match(changeRegistry, /settings\.windows_tunnel_driver/)
    assert.doesNotMatch(changeRegistry, /theme\.mode/)
    assert.doesNotMatch(backendMapSource, /theme\.mode/)
  })

  it('stages setting edits and saves only changed fields from the fixed page header', () => {
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const configStore = readSource('src', 'stores', 'config.ts')
    const changeRegistry = readSource('src', 'pages', 'settings', 'changeRegistry.ts')

    assert.match(settings, /settings-header/)
    assert.match(settings, /settings-scroll-body/)
    assert.match(settings, /v-if="dirtyChanges\.length"/)
    assert.match(settings, /保存修改/)
    assert.match(settings, /pendingChangeRows/)
    assert.match(settings, /@mouseenter="saveHover = true"/)
    assert.match(settings, /@mouseleave="saveHover = false"/)
    assert.match(settings, /buildDirtyBackendPayloads/)
    assert.match(settings, /config\.saveAuthConfig\(authPayload\)/)
    assert.match(settings, /config\.saveSettings\(settingsPayload\)/)
    assert.doesNotMatch(settings, /theme\.setThemeMode\(themeDraft\.mode\)/)
    assert.doesNotMatch(settings, /theme\.setAccent\('light', themeDraft\.lightAccent\)/)
    assert.doesNotMatch(settings, /theme\.setAccent\('dark', themeDraft\.darkAccent\)/)
    assert.match(settings, /请输入用户名后再开启记住密码/)

    assert.match(changeRegistry, /SettingChangeKey/)
    assert.match(changeRegistry, /backendGroup:\s*'auth'/)
    assert.match(changeRegistry, /backendGroup:\s*'settings'/)
    assert.match(changeRegistry, /auth\.remember_password/)
    assert.match(changeRegistry, /settings\.mtu/)
    assert.match(changeRegistry, /settings\.extra_args/)
    assert.doesNotMatch(changeRegistry, /theme\.lightAccent/)
    assert.doesNotMatch(changeRegistry, /theme\.darkAccent/)
    assert.match(changeRegistry, /settings\.include_class_a_private_routes/)
    assert.match(changeRegistry, /settings\.include_class_b_private_routes/)

    assert.match(configStore, /const payload: Partial<AuthConfig> = \{\}/)
    assert.doesNotMatch(configStore, /server:\s*config\.server \?\? ''/)
    assert.match(configStore, /Object\.prototype\.hasOwnProperty\.call\(config, 'server'\)/)
  })

  it('keeps save affordance beside the settings title and above form fields', () => {
    const settings = readSource('src', 'pages', 'SettingsPage.vue')

    assert.match(settings, /<h1[^>]*>设置<\/h1>[\s\S]*v-if="dirtyChanges\.length"[\s\S]*保存修改/)
    assert.match(settings, /settings-save-popover/)
    assert.match(settings, /z-\[80\]/)
    assert.doesNotMatch(settings, /justify-between/)
  })

  it('supports frontend-only light dark and system themes outside quick start', () => {
    const themeStore = readSource('src', 'stores', 'theme.ts')
    const css = readSource('src', 'style.css')
    const app = readSource('src', 'App.vue')
    const quickStart = readSource('src', 'components', 'QuickStartDialog.vue')

    assert.match(themeStore, /export type ThemeMode = 'light' \| 'dark' \| 'system'/)
    assert.match(themeStore, /exv:theme-mode/)
    assert.match(themeStore, /matchMedia\('\(prefers-color-scheme: dark\)'\)/)
    assert.match(themeStore, /document\.documentElement\.dataset\.theme/)
    assert.match(css, /\[data-theme="light"\]/)
    assert.match(css, /--color-bg:\s*#ffffff/i)
    assert.match(css, /--color-accent:\s*#2563eb/i)
    assert.match(app, /theme\.initialize\(\)/)
    assert.doesNotMatch(quickStart, /theme/i)
  })

  it('offers per-theme accent palettes and applies accent tokens to topology', () => {
    const themeStore = readSource('src', 'stores', 'theme.ts')
    const personalization = readSource('src', 'pages', 'settings', 'SettingsPersonalizationSection.vue')
    const configStore = readSource('src', 'stores', 'config.ts')
    const css = readSource('src', 'style.css')
    const dashboard = readSource('src', 'pages', 'DashboardPage.vue')

    assert.match(themeStore, /export type ThemeAccentTheme = 'light' \| 'dark'/)
    assert.match(themeStore, /exv:accent-light/)
    assert.match(themeStore, /exv:accent-dark/)
    assert.match(themeStore, /#A41F35/i)
    assert.match(themeStore, /#22C55E/i)
    assert.match(themeStore, /document\.documentElement\.style\.setProperty\('--color-accent'/)
    assert.match(personalization, /themeAccentPalettes/)
    assert.match(personalization, /强调色/)
    assert.match(personalization, /activeAccentOptions/)
    assert.match(personalization, /theme\.setThemeMode\(mode\)/)
    assert.match(personalization, /theme\.setAccent\(activeAccentTheme\.value/)
    assert.doesNotMatch(personalization, /浅色 Accent/)
    assert.doesNotMatch(personalization, /深色 Accent/)
    assert.match(configStore, /remember_password:\s*false/)
    assert.match(configStore, /Object\.prototype\.hasOwnProperty\.call\(config, 'remember_password'\)/)
    assert.match(css, /--color-accent-rgb/)
    assert.match(css, /\[data-theme="light"\][\s\S]*--color-warning:\s*#8a5a24/i)
    assert.match(css, /\[data-theme="light"\][\s\S]*--color-success:\s*#2f7d59/i)
    assert.match(css, /--color-warning-rgb/)
    assert.match(css, /--color-success-rgb/)
    assert.match(css, /--topology-accent-stroke/)
    assert.match(dashboard, /var\(--color-warning\)/)
    assert.match(dashboard, /var\(--topology-warning-glow\)/)
    assert.doesNotMatch(dashboard, /rgba\(245,\s*158,\s*11/)
    assert.doesNotMatch(dashboard, /rgb\(245[,\s]+158[,\s]+11/)
    assert.match(dashboard, /padding-bottom:\s*5\.75rem/)
    assert.match(dashboard, /margin-top:\s*-12rem/)
    assert.match(dashboard, /var\(--topology-accent-stroke\)/)
    assert.match(dashboard, /var\(--topology-accent-glow\)/)
    assert.doesNotMatch(dashboard, /rgba\(34,\s*197,\s*94,\s*0\.78\)/)
    assert.doesNotMatch(dashboard, /rgb\(34\s+197\s+94\)/)
  })

  it('uses semantic health color for success messages instead of accent', () => {
    const auth = readSource('src', 'pages', 'settings', 'SettingsAuthSection.vue')
    const legacyAuth = readSource('src', 'pages', 'AuthPage.vue')
    const toast = readSource('src', 'components', 'Toast.vue')
    const connection = readSource('src', 'pages', 'settings', 'SettingsConnectionSection.vue')
    const routes = readSource('src', 'pages', 'settings', 'SettingsRoutesSection.vue')
    const system = readSource('src', 'pages', 'settings', 'SettingsSystemSection.vue')

    for (const source of [auth, legacyAuth, connection, routes, system, toast]) {
      assert.doesNotMatch(source, /bg-green-500\/10/)
      assert.doesNotMatch(source, /text-green-400/)
    }
    assert.match(legacyAuth, /bg-success\/10/)
    assert.match(toast, /toast\.type === 'success' \? 'bg-success\/10 border-success\/30 text-success'/)
  })

  it('applies requested page heading log and sidebar UI changes', () => {
    const dashboard = readSource('src', 'pages', 'DashboardPage.vue')
    const logs = readSource('src', 'pages', 'LogsPage.vue')
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const nav = readSource('src', 'components', 'NavBar.vue')

    assert.match(dashboard, /<h1[^>]*text-3xl[\s\S]*主面板/)
    assert.match(settings, /<h1[^>]*text-3xl[\s\S]*设置/)
    assert.match(logs, /<h1[^>]*text-3xl[\s\S]*日志/)
    assert.doesNotMatch(logs, /FileText/)
    assert.doesNotMatch(nav, /连接方式/)
    assert.doesNotMatch(nav, /connectionMethodLabel/)
    assert.match(nav, /showSidebarStatusDetails = computed\(\(\) => Boolean\(vpn\.status\?\.connected\)\)/)
  })

  it('uses one export config action with a password choice dialog and corrected icons', () => {
    const system = readSource('src', 'pages', 'settings', 'SettingsSystemSection.vue')

    assert.match(system, /showExportDialog/)
    assert.match(system, /exportIncludePassword/)
    assert.match(system, /exportPasswordRequired/)
    assert.match(system, /config\.exportConfig\(\{\s*protected:\s*exportIncludePassword\.value/)
    assert.match(system, /导入配置[\s\S]*<Download/)
    assert.match(system, /导出配置[\s\S]*<Upload/)
    assert.doesNotMatch(system, /exportConfigUnprotected/)
    assert.doesNotMatch(system, /exportConfigProtected/)
    assert.doesNotMatch(system, /导出配置（不含密码）/)
    assert.doesNotMatch(system, /导出配置（密码保护）/)
  })

  it('routes native WebView CLI actions through desktop RPC instead of unsupported stubs', () => {
    const win32Host = readSource('..', 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp')
    const darwinHost = readSource('..', 'src', 'platform', 'darwin', 'ui_shell', 'wk_webview_host_darwin.mm')
    const systemActions = readSource('..', 'src', 'core', 'app_api', 'desktop_system_actions.cpp')

    for (const source of [win32Host, darwinHost]) {
      assert.match(source, /status:\s*\(\) => rpc\('cli\.status'\)/)
      assert.match(source, /install:\s*\(\) => rpc\('cli\.install'\)/)
      assert.match(source, /uninstall:\s*\(\) => rpc\('cli\.uninstall'\)/)
      assert.doesNotMatch(source, /unsupported\('cli\.install'\)/)
    }
    assert.match(systemActions, /"cli\.status"/)
    assert.match(systemActions, /"cli\.install"/)
    assert.match(systemActions, /"cli\.uninstall"/)
  })

  it('keeps settings axis compact and uses hand cursors only on interactive controls', () => {
    const axis = readSource('src', 'pages', 'settings', 'SettingsSectionAxis.vue')
    const css = readSource('src', 'style.css')
    const dashboard = readSource('src', 'pages', 'DashboardPage.vue')

    assert.match(axis, /cursor-pointer/)
    assert.match(axis, /w-auto/)
    assert.match(axis, /px-2/)
    assert.doesNotMatch(axis, /w-24/)
    assert.doesNotMatch(axis, /max-w-16/)

    assert.match(css, /button:not\(:disabled\)/)
    assert.match(css, /input\[type="checkbox"\]:not\(:disabled\)/)
    assert.match(css, /input\[type="radio"\]:not\(:disabled\)/)
    assert.doesNotMatch(dashboard, /topology-[^"'`\n]*cursor-pointer/)
  })

  it('supports launch at login and gated auto connect on launch settings', () => {
    const app = readSource('src', 'App.vue')
    const configStore = readSource('src', 'stores', 'config.ts')
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const connection = readSource('src', 'pages', 'settings', 'SettingsConnectionSection.vue')
    const system = readSource('src', 'pages', 'settings', 'SettingsSystemSection.vue')
    const registry = readSource('src', 'pages', 'settings', 'changeRegistry.ts')
    const quickStart = readSource('src', 'components', 'QuickStartDialog.vue')

    assert.match(configStore, /launch_at_login:\s*boolean/)
    assert.match(configStore, /auto_connect_on_launch:\s*boolean/)
    assert.match(configStore, /launch_at_login:\s*false/)
    assert.match(configStore, /auto_connect_on_launch:\s*false/)
    assert.match(settings, /launch_at_login:\s*false/)
    assert.match(settings, /auto_connect_on_launch:\s*false/)

    assert.match(system, /开机自启/)
    assert.match(system, /launch_at_login/)
    assert.match(system, /launchAtLoginModel/)
    assert.match(system, /<ToggleSwitch/)

    assert.match(connection, /启动时自动连接/)
    assert.match(connection, /auto_connect_on_launch/)
    assert.match(connection, /startupAutoConnectAllowed/)
    assert.match(connection, /remember_password/)
    assert.match(connection, /password_stored/)
    assert.match(connection, /serviceInstalled/)

    assert.match(registry, /settings\.launch_at_login/)
    assert.match(registry, /settings\.auto_connect_on_launch/)
    assert.match(registry, /backendField:\s*'launch_at_login'/)
    assert.match(registry, /backendField:\s*'auto_connect_on_launch'/)

    assert.match(app, /maybeAutoConnectOnLaunch/)
    assert.match(app, /auto_connect_on_launch/)
    assert.match(app, /remember_password/)
    assert.match(app, /password_stored/)
    assert.match(app, /serviceInstalled/)
    assert.match(app, /vpn\.connect\(\)/)
    assert.doesNotMatch(quickStart, /auto_connect_on_launch/)
  })

  it('persists settings and logs page state across page switches', () => {
    const app = readSource('src', 'App.vue')
    const pageState = readSource('src', 'stores', 'pageState.ts')
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const logs = readSource('src', 'pages', 'LogsPage.vue')

    assert.match(app, /<KeepAlive/)
    assert.match(app, /SettingsPage/)
    assert.match(app, /LogsPage/)

    assert.match(pageState, /usePageStateStore/)
    assert.match(pageState, /settings:[\s\S]*scrollTop/)
    assert.match(pageState, /settings:[\s\S]*authDraft/)
    assert.match(pageState, /rememberSettingsDrafts/)
    assert.match(pageState, /logs:[\s\S]*scrollTop/)
    assert.match(pageState, /logs:[\s\S]*userScrolled/)

    assert.match(settings, /usePageStateStore/)
    assert.match(settings, /restoreCachedSettingsDrafts/)
    assert.match(settings, /rememberSettingsDrafts/)
    assert.match(settings, /pageState\.settings\.scrollTop/)
    assert.match(settings, /pageState\.settings\.userInteracted/)

    assert.match(logs, /usePageStateStore/)
    assert.match(logs, /pageState\.logs\.scrollTop/)
    assert.match(logs, /pageState\.logs\.userScrolled/)
    assert.match(logs, /handleLogsScroll/)
    assert.match(logs, /@scroll="handleLogsScroll"/)
  })
})
