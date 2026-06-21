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

function cssRule(source: string, selector: string): string {
  const start = source.indexOf(`${selector} {`)
  assert.notEqual(start, -1, `missing CSS rule ${selector}`)
  const end = source.indexOf('\n}', start)
  assert.notEqual(end, -1, `missing CSS rule terminator ${selector}`)
  return source.slice(start, end + 2)
}

function assertUnixDesktopScriptPackagesWebView(script: string, platform: string): void {
  assert.match(script, new RegExp(`EXV_BUILD_PLATFORM=${platform}`))
  assert.match(script, new RegExp(`EXV_WEBUI_DIST_DIR="\\$BUILD_ROOT/webview/dist"`))
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

    assert.match(packageJson.scripts['webview:compile'], /EXV_RENDERER_TARGET=webview/)
    assert.match(packageJson.scripts['webview:package'], /package_ui_shell\.py/)
    assert.match(packageJson.scripts['test:contract'], /run-host-test\.cjs/)
  })

  it('makes platform desktop build scripts package the WebView shell by default', () => {
    const windows = readFileSync(join(repoRoot, 'scripts', 'build-windows.ps1'), 'utf8')
    const macos = readFileSync(join(repoRoot, 'scripts', 'build-macos.sh'), 'utf8')
    const linux = readFileSync(join(repoRoot, 'scripts', 'build-linux.sh'), 'utf8')

    assert.match(windows, /EXV_WEBUI_DIST_DIR = Join-Path \$buildRoot 'webview\\dist'/)
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
    assert.doesNotMatch(packageScript, /--state-dir/)
    assert.match(packageScript, /MINGW_RUNTIME_DLLS/)
    assert.match(packageScript, /wintun\.dll/)
    assert.match(packageScript, /copy_windows_runtime_assets/)
    assert.match(packageScript, /def validate_launch_args_targets\(/)
    assert.match(packageScript, /Launch args target not found/)
    assert.match(packageScript, /PACKAGE_BINARIES\s*=/)
    assert.match(packageScript, /"exv-helper"/)
    assert.match(packageScript, /def validate_required_package_binaries\(/)
    assert.match(packageScript, /Required package executable/)
    assert.match(packageScript, /--verify-launch-targets-only/)
    assert.match(packageScript, /--package-dir/)

    const readme = readFileSync(join(webuiRoot, 'README.md'), 'utf8')
    assert.match(readme, /exv-ui\.args/)
    assert.match(readme, /--renderer-index/)
  })

  it('uses EXV-owned WebView host channels', () => {
    const hostContract = readFileSync(join(webuiRoot, 'host', 'shared', 'host-contract.ts'), 'utf8')
    const oldChannelPattern = new RegExp(`${'ecnu'}-vpn:`)

    assert.match(hostContract, /rpc:\s*'exv:rpc'/)
    assert.match(hostContract, /event:\s*'exv:event'/)
    assert.doesNotMatch(hostContract, oldChannelPattern)
  })

  it('keeps active packaging smoke and merge-prep scripts on WebView package paths', () => {
    const windowsSmoke = readFileSync(join(repoRoot, 'scripts', 'windows-packaging-smoke.ps1'), 'utf8')
    const macosSmoke = readFileSync(join(repoRoot, 'scripts', 'macos-packaging-smoke.sh'), 'utf8')
    const windowsMergePrep = readFileSync(join(repoRoot, 'scripts', 'validate-merge-prep-windows.ps1'), 'utf8')
    const macosMergePrep = readFileSync(join(repoRoot, 'scripts', 'validate-merge-prep-macos.sh'), 'utf8')

    assert.match(windowsSmoke, /build\\windows\\webview\\package\\EXV/)
    assert.match(windowsSmoke, /WebView2Loader\.dll/)
    assert.match(windowsSmoke, /electron\.exe/)
    assert.match(windowsSmoke, /--verify-launch-targets-only/)
    assert.match(windowsSmoke, /--package-dir \$PackageRoot/)
    assert.match(windowsSmoke, /LOCALAPPDATA/)
    assert.match(windowsSmoke, /EXV\\Helper\\exv-helper\.exe/)
    assert.doesNotMatch(windowsSmoke, /windows\\electron|dist-electron/i)

    assert.match(macosSmoke, /build\/macos\/webview\/package\/EXV/)
    assert.match(macosSmoke, /package_ui_shell\.py --verify-launch-targets-only --package-dir "\$PACKAGE_ROOT"/)
    assert.doesNotMatch(macosSmoke, /build\/macos\/electron|Electron \.app|ELECTRON_RELEASE/)

    assert.match(windowsMergePrep, /build-windows\.ps1'.*'desktop'/s)
    assert.doesNotMatch(windowsMergePrep, /build:electron|prepare:native|Electron main\/preload/)
    assert.match(macosMergePrep, /bash "\$SCRIPT_DIR\/build-macos\.sh" desktop/)
    assert.doesNotMatch(macosMergePrep, /build:electron|prepare:native|Electron main\/preload/)
  })

  it('keeps Windows release packaging on native WebView package outputs', () => {
    const releaseScriptPath = join(repoRoot, 'scripts', 'package-windows-release.ps1')
    const nsisPath = join(repoRoot, 'distribution', 'windows', 'exv.nsi')

    assert.equal(existsSync(releaseScriptPath), true, 'Windows release packaging script should exist')
    assert.equal(existsSync(nsisPath), true, 'Windows NSIS installer script should exist')

    const releaseScript = readFileSync(releaseScriptPath, 'utf8')
    const nsis = readFileSync(nsisPath, 'utf8')
    const readme = readFileSync(join(repoRoot, 'README.md'), 'utf8')
    const buildGuide = readFileSync(join(repoRoot, 'docs', 'build_guide.md'), 'utf8')
    const runtimeAssets = readFileSync(join(repoRoot, 'docs', 'runtime-assets.md'), 'utf8')

    assert.match(releaseScript, /\[string\]\$Version/)
    assert.match(releaseScript, /\[switch\]\$SkipBuild/)
    assert.match(releaseScript, /\[string\]\$PackageRoot/)
    assert.match(releaseScript, /\[string\]\$OutputDir/)
    assert.match(releaseScript, /\[string\]\$NsisPath/)
    assert.match(releaseScript, /scripts\\build-windows\.ps1/)
    assert.match(releaseScript, /desktop/)
    assert.match(releaseScript, /package_ui_shell\.py/)
    assert.match(releaseScript, /--verify-launch-targets-only/)
    assert.match(releaseScript, /build\\windows\\webview\\package\\EXV/)
    assert.match(releaseScript, /EXV-\$Version-windows-x64-portable\.zip/)
    assert.match(releaseScript, /EXV-\$Version-windows-x64-setup\.exe/)
    assert.match(releaseScript, /Compress-Archive/)
    assert.match(releaseScript, /Expand-Archive/)
    assert.match(releaseScript, /windows-packaging-smoke\.ps1/)
    assert.match(releaseScript, /makensis\.exe/)
    assert.match(releaseScript, /distribution\\windows\\exv\.nsi/)
    assert.match(releaseScript, /function New-NsisUninstallManifest/)
    assert.match(releaseScript, /UNINSTALL_MANIFEST/)
    assert.match(releaseScript, /Get-ChildItem/)
    assert.match(releaseScript, /Get-RelativePathFromRoot/)
    assert.doesNotMatch(releaseScript, /GetRelativePath/)
    assert.match(releaseScript, /RMDir "\$INSTDIR"/)
    assert.doesNotMatch(releaseScript, /-RuntimeDir \$resolvedPackageRoot/)
    assert.doesNotMatch(releaseScript, /build-windows\\cpp\\exv\.exe|build-windows\\cpp\\exv-helper\.exe|build-windows\\cpp\\exv-ui\.exe/)
    assert.doesNotMatch(releaseScript, /desktop:package|build:electron|electron\\release|dist-electron/i)

    assert.match(nsis, /RequestExecutionLevel user/)
    assert.match(nsis, /\$LOCALAPPDATA\\Programs\\EXV/)
    assert.match(nsis, /SetShellVarContext current/)
    assert.match(nsis, /File \/r "\$\{SOURCE_DIR\}\\\*\.\*"/)
    assert.match(nsis, /CreateShortCut/)
    assert.match(nsis, /WriteUninstaller/)
    assert.match(nsis, /WriteRegStr HKCU/)
    assert.match(nsis, /DeleteRegKey HKCU/)
    assert.match(nsis, /DeleteRegValue HKCU "Software\\EXV" "InstallDir"/)
    assert.match(nsis, /DeleteRegKey \/ifempty HKCU "Software\\EXV"/)
    assert.doesNotMatch(nsis, /DeleteRegKey HKCU "Software\\EXV"/)
    assert.doesNotMatch(nsis, /RequestExecutionLevel admin/)
    assert.doesNotMatch(nsis, /RequestExecutionLevel highest/)
    assert.doesNotMatch(nsis, /SetShellVarContext all/)
    assert.doesNotMatch(nsis, /HKLM/)
    assert.doesNotMatch(nsis, /\$PROGRAMFILES/)
    assert.doesNotMatch(nsis, /RMDir \/r "\$LOCALAPPDATA\\EXV"/)
    assert.match(nsis, /!include "\$\{UNINSTALL_MANIFEST\}"/)
    assert.doesNotMatch(nsis, /RMDir \/r "\$INSTDIR"/)
    assert.doesNotMatch(nsis, /RMDir \/r "\$INSTDIR\\bin"/)
    assert.doesNotMatch(nsis, /RMDir \/r "\$INSTDIR\\webui"/)

    assert.match(readme, /EXV-<version>-windows-x64-portable\.zip/)
    assert.match(readme, /EXV-<version>-windows-x64-setup\.exe/)
    assert.match(buildGuide, /package-windows-release\.ps1 -Version/)
    assert.match(buildGuide, /makensis\.exe/)
    assert.match(buildGuide, /windows-packaging-smoke\.ps1/)
    assert.match(runtimeAssets, /Windows release packaging/)
    assert.match(runtimeAssets, /WebView2 Evergreen Runtime/)
  })

  it('keeps start.ps1 desktop flow on native WebView shell paths', () => {
    const startPs1 = readFileSync(join(repoRoot, 'start.ps1'), 'utf8')
    const startCommon = readFileSync(join(repoRoot, 'scripts', 'start-common.ps1'), 'utf8')
    const startFlow = `${startPs1}\n${startCommon}`

    assert.match(startPs1, /start-common\.ps1/)
    assert.match(startFlow, /build\\windows\\webview\\package\\EXV/)
    assert.match(startFlow, /exv-ui\.exe/)
    assert.match(startFlow, /LOCALAPPDATA/)
    assert.match(startFlow, /EXV\\Helper\\exv-helper\.exe/)
    assert.match(startFlow, /Test-HelperServiceUsesStablePath/)
    assert.match(startFlow, /function Resolve-WebView2Sdk/)
    assert.match(startFlow, /-DWEBVIEW2_SDK_DIR=\$resolvedWebView2Sdk/)
    assert.doesNotMatch(startFlow, /\$alwaysMatchNames = @\('exv\.exe', 'exv-helper\.exe', 'exv-ui\.exe'\)/)
    assert.doesNotMatch(startFlow, /build\\windows\\electron|dist-electron|desktop:package|desktop:package:dir|build:electron/i)
    assert.doesNotMatch(startFlow, /Find-ElectronProcess|Electron process/i)
  })

  it('keeps first-run service install prompts out of the App bootstrap', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const dashboardVue = readFileSync(join(webuiRoot, 'src', 'pages', 'DashboardPage.vue'), 'utf8')

    assert.doesNotMatch(appVue, /service_install_prompt_seen/)
    assert.doesNotMatch(appVue, /serviceInstallPrompt\(/)
    assert.doesNotMatch(appVue, /markServicePromptSeen/)
    assert.match(dashboardVue, /!vpn\.serviceInstalled/)
    assert.match(dashboardVue, /服务已安装但当前不可用/)
  })

  it('keeps window mode resizing out of the settings persistence path', () => {
    const configStore = readFileSync(join(webuiRoot, 'src', 'stores', 'config.ts'), 'utf8')
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const frameVue = readFileSync(join(webuiRoot, 'src', 'components', 'AppWindowFrame.vue'), 'utf8')

    assert.doesNotMatch(configStore, /window\.exv\.window\.setMode/)
    assert.match(configStore, /localStorage\.getItem\('exv:minimal-mode'\)/)
    assert.match(configStore, /localStorage\.setItem\('exv:minimal-mode'/)
    assert.match(configStore, /delete remoteSettings\.minimal_mode/)
    assert.match(configStore, /settings\.value = \{ \.\.\.settings\.value, \.\.\.s \}/)
    assert.doesNotMatch(appVue, /window\.exv\?\.window\?\.setMode/)
    assert.match(frameVue, /watch\(\s*\(\) => props\.mode/)
    assert.match(frameVue, /requestAnimationFrame/)
    assert.match(frameVue, /window\.exv\?\.window\?\.resizeForMode/)
  })

  it('keeps service install prompt seen state in core-owned settings', () => {
    const configStore = readFileSync(join(webuiRoot, 'src', 'stores', 'config.ts'), 'utf8')
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')

    assert.match(configStore, /service_install_prompt_seen: boolean/)
    assert.doesNotMatch(configStore, /localStorage\.getItem\('exv:service-install-prompt-seen'\)/)
    assert.doesNotMatch(configStore, /localStorage\.setItem\('exv:service-install-prompt-seen'/)
    assert.doesNotMatch(configStore, /delete remoteSettings\.service_install_prompt_seen/)
    assert.doesNotMatch(appVue, /servicePromptVisible/)
  })

  it('does not render a renderer-owned service install prompt from App', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const globalStack = readFileSync(join(webuiRoot, 'src', 'windows', 'GlobalWindowStack.vue'), 'utf8')

    assert.doesNotMatch(appVue, /建议您安装辅助服务/)
    assert.doesNotMatch(appVue, /function dismissServicePrompt/)
    assert.doesNotMatch(appVue, /async function installServiceFromPrompt/)
    assert.doesNotMatch(appVue, /<ServiceInstallLoadingOverlay/)
    assert.match(globalStack, /<ServiceInstallLoadingOverlay/)
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
    const types = readFileSync(join(webuiRoot, 'src', 'types', 'exv.d.ts'), 'utf8')
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
    assert.match(types, /resolveClosePrompt\(result: unknown\): Promise<\{ ok: true \}>/)
    assert.match(types, /startDrag\(start\?: DesktopWindowDragStart\): Promise<\{ ok: true \}>/)
    assert.match(types, /shell:\s*\{[\s\S]*openExternal\(url: string\): Promise<\{ ok: true \}>/)
    assert.match(win32Host, /resizeForMode: \(mode, request\) => rpc\('window\.resizeForMode', \{ mode, request \}\)/)
    assert.match(win32Host, /minimize: \(\) => rpc\('window\.minimize'\)/)
    assert.match(win32Host, /requestClose: \(\) => rpc\('window\.requestClose'\)/)
    assert.match(win32Host, /startDrag: \(drag\) => rpc\('window\.startDrag', drag \?\? \{\}\)/)
    assert.match(win32Host, /shell:\s*\{[\s\S]*openExternal: \(url\) => rpc\('shell\.openExternal', \{ url \}\)/)
    assert.match(win32Host, /if \(action == "shell\.openExternal"\)[\s\S]*ShellExecuteW/)
    assert.match(win32Host, /if \(action == "window\.resolveClosePrompt"\)[\s\S]*data\["ok"\] = true/)
    assert.match(darwinHost, /resizeForMode: \(mode, request\) => rpc\('window\.resizeForMode', \{ mode, request \}\)/)
    assert.match(darwinHost, /minimize: \(\) => rpc\('window\.minimize'\)/)
    assert.match(darwinHost, /requestClose: \(\) => rpc\('window\.requestClose'\)/)
    assert.match(darwinHost, /startDrag: \(drag\) => rpc\('window\.startDrag', drag \?\? \{\}\)/)
    assert.match(darwinHost, /shell:\s*\{[\s\S]*openExternal: \(url\) => rpc\('shell\.openExternal', \{ url \}\)/)
    assert.match(darwinHost, /if \(action == "shell\.openExternal"\)[\s\S]*openURL/)
    assert.match(darwinHost, /if \(action == "window\.resolveClosePrompt"\)[\s\S]*data\["ok"\] = true/)
  })

  it('opens About repository links through the native system browser bridge', () => {
    const aboutPage = readFileSync(join(webuiRoot, 'src', 'pages', 'AboutPage.vue'), 'utf8')

    assert.match(aboutPage, /async function openRepository\(\)/)
    assert.match(aboutPage, /window\.exv\?\.shell\?\.openExternal\?\.\(distributionConfig\.repository\.url\)/)
    assert.match(aboutPage, /@click\.prevent="openRepository"/)
    assert.doesNotMatch(aboutPage, /target="_blank"/)
  })

  it('wraps advanced and minimal content in the shared transparent app frame', () => {
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const frameVue = readFileSync(join(webuiRoot, 'src', 'components', 'AppWindowFrame.vue'), 'utf8')
    const navBarVue = readFileSync(join(webuiRoot, 'src', 'components', 'NavBar.vue'), 'utf8')
    const appIconSvg = readFileSync(join(webuiRoot, 'src', 'assets', 'app-icon.svg'), 'utf8')
    const advancedTitlebarRule = cssRule(frameVue, '.app-window-frame--advanced .app-window-titlebar')
    const titlebarRule = cssRule(frameVue, '.app-window-titlebar')
    const titlebarIdentityRule = cssRule(frameVue, '.app-window-titlebar__identity')
    const titlebarTitleRule = cssRule(frameVue, '.app-window-titlebar__title')
    const titlebarControlsRule = cssRule(frameVue, '.app-window-titlebar__controls')
    const titlebarButtonRule = cssRule(frameVue, '.app-window-titlebar__button')

    assert.match(appVue, /import AppWindowFrame from '\.\/components\/AppWindowFrame\.vue'/)
    assert.match(appVue, /<AppWindowFrame[\s\S]*:mode="minimalMode \? 'minimal' : 'advanced'"/)
    assert.match(appVue, /<MinimalModeView v-if="minimalMode" \/>/)
    assert.match(appVue, /<div v-else class="app-advanced-shell/)
    assert.match(appVue, /app-advanced-content-titlebar-spacer/)
    assert.match(appVue, /<main class="min-w-0 flex-1 overflow-hidden pl-44">[\s\S]*app-advanced-content-titlebar-spacer/)
    assert.match(appVue, /h-\[34px\][\s\S]*border-b[\s\S]*border-border/)
    assert.match(frameVue, /<header[\s\S]*class="app-window-titlebar/)
    assert.match(frameVue, /@pointerdown="startWindowDrag"/)
    assert.match(frameVue, /if \(isWindows\.value\) return/)
    assert.match(frameVue, /EXV/)
    assert.match(frameVue, /v-if="visualMode === 'minimal'"/)
    assert.match(frameVue, /:aria-label="titlebarTitle"/)
    assert.match(frameVue, /<span class="app-window-titlebar__title" aria-hidden="true">EXV<\/span>/)
    assert.doesNotMatch(frameVue, /app-window-titlebar__title-line/)
    assert.match(frameVue, /import appIconUrl from '\.\.\/assets\/app-icon\.svg'/)
    assert.match(navBarVue, /import appIconUrl from '\.\.\/assets\/app-icon\.svg'/)
    assert.match(frameVue, /:src="appIconUrl"/)
    assert.match(navBarVue, /:src="appIconUrl"/)
    assert.match(appIconSvg, /<svg[\s\S]*viewBox="0 0 1024 1024"/)
    assert.match(advancedTitlebarRule, /left: 0;/)
    assert.match(advancedTitlebarRule, /right: 0;/)
    assert.match(advancedTitlebarRule, /justify-content: flex-end;/)
    assert.match(advancedTitlebarRule, /padding: 0;/)
    assert.match(titlebarRule, /app-region: drag;/)
    assert.match(titlebarRule, /-webkit-app-region: drag;/)
    assert.match(titlebarControlsRule, /app-region: no-drag;/)
    assert.match(titlebarControlsRule, /-webkit-app-region: no-drag;/)
    assert.match(titlebarButtonRule, /app-region: no-drag;/)
    assert.match(titlebarButtonRule, /-webkit-app-region: no-drag;/)
    assert.match(frameVue, /window\.exv\?\.events\.subscribe\(\(event\) =>/)
    assert.match(frameVue, /event\.type !== 'window-control-state'/)
    assert.match(frameVue, /nativeWindowControlState\.value = \{ control, pressed \}/)
    assert.match(frameVue, /app-window-titlebar__button--native-hover/)
    assert.match(frameVue, /app-window-titlebar__button--native-pressed/)
    assert.doesNotMatch(advancedTitlebarRule, /var\(--advanced-sidebar-width\)/)
    assert.doesNotMatch(advancedTitlebarRule, /border-bottom:/)
    assert.match(titlebarIdentityRule, /display: flex;/)
    assert.match(titlebarIdentityRule, /align-items: center;/)
    assert.doesNotMatch(titlebarIdentityRule, /grid-template-rows:/)
    assert.match(titlebarTitleRule, /white-space: nowrap;/)
    assert.doesNotMatch(titlebarTitleRule, /flex-direction: column;/)
    assert.match(frameVue, /\.app-window-frame--advanced \.app-window-titlebar__identity \{[\s\S]*display: none;/)
    assert.match(frameVue, /\.app-window-frame--advanced \.app-window-content-shell \{[\s\S]*height: 100%;/)
    assert.match(frameVue, /\.app-window-frame--minimal \.app-window-titlebar \{[\s\S]*position: relative;/)
    assert.match(frameVue, /\.app-window-frame--minimal \.app-window-content-shell \{[\s\S]*height: calc\(100% - var\(--titlebar-height\)\);/)
    assert.match(frameVue, /border-radius: var\(--window-radius\)/)
    assert.match(frameVue, /<slot \/>/)
    assert.match(frameVue, /isWindows/)
    assert.match(frameVue, /isMac/)
    assert.match(frameVue, /window\.exv\?\.window\?\.startDrag/)
    assert.match(frameVue, /window\.exv\?\.window\?\.minimize/)
    assert.match(frameVue, /window\.exv\?\.window\?\.requestClose/)
  })

  it('coordinates direction-specific transparent shell mode transitions in the renderer', () => {
    const frameVue = readFileSync(join(webuiRoot, 'src', 'components', 'AppWindowFrame.vue'), 'utf8')
    const appVue = readFileSync(join(webuiRoot, 'src', 'App.vue'), 'utf8')
    const styleCss = readFileSync(join(webuiRoot, 'src', 'style.css'), 'utf8')
    const windowLayout = readFileSync(join(repoRoot, 'src', 'app', 'ui_shell', 'window_layout.hpp'), 'utf8')
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const frameRule = cssRule(frameVue, '.app-window-frame')
    const transparentHostRule = cssRule(frameVue, '.app-window-transparent-host')
    const transitionSurfaceRule = cssRule(frameVue, '.mode-transition-surface')
    const minimalTransitionSurfaceRule = cssRule(frameVue, '.app-window-frame--minimal.app-window-frame--transitioning .mode-transition-surface')
    const advancedTransitionSurfaceRule = cssRule(frameVue, '.app-window-frame--advanced.app-window-frame--transitioning .mode-transition-surface')
    const transitionOverlayRule = cssRule(frameVue, '.mode-transition-overlay')
    const htmlRule = cssRule(styleCss, 'html')
    const bodyRule = cssRule(styleCss, 'body')
    const appRootRule = cssRule(styleCss, '#app')

    assert.match(frameVue, /const MODE_TRANSITION_MS = 300/)
    assert.match(frameVue, /const POST_RESIZE_SETTLE_MS = 50/)
    assert.match(frameVue, /type TransitionPhase = 'idle' \| 'native-resize-before-animation' \| 'preview-animating' \| 'native-resize-after-animation' \| 'settling'/)
    assert.match(frameVue, /async function runModeTransition\(nextMode: WindowMode\)/)
    assert.match(frameVue, /if \(appliedMode\.value === 'minimal' && nextMode === 'advanced'\)/)
    assert.match(frameVue, /transitionPhase\.value = 'native-resize-before-animation'[\s\S]*await resizeNativeWindow\(nextMode, request\)[\s\S]*transitionPhase\.value = 'preview-animating'/)
    assert.match(frameVue, /if \(appliedMode\.value === 'advanced' && nextMode === 'minimal'\)/)
    assert.match(frameVue, /transitionPhase\.value = 'preview-animating'[\s\S]*await waitForPreviewAnimation\(\)[\s\S]*transitionPhase\.value = 'native-resize-after-animation'[\s\S]*await resizeNativeWindow\(nextMode, request\)/)
    assert.match(frameVue, /await wait\(POST_RESIZE_SETTLE_MS\)/)
    assert.match(frameVue, /request !== windowModeRequest/)
    assert.match(frameVue, /mode-transition-overlay/)
    assert.match(frameVue, /backdrop-filter: blur/)
    assert.match(frameVue, /mode-transition-surface/)
    assert.match(frameRule, /--window-radius: 8px;/)
    assert.match(frameRule, /--app-window-shadow-margin: 0px;/)
    assert.match(frameRule, /--app-window-shadow-margin-total: 0px;/)
    assert.match(frameRule, /--app-window-shadow:/)
    assert.match(frameRule, /background: transparent;/)
    assert.match(transparentHostRule, /background: transparent;/)
    assert.match(transparentHostRule, /overflow: hidden;/)
    assert.match(transparentHostRule, /padding: var\(--app-window-shadow-margin\);/)
    assert.match(transparentHostRule, /box-sizing: border-box;/)
    assert.match(transitionSurfaceRule, /background: transparent;/)
    assert.match(transitionSurfaceRule, /overflow: hidden;/)
    assert.match(transitionSurfaceRule, /width: calc\(100vw - var\(--app-window-shadow-margin-total\)\);/)
    assert.match(transitionSurfaceRule, /height: calc\(100vh - var\(--app-window-shadow-margin-total\)\);/)
    assert.match(transitionSurfaceRule, /box-shadow: var\(--app-window-shadow\);/)
    assert.match(transitionSurfaceRule, /border-radius: var\(--window-radius\);/)
    assert.match(minimalTransitionSurfaceRule, /width: var\(--minimal-width\);/)
    assert.match(minimalTransitionSurfaceRule, /height: var\(--minimal-height\);/)
    assert.match(advancedTransitionSurfaceRule, /width: var\(--advanced-width\);/)
    assert.match(advancedTransitionSurfaceRule, /height: var\(--advanced-height\);/)
    assert.match(transitionOverlayRule, /position: absolute;/)
    assert.match(transitionOverlayRule, /background: #0a1223;/)
    assert.doesNotMatch(transitionOverlayRule, /position: fixed;/)
    assert.doesNotMatch(transitionOverlayRule, /background: rgba\(10, 18, 35, 0\.\d+\)/)
    assert.match(htmlRule, /background: transparent;/)
    assert.match(bodyRule, /background: transparent;/)
    assert.match(appRootRule, /background: transparent;/)
    assert.doesNotMatch(bodyRule, /@apply [^;]*bg-bg/)
    assert.doesNotMatch(appVue, /modeTransitionVisible/)
    assert.doesNotMatch(appVue, /applyWindowMode/)
    assert.match(windowLayout, /kWindowShadowMarginPx = 0/)
    assert.match(windowLayout, /kAppSurfaceAdvancedWindowBounds\{972, 563\}/)
    assert.match(windowLayout, /kAppSurfaceMinimalWindowBounds\{302, 118\}/)
    assert.match(windowLayout, /kElectronAdvancedWindowBounds\{[\s\S]*kAppSurfaceAdvancedWindowBounds\.width \+ kWindowShadowMarginPx \* 2/)
    assert.match(windowLayout, /kElectronMinimalWindowBounds\{[\s\S]*kAppSurfaceMinimalWindowBounds\.height \+ kWindowShadowMarginPx \* 2/)
    const hitTestStart = win32Host.indexOf('LRESULT hit_test_custom_frame')
    const trayStart = win32Host.indexOf('void show_tray_menu()', hitTestStart)
    assert.notEqual(hitTestStart, -1)
    assert.notEqual(trayStart, -1)
    const hitTestBlock = win32Host.slice(hitTestStart, trayStart)
    assert.match(hitTestBlock, /kWindowShadowMarginPx/)
    assert.match(hitTestBlock, /content_x = x - shadow_margin/)
    assert.match(hitTestBlock, /content_y = y - shadow_margin/)
    assert.match(hitTestBlock, /content_width = width - shadow_margin \* 2/)
    assert.match(hitTestBlock, /return HTNOWHERE;/)
  })

  it('keeps core RPC work off the Win32 WebView message callback thread', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const runtime = readFileSync(join(repoRoot, 'src', 'app', 'ui_shell', 'ui_shell_runtime.cpp'), 'utf8')
    const rpcClient = readFileSync(join(repoRoot, 'src', 'app', 'ui_shell', 'core_rpc_client.cpp'), 'utf8')
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
    assert.match(runtime, /runtime_config\.pump_core_events = \[&client\]\(\) \{ client\.pump_events\(\); \};/)
    assert.match(rpcClient, /std::try_to_lock/)
    assert.match(rpcClient, /if \(!read_lock\.owns_lock\(\)\) \{[\s\S]*return;/)
    assert.match(runtime, /event_pump_thread\.join\(\)/)
  })

  it('starts the Win32 system titlebar move loop with a validated drag start position', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const startDragStart = win32Host.indexOf('void start_window_drag(')
    assert.notEqual(startDragStart, -1)
    const quitFromTrayStart = win32Host.indexOf('void quit_from_tray()', startDragStart)
    assert.notEqual(quitFromTrayStart, -1)
    const startDragBlock = win32Host.slice(startDragStart, quitFromTrayStart)

    assert.match(startDragBlock, /GetCursorPos\(&cursor\)/)
    assert.match(startDragBlock, /renderer_client_to_screen\(renderer_start\)/)
    assert.match(startDragBlock, /renderer_derived_start\.value_or\(renderer_start_point\.value_or\(cursor\)\)/)
    assert.match(startDragBlock, /renderer_titlebar_hit_test\(renderer_start\)/)
    assert.match(startDragBlock, /left_mouse_button_down\(\)/)
    assert.match(startDragBlock, /start-drag-reject-button-up/)
    assert.match(startDragBlock, /GetCursorPos\(&move_loop_start\)/)
    assert.match(startDragBlock, /start-drag-current-cursor/)
    assert.match(startDragBlock, /ReleaseCapture\(\)/)
    assert.match(startDragBlock, /SendMessageW\(hwnd_, WM_NCLBUTTONDOWN, HTCAPTION/)
    assert.match(startDragBlock, /MAKELPARAM\(move_loop_start\.x, move_loop_start\.y\)/)
    assert.match(win32Host, /case WM_MOUSEACTIVATE:[\s\S]*return MA_ACTIVATE;/)
    assert.doesNotMatch(startDragBlock, /GetMessagePos\(\)/)
    assert.doesNotMatch(startDragBlock, /SetCapture\(hwnd_\)/)
    assert.doesNotMatch(win32Host, /void update_window_drag\(\)/)
  })

  it('enables WebView2 non-client drag regions for the Windows app frame', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )

    assert.match(win32Host, /ICoreWebView2Settings9/)
    assert.match(win32Host, /put_IsNonClientRegionSupportEnabled\(TRUE\)/)
    assert.match(win32Host, /configure_non_client_region_support\(\)/)
  })

  it('routes Windows titlebar control buttons through native hit testing', () => {
    const win32Host = readFileSync(
      join(repoRoot, 'src', 'platform', 'win32', 'ui_shell', 'webview2_host_win32.cpp'),
      'utf8',
    )
    const types = readFileSync(join(webuiRoot, 'src', 'types', 'exv.d.ts'), 'utf8')

    assert.match(win32Host, /control_button_hit_test\(content_x, content_y, content_width, dpi\)/)
    assert.match(win32Host, /return HTMINBUTTON;/)
    assert.match(win32Host, /return HTCLOSE;/)
    assert.match(win32Host, /emit_window_control_state\(/)
    assert.match(win32Host, /event\["type"\] = "window-control-state"/)
    assert.match(win32Host, /case WM_NCLBUTTONDOWN:/)
    assert.match(win32Host, /case WM_NCLBUTTONUP:/)
    assert.match(win32Host, /case WM_NCMOUSELEAVE:/)
    assert.match(types, /export type DesktopWindowControl = 'minimize' \| 'close'/)
    assert.match(types, /export interface WindowControlStateEvent/)
    assert.match(types, /type: DesktopEventType \| 'window-control-state'/)
  })

  it('treats accepted VPN connect jobs as cancellable frontend state', () => {
    const store = readFileSync(join(webuiRoot, 'src', 'stores', 'vpn.ts'), 'utf8')
    const dashboard = readFileSync(join(webuiRoot, 'src', 'pages', 'DashboardPage.vue'), 'utf8')
    const minimal = readFileSync(join(webuiRoot, 'src', 'components', 'MinimalModeView.vue'), 'utf8')
    const types = readFileSync(join(webuiRoot, 'src', 'types', 'exv.d.ts'), 'utf8')
    const sse = readFileSync(join(webuiRoot, 'src', 'composables', 'useSSE.ts'), 'utf8')

    assert.match(store, /interface VpnConnectAccepted/)
    assert.match(store, /function isVpnConnectAccepted/)
    assert.match(store, /function updateStatusFromEvent/)
    assert.match(store, /function startConnectStatusPolling/)
    assert.match(store, /function stopConnectStatusPolling/)
    assert.match(store, /if \(isVpnConnectAccepted\(data\)\)/)
    assert.match(store, /connectInFlight\.value = true[\s\S]*startConnectStatusPolling\(\)[\s\S]*loading\.value = false/)
    assert.match(store, /function isTerminalConnectStatus\(nextStatus: VpnStatus\)/)
    assert.match(store, /if \(connectInFlight\.value && isTerminalConnectStatus\(nextStatus\)\)/)
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
    assert.match(types, /connectElevated\(password\?: string\): Promise<VpnStatus \| VpnConnectAccepted \| VpnError>/)
    assert.match(store, /api\.post<VpnStatus \| VpnConnectAccepted \| VpnError>\(\s*'\/connect\/elevated'/)
    assert.match(store, /async function connectElevated[\s\S]*if \(isVpnConnectAccepted\(data\)\)/)
    assert.match(sse, /store\.updateStatusFromEvent\(event\.data as Partial<VpnStatus>\)/)
  })

  it('normalizes helper service maintenance results before chaining connect', () => {
    const store = readFileSync(join(webuiRoot, 'src', 'stores', 'vpn.ts'), 'utf8')
    const dashboard = readFileSync(join(webuiRoot, 'src', 'pages', 'DashboardPage.vue'), 'utf8')

    assert.match(store, /interface ServiceOperationResult/)
    assert.match(store, /function serviceStatusFromOperationResult\(data: ServiceStatus \| ServiceOperationResult\)/)
    assert.match(store, /const nextStatus = serviceStatusFromOperationResult\(data\)/)
    assert.match(store, /serviceStatus\.value = nextStatus[\s\S]*if \(nextStatus\.warning \|\| !nextStatus\.available\)/)
    assert.doesNotMatch(store, /serviceStatus\.value = data[\s\S]*if \(data\.warning \|\| !data\.available\)/)
    assert.match(store, /const installed = await installService\(\)[\s\S]*await fetchServiceStatus\(\)[\s\S]*await connect\(\)/)
    assert.match(dashboard, /!vpn\.serviceAvailable[\s\S]*!vpn\.serviceInstalled/)
  })

  it('uses the packaged app icon during mode resize masking', () => {
    const frameVue = readFileSync(join(webuiRoot, 'src', 'components', 'AppWindowFrame.vue'), 'utf8')
    const navBarVue = readFileSync(join(webuiRoot, 'src', 'components', 'NavBar.vue'), 'utf8')
    const appIconSvg = readFileSync(join(webuiRoot, 'src', 'assets', 'app-icon.svg'), 'utf8')

    assert.match(frameVue, /import appIconUrl from '\.\.\/assets\/app-icon\.svg'/)
    assert.match(navBarVue, /import appIconUrl from '\.\.\/assets\/app-icon\.svg'/)
    assert.match(frameVue, /<img[\s\S]*class="mode-transition-icon"[\s\S]*:src="appIconUrl"/)
    assert.match(navBarVue, /<img[\s\S]*:src="appIconUrl"[\s\S]*class="h-9 w-9 shrink-0"/)
    assert.match(appIconSvg, /<svg[\s\S]*aria-label="EXV logo"/)
    assert.doesNotMatch(frameVue, /<div class="mode-transition-icon">EXV<\/div>/)
    assert.doesNotMatch(frameVue, /src="\/favicon\.svg"/)
    assert.doesNotMatch(navBarVue, /src="\/favicon\.svg"/)
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
    assert.match(manualPhase7, /Toggle advanced -> minimal 10 times at moderate speed, advanced -> minimal 10 times rapidly, and minimal -> advanced 10 times rapidly/)
    assert.match(manualPhase7, /Transparent Shell Mode Transition Checks/)
    assert.match(manualPhase7, /Windows: advanced -> minimal shows app icon mask immediately/)
    assert.match(manualPhase7, /Windows: advanced -> minimal visually shrinks inside the old physical window, then performs one final physical resize/)
    assert.match(manualPhase7, /Windows: minimal -> advanced performs one physical resize first/)
    assert.match(manualPhase7, /Windows: transparent shell area is not painted as an opaque full-window panel/)
    assert.match(manualPhase7, /Windows: during the transition mask, the area outside the mask but inside the physical window is fully transparent/)
    assert.match(manualPhase7, /Windows: toggle advanced -> minimal 10 times at moderate speed/)
    assert.match(manualPhase7, /Windows: toggle advanced -> minimal 10 times rapidly/)
    assert.match(manualPhase7, /Windows: toggle minimal -> advanced 10 times rapidly/)
    assert.match(manualPhase7, /Windows: click minimize during idle; the window minimizes/)
    assert.match(manualPhase7, /Windows: click close during idle; the existing close-confirmation flow opens/)
    assert.match(manualPhase7, /Windows: close and minimize still work after transitions/)
    assert.match(manualPhase7, /macOS: advanced -> minimal shows app icon mask immediately/)
    assert.match(manualPhase7, /macOS: traffic-light controls remain native and embedded in the app frame/)
    assert.match(manualPhase7, /macOS: the same direction-specific resize timing is visible/)
    assert.match(manualPhase7, /macOS: transparent shell area is not painted as an opaque full-window panel/)
    assert.match(manualPhase7, /macOS: during the transition mask, the area outside the mask but inside the physical window is fully transparent/)
    assert.match(manualPhase7, /macOS: app icon mask remains visible through transition settle/)
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
      assert.match(text, /build\/<platform>\/webview\/package\/EXV|build\\windows\\webview\\package\\EXV|build\/macos\/webview\/package\/EXV/)
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
