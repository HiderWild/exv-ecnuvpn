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
    assert.match(macosMergePrep, /build-macos\.sh"\s+desktop/)
    assert.doesNotMatch(macosMergePrep, /build:electron|prepare:native|Electron main\/preload/)
  })

  it('keeps start.ps1 desktop flow on native WebView shell paths', () => {
    const startPs1 = readFileSync(join(repoRoot, 'start.ps1'), 'utf8')

    assert.match(startPs1, /build\\windows\\webview\\package\\ECNU VPN/)
    assert.match(startPs1, /exv-ui\.exe/)
    assert.doesNotMatch(startPs1, /build\\windows\\electron|dist-electron|desktop:package|desktop:package:dir|build:electron/i)
    assert.doesNotMatch(startPs1, /Find-ElectronProcess|Electron process/i)
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
      existsSync(join(repoRoot, 'docs', 'superpowers', 'archive', 'windows-electron-helper-recovery.md')),
      true,
    )
  })
})
