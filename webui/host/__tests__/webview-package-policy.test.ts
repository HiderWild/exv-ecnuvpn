import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()
const repoRoot = join(webuiRoot, '..')
const requireFromWebui = createRequire(join(webuiRoot, 'package.json'))
const { getBuildLayout } = requireFromWebui('./scripts/build-layout.cjs') as {
  getBuildLayout: () => {
    rendererTarget: string
    rendererOutDir: string
    webviewRendererOutDir: string
  }
}

function powershellCase(script: string, label: string): string {
  const start = script.indexOf(`  '${label}' {`)
  assert.notEqual(start, -1, `missing PowerShell case ${label}`)
  const next = script.indexOf("\n  '", start + 1)
  return script.slice(start, next === -1 ? script.length : next)
}

function bashCase(script: string, label: string): string {
  const start = script.indexOf(`  ${label})`)
  assert.notEqual(start, -1, `missing bash case ${label}`)
  const end = script.indexOf('\n    ;;', start)
  assert.notEqual(end, -1, `missing bash case terminator ${label}`)
  return script.slice(start, end)
}

function assertUnixDesktopScriptPackagesWebView(script: string, platform: string): void {
  assert.match(script, new RegExp(`ECNUVPN_BUILD_PLATFORM=${platform}`))
  assert.match(script, new RegExp(`ECNUVPN_WEBUI_DIST_DIR="\\$BUILD_ROOT/webview/dist"`))
  assert.match(script, /-DEXV_BUILD_UI_SHELL=ON/)
  assert.match(script, /package_webview\(\)/)
  assert.match(script, /python3 scripts\/package_ui_shell\.py/)
  assert.doesNotMatch(script, /pnpm run webview:package/)
  const desktopCase = bashCase(script, 'desktop')
  const allCase = bashCase(script, 'all')
  assert.match(desktopCase, /build_cpp on/)
  assert.match(allCase, /build_cpp on/)
  assert.match(desktopCase, /package_webview/)
  assert.match(allCase, /package_webview/)
  assert.doesNotMatch(desktopCase, /compile_electron|package_desktop/)
  assert.doesNotMatch(allCase, /compile_electron|package_desktop/)
}

describe('native WebView package policy', () => {
  it('makes WebView renderer output the default neutral build target', () => {
    const layout = getBuildLayout()
    assert.equal(layout.rendererTarget, 'webview')
    assert.equal(layout.rendererOutDir, layout.webviewRendererOutDir)
    assert.doesNotMatch(layout.rendererOutDir.replace(/\\/g, '/'), /\/electron\/dist$/)
  })

  it('retires Electron from production package dependencies and scripts', () => {
    const packageJson = JSON.parse(readFileSync(join(webuiRoot, 'package.json'), 'utf8'))
    assert.equal(packageJson.devDependencies.electron, undefined)
    assert.equal(packageJson.devDependencies['electron-builder'], undefined)
    assert.equal(packageJson.devDependencies['@types/electron'], undefined)
    assert.equal(packageJson.scripts['desktop:dev'], undefined)
    assert.equal(packageJson.scripts['electron:dev'], undefined)
    assert.equal(packageJson.scripts['electron:start'], undefined)
    assert.equal(packageJson.scripts['build:electron'], undefined)
    assert.equal(packageJson.scripts['desktop:compile'], undefined)
    assert.equal(packageJson.scripts['desktop:package'], undefined)
    assert.equal(packageJson.scripts['desktop:package:dir'], undefined)
    assert.equal(packageJson.scripts['desktop:debug'], undefined)
    assert.equal(packageJson.scripts['desktop:build'], undefined)
    assert.equal(packageJson.main, undefined)
    assert.equal(packageJson.pnpm?.onlyBuiltDependencies?.includes('electron') ?? false, false)

    assert.match(packageJson.scripts['webview:compile'], /ECNUVPN_RENDERER_TARGET=webview/)
    assert.match(packageJson.scripts['webview:package'], /package_ui_shell\.py/)
    assert.match(packageJson.scripts['test:contract'], /run-host-test\.cjs/)
  })

  it('makes platform desktop build scripts package the WebView shell by default', () => {
    const windows = readFileSync(join(repoRoot, 'scripts', 'build-windows.ps1'), 'utf8')
    const macos = readFileSync(join(repoRoot, 'scripts', 'build-macos.sh'), 'utf8')
    const linux = readFileSync(join(repoRoot, 'scripts', 'build-linux.sh'), 'utf8')

    assert.match(windows, /ECNUVPN_WEBUI_DIST_DIR = Join-Path \$buildRoot 'webview\\dist'/)
    assert.match(windows, /-DEXV_BUILD_UI_SHELL=ON/)
    assert.match(windows, /function Resolve-WebView2Sdk/)
    assert.match(windows, /build\\deps\\webview2\\1\.0\.4022\.49/)
    assert.match(windows, /-DWEBVIEW2_SDK_DIR=\$resolvedWebView2Sdk/)
    assert.match(windows, /function Invoke-WebViewPackage/)
    const windowsDesktop = powershellCase(windows, 'desktop')
    const windowsAll = powershellCase(windows, 'all')
    assert.match(windowsDesktop, /Invoke-CppBuild -UiShell/)
    assert.match(windowsAll, /Invoke-CppBuild -UiShell/)
    assert.match(windowsDesktop, /Invoke-WebViewPackage/)
    assert.match(windowsAll, /Invoke-WebViewPackage/)
    assert.doesNotMatch(windowsDesktop, /Invoke-ElectronCompile|Invoke-DesktopPackage/)
    assert.doesNotMatch(windowsAll, /Invoke-ElectronCompile|Invoke-DesktopPackage/)
    assert.doesNotMatch(windows, /desktop:compile|desktop:package|build:electron|electron\\release|electron-builder/i)

    assertUnixDesktopScriptPackagesWebView(macos, 'macos')
    assertUnixDesktopScriptPackagesWebView(linux, 'linux')
    assert.doesNotMatch(macos, /desktop:compile|desktop:package|build:electron|electron\/release|electron-builder/i)
  })

  it('removes Electron adapter source and config from production webui paths', () => {
    assert.equal(existsSync(join(webuiRoot, 'scripts', 'run-host-test.cjs')), true)
    assert.equal(existsSync(join(webuiRoot, 'scripts', 'run-electron-test.cjs')), false)
    assert.equal(existsSync(join(webuiRoot, 'scripts', 'build-electron.cjs')), false)
    assert.equal(existsSync(join(webuiRoot, 'scripts', 'prepare-native.cjs')), false)
    assert.equal(existsSync(join(webuiRoot, 'electron-builder.config.cjs')), false)
    assert.equal(existsSync(join(webuiRoot, 'tsconfig.electron.json')), false)
    assert.equal(existsSync(join(webuiRoot, 'desktop', 'main')), false)
    assert.equal(existsSync(join(webuiRoot, 'desktop', 'preload')), false)
    assert.equal(existsSync(join(webuiRoot, 'build-resources')), false)
  })

  it('does not let native WebView packages fall back to Electron renderer output', () => {
    const packageScript = readFileSync(join(repoRoot, 'scripts', 'package_ui_shell.py'), 'utf8')
    assert.doesNotMatch(packageScript, /build"\s*\/\s*platform\s*\/\s*"electron"\s*\/\s*"dist"/)
    assert.doesNotMatch(packageScript, /electron"\s*,\s*"dist"/)
    assert.match(packageScript, /WebView2Loader\.dll/)
  })

  it('writes packaged launch arguments for exv-ui', () => {
    const packageScript = readFileSync(join(repoRoot, 'scripts', 'package_ui_shell.py'), 'utf8')
    assert.match(packageScript, /def write_launch_args\(/)
    assert.match(packageScript, /exv-ui\.args/)
    assert.match(packageScript, /--exv/)
    assert.match(packageScript, /--renderer-index/)
    assert.match(packageScript, /MINGW_RUNTIME_DLLS/)
    assert.match(packageScript, /wintun\.dll/)
    assert.match(packageScript, /copy_windows_runtime_assets/)
    assert.match(packageScript, /def validate_launch_args_targets\(/)
    assert.match(packageScript, /Launch args target not found/)
    assert.match(packageScript, /--verify-launch-targets-only/)
    assert.match(packageScript, /--package-dir/)

    const readme = readFileSync(join(webuiRoot, 'README.md'), 'utf8')
    assert.match(readme, /exv-ui\.args/)
    assert.match(readme, /--renderer-index/)
  })

  it('keeps active packaging smoke and merge-prep scripts on WebView package paths', () => {
    const windowsSmoke = readFileSync(join(repoRoot, 'scripts', 'windows-packaging-smoke.ps1'), 'utf8')
    const macosSmoke = readFileSync(join(repoRoot, 'scripts', 'macos-packaging-smoke.sh'), 'utf8')
    const windowsMergePrep = readFileSync(join(repoRoot, 'scripts', 'validate-merge-prep-windows.ps1'), 'utf8')
    const macosMergePrep = readFileSync(join(repoRoot, 'scripts', 'validate-merge-prep-macos.sh'), 'utf8')

    assert.match(windowsSmoke, /build\\windows\\webview\\package\\ECNU VPN/)
    assert.match(windowsSmoke, /WebView2Loader\.dll/)
    assert.match(windowsSmoke, /electron\.exe/)
    assert.doesNotMatch(windowsSmoke, /windows\\electron|dist-electron/i)

    assert.match(macosSmoke, /build\/macos\/webview\/package\/ECNU VPN/)
    assert.doesNotMatch(macosSmoke, /build\/macos\/electron|Electron \.app|ELECTRON_RELEASE/)

    assert.match(windowsMergePrep, /build-windows\.ps1'.*'desktop'/s)
    assert.doesNotMatch(windowsMergePrep, /build:electron|prepare:native|Electron main\/preload/)
    assert.match(macosMergePrep, /bash "\$SCRIPT_DIR\/build-macos\.sh" desktop/)
    assert.doesNotMatch(macosMergePrep, /build:electron|prepare:native|Electron main\/preload/)
  })

  it('keeps start.ps1 desktop flow on native WebView shell paths', () => {
    const startPs1 = readFileSync(join(repoRoot, 'start.ps1'), 'utf8')

    assert.match(startPs1, /build\\windows\\webview\\package\\ECNU VPN/)
    assert.match(startPs1, /exv-ui\.exe/)
    assert.match(startPs1, /function Resolve-WebView2Sdk/)
    assert.match(startPs1, /-DWEBVIEW2_SDK_DIR=\$resolvedWebView2Sdk/)
    assert.doesNotMatch(startPs1, /build\\windows\\electron|dist-electron|desktop:package|desktop:package:dir|build:electron/i)
    assert.doesNotMatch(startPs1, /Find-ElectronProcess|Electron process/i)
  })

  it('shows service install prompts only when the helper service is not installed', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const dashboardVue = readFileSync(join(webuiRoot, 'src', 'pages', 'DashboardPage.vue'), 'utf8')

    assert.match(appVue, /!config\.settings\.service_install_prompt_seen && !vpn\.serviceInstalled/)
    assert.doesNotMatch(appVue, /!config\.settings\.service_install_prompt_seen && !vpn\.serviceAvailable/)
    assert.match(dashboardVue, /!vpn\.serviceInstalled/)
    assert.match(dashboardVue, /服务已安装但未运行/)
  })

  it('keeps window mode resizing out of the settings persistence path', () => {
    const configStore = readFileSync(join(webuiRoot, 'src', 'stores', 'config.ts'), 'utf8')
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')

    assert.doesNotMatch(configStore, /window\.ecnuVpn\.window\.setMode/)
    assert.match(configStore, /localStorage\.getItem\('ecnu-vpn:minimal-mode'\)/)
    assert.match(configStore, /localStorage\.setItem\('ecnu-vpn:minimal-mode'/)
    assert.match(configStore, /delete remoteSettings\.minimal_mode/)
    assert.match(configStore, /settings\.value = \{ \.\.\.settings\.value, \.\.\.s \}/)
    assert.match(appVue, /watch\(\s*minimalMode/)
    assert.match(appVue, /requestAnimationFrame/)
    assert.match(appVue, /window\.ecnuVpn\?\.window\?\.setMode/)
  })

  it('keeps service install prompt seen state in frontend storage only', () => {
    const configStore = readFileSync(join(webuiRoot, 'src', 'stores', 'config.ts'), 'utf8')
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')

    assert.match(configStore, /localStorage\.getItem\('ecnu-vpn:service-install-prompt-seen'\)/)
    assert.match(configStore, /localStorage\.setItem\('ecnu-vpn:service-install-prompt-seen'/)
    assert.match(configStore, /delete remoteSettings\.service_install_prompt_seen/)
    assert.match(appVue, /await markServicePromptSeen\(\)[\s\S]*servicePromptVisible\.value = true/)
  })

  it('marks service install prompts seen before display and keeps dismiss visual-only', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const promptGateStart = appVue.indexOf('if (!config.settings.service_install_prompt_seen && !vpn.serviceInstalled)')
    const dismissStart = appVue.indexOf('function dismissServicePrompt')
    const installStart = appVue.indexOf('async function installServiceFromPrompt')
    const closeStart = appVue.indexOf('async function resolveClosePrompt')
    assert.notEqual(promptGateStart, -1)
    assert.notEqual(dismissStart, -1)
    assert.notEqual(installStart, -1)
    assert.notEqual(closeStart, -1)

    const promptGate = appVue.slice(promptGateStart, dismissStart)
    const dismissBlock = appVue.slice(dismissStart, installStart)
    const installBlock = appVue.slice(installStart, closeStart)
    assert.match(promptGate, /await markServicePromptSeen\(\)[\s\S]*servicePromptVisible\.value = true/)
    assert.doesNotMatch(dismissBlock, /await markServicePromptSeen\(\)/)
    assert.match(dismissBlock, /function dismissServicePrompt\(\)[\s\S]*servicePromptVisible\.value = false/)
    assert.doesNotMatch(installBlock, /await markServicePromptSeen\(\)/)
    assert.match(installBlock, /servicePromptVisible\.value = false[\s\S]*await vpn\.installService\(\)/)
  })

  it('defers native WebView window mode resizing outside the WebMessage callback', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const setModeBranchStart = win32Host.indexOf('if (action == "window.setMode")')
    assert.notEqual(setModeBranchStart, -1)
    const resolveCloseBranchStart = win32Host.indexOf('if (action == "window.resolveClosePrompt")', setModeBranchStart)
    assert.notEqual(resolveCloseBranchStart, -1)
    const setModeBranch = win32Host.slice(setModeBranchStart, resolveCloseBranchStart)

    assert.match(setModeBranch, /defer_window_mode\(mode,\s*mode_request\)/)
    assert.match(win32Host, /if \(action == "window\.resizeForMode"\)/)
    assert.match(win32Host, /apply_window_mode_once\(mode\)/)
    assert.doesNotMatch(win32Host, /SetTimer\(hwnd_, kWindowModeAnimationTimer/)
    assert.doesNotMatch(win32Host, /step_window_mode_animation/)
  })

  it('exposes one-shot native window actions for transparent shell transitions', () => {
    const types = readFileSync(join(webuiRoot, 'src', 'types', 'ecnu-vpn.d.ts'), 'utf8')
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const darwinHost = readFileSync(
      join(repoRoot, 'src', 'platform', 'darwin', 'ui_shell', 'wk_webview_host_darwin.mm'),
      'utf8',
    )

    assert.match(types, /resizeForMode\(mode: DesktopWindowMode, request\?: number\): Promise<\{ ok: true; mode: DesktopWindowMode \}>/)
    assert.match(types, /minimize\(\): Promise<\{ ok: true \}>/)
    assert.match(types, /requestClose\(\): Promise<\{ ok: true \}>/)
    assert.match(win32Host, /resizeForMode: \(mode, request\) => rpc\('window\.resizeForMode', \{ mode, request \}\)/)
    assert.match(win32Host, /minimize: \(\) => rpc\('window\.minimize'\)/)
    assert.match(win32Host, /requestClose: \(\) => rpc\('window\.requestClose'\)/)
    assert.match(darwinHost, /resizeForMode: \(mode, request\) => rpc\('window\.resizeForMode', \{ mode, request \}\)/)
    assert.match(darwinHost, /minimize: \(\) => rpc\('window\.minimize'\)/)
    assert.match(darwinHost, /requestClose: \(\) => rpc\('window\.requestClose'\)/)
  })

  it('wraps advanced and minimal content in the shared transparent app frame', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const frameVue = readFileSync(join(webuiRoot, 'src', 'components', 'AppWindowFrame.vue'), 'utf8')

    assert.match(appVue, /import AppWindowFrame from '\.\/components\/AppWindowFrame\.vue'/)
    assert.match(appVue, /<AppWindowFrame[\s\S]*:mode="minimalMode \? 'minimal' : 'advanced'"/)
    assert.match(appVue, /<MinimalModeView v-if="minimalMode" \/>/)
    assert.match(appVue, /<div v-else class="app-advanced-shell/)
    assert.match(frameVue, /<header[\s\S]*class="app-window-titlebar/)
    assert.match(frameVue, /<slot \/>/)
    assert.match(frameVue, /isWindows/)
    assert.match(frameVue, /isMac/)
    assert.match(frameVue, /window\.ecnuVpn\?\.window\?\.minimize/)
    assert.match(frameVue, /window\.ecnuVpn\?\.window\?\.requestClose/)
  })

  it('keeps core RPC work off the Win32 WebView message callback thread', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const runtime = readFileSync(join(repoRoot, 'src', 'app', 'ui_shell', 'ui_shell_runtime.cpp'), 'utf8')
    const onMessageStart = win32Host.indexOf('HRESULT on_web_message')
    const enqueueStart = win32Host.indexOf('void enqueue_host_request', onMessageStart)
    assert.notEqual(onMessageStart, -1)
    assert.notEqual(enqueueStart, -1)
    const onMessage = win32Host.slice(onMessageStart, enqueueStart)

    assert.match(onMessage, /enqueue_host_request\(request_json\)/)
    assert.doesNotMatch(onMessage, /handler_\s*\?\s*handler_\(request_json\)/)
    assert.match(win32Host, /kHostBridgeResponseMessage/)
    assert.match(win32Host, /std::thread/)
    assert.match(runtime, /std::thread event_pump_thread/)
    assert.match(runtime, /client\.pump_events\(\)/)
    assert.match(runtime, /runtime_config\.pump_core_events = \[\]\(\) \{\}/)
    assert.match(runtime, /event_pump_thread\.join\(\)/)
  })

  it('treats accepted VPN connect jobs as cancellable frontend state', () => {
    const store = readFileSync(join(webuiRoot, 'src', 'stores', 'vpn.ts'), 'utf8')
    const dashboard = readFileSync(join(webuiRoot, 'src', 'pages', 'DashboardPage.vue'), 'utf8')
    const minimal = readFileSync(join(webuiRoot, 'src', 'components', 'MinimalModeView.vue'), 'utf8')
    const types = readFileSync(join(webuiRoot, 'src', 'types', 'ecnu-vpn.d.ts'), 'utf8')
    const sse = readFileSync(join(webuiRoot, 'src', 'composables', 'useSSE.ts'), 'utf8')

    assert.match(store, /interface VpnConnectAccepted/)
    assert.match(store, /function isVpnConnectAccepted/)
    assert.match(store, /function updateStatusFromEvent/)
    assert.match(store, /function startConnectStatusPolling/)
    assert.match(store, /function stopConnectStatusPolling/)
    assert.match(store, /if \(isVpnConnectAccepted\(data\)\)/)
    assert.match(store, /connectInFlight\.value = true[\s\S]*startConnectStatusPolling\(\)[\s\S]*loading\.value = false/)
    assert.match(store, /if \(connectInFlight\.value && \(nextStatus\.connected \|\| nextStatus\.process_running === false\)\)/)
    assert.match(store, /stopConnectStatusPolling\(\)[\s\S]*stopAuthInteractionPolling\(\)/)
    assert.match(store, /async function cancelConnect\(\)/)
    assert.match(store, /api\.post<[^>]+>\('\/disconnect'\)/)
    assert.match(store, /user_cancelled/)
    assert.match(store, /cancelConnect, connectElevated/)
    assert.match(store, /updateStatusFromEvent,/)

    assert.match(dashboard, /if \(connecting\.value\) \{[\s\S]*vpn\.cancelConnect\(\)/)
    assert.match(minimal, /if \(connecting\.value\) \{[\s\S]*vpn\.cancelConnect\(\)/)
    assert.doesNotMatch(minimal, /const busy = computed\(\(\) => vpn\.loading \|\| vpn\.serviceBusy \|\| vpn\.connectInFlight/)
    assert.match(types, /interface VpnConnectAccepted/)
    assert.match(types, /connect\(password\?: string\): Promise<VpnStatus \| VpnConnectAccepted>/)
    assert.match(sse, /store\.updateStatusFromEvent\(event\.data as Partial<VpnStatus>\)/)
  })

  it('keeps cross-platform WebView acceptance gates executable by agents', () => {
    const windowsAcceptance = readFileSync(join(repoRoot, 'scripts', 'accept-webview-shell-windows.ps1'), 'utf8')
    const macosAcceptance = readFileSync(join(repoRoot, 'scripts', 'accept-webview-shell-macos.sh'), 'utf8')
    const linuxAcceptance = readFileSync(join(repoRoot, 'scripts', 'accept-webview-shell-linux.sh'), 'utf8')

    assert.match(windowsAcceptance, /build\\webview-acceptance\\windows/)
    assert.match(windowsAcceptance, /generate_contracts\.py --check/)
    assert.match(windowsAcceptance, /-DEXV_BUILD_UI_SHELL=ON/)
    assert.match(windowsAcceptance, /build-windows\\cpp/)
    assert.match(windowsAcceptance, /windows-packaging-smoke\.ps1/)
    assert.match(windowsAcceptance, /git diff --check/)

    assert.match(macosAcceptance, /build\/webview-acceptance\/macos/)
    assert.match(macosAcceptance, /cmake --fresh --preset macos-release/)
    assert.match(macosAcceptance, /-DCMAKE_CXX_COMPILER="\$LLVM_PREFIX\/bin\/clang\+\+"/)
    assert.match(macosAcceptance, /-DEXV_BUILD_UI_SHELL=ON/)
    assert.match(macosAcceptance, /pnpm --dir webui install --frozen-lockfile/)
    assert.match(macosAcceptance, /bash scripts\/build-macos\.sh desktop/)
    assert.match(macosAcceptance, /bash scripts\/macos-packaging-smoke\.sh/)
    assert.match(macosAcceptance, /git diff --check/)

    assert.match(linuxAcceptance, /build\/webview-acceptance\/linux/)
    assert.match(linuxAcceptance, /cmake (?:--fresh )?--preset linux-release/)
    assert.match(linuxAcceptance, /-DEXV_BUILD_UI_SHELL=ON/)
    assert.match(linuxAcceptance, /pnpm --dir webui install --frozen-lockfile/)
    assert.match(linuxAcceptance, /bash scripts\/build-linux\.sh desktop/)
    assert.match(linuxAcceptance, /git (?:-c \S+ )?diff --check/)
  })

  it('keeps manual Phase 7 capture able to probe read-only RPC responsiveness', () => {
    const manualPhase7 = readFileSync(join(repoRoot, 'scripts', 'manual-phase7-vpn-verification.ps1'), 'utf8')

    assert.match(manualPhase7, /\[switch\]\$ProbeRpc/)
    assert.match(manualPhase7, /rpc-probes\.csv/)
    assert.match(manualPhase7, /desktop-rpc/)
    assert.match(manualPhase7, /status\.get/)
    assert.match(manualPhase7, /logs\.list/)
    assert.match(manualPhase7, /rpc_timeout_ms/)
    assert.match(manualPhase7, /error_code/)
  })

  it('documents WebView as the production desktop shell in active docs', () => {
    const readme = readFileSync(join(repoRoot, 'README.md'), 'utf8')
    const buildGuide = readFileSync(join(repoRoot, 'docs', 'build_guide.md'), 'utf8')
    const userGuide = readFileSync(join(repoRoot, 'docs', 'user_guide.md'), 'utf8')

    for (const [label, text] of [
      ['README.md', readme],
      ['docs/build_guide.md', buildGuide],
      ['docs/user_guide.md', userGuide],
    ] as const) {
      assert.match(text, /WebView/)
      assert.match(text, /build\/<platform>\/webview\/package\/ECNU VPN|build\\windows\\webview\\package\\ECNU VPN|build\/macos\/webview\/package\/ECNU VPN/)
      assert.doesNotMatch(text, /build\/windows\/electron|build\\windows\\electron|build\/macos\/electron|build\\macos\\electron/)
      assert.doesNotMatch(text, /Desktop UI \(Electron\)|Electron-based desktop app is the recommended interface|Electron 桌面应用|Electron 桌面端|through Electron IPC/)
      assert.doesNotMatch(text, /pnpm run desktop:package|pnpm run desktop:build|pnpm run build:electron|pnpm run desktop:dev/)
    }

    assert.equal(existsSync(join(repoRoot, 'docs', 'windows-electron-helper-recovery.md')), false)
    assert.equal(
      existsSync(join(repoRoot, 'docs', 'archive', '2026-06', 'windows-electron-helper-recovery.md')),
      true,
    )
  })
})
