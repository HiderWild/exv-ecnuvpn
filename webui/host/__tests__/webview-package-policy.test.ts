import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()
const repoRoot = join(webuiRoot, '..')
const requireFromWebui = createRequire(join(webuiRoot, 'package.json'))
const { getBuildLayout } = requireFromWebui('./scripts/build-layout.cjs') as {
  getBuildLayout: () => {
    rendererTarget: string
    rendererOutDir: string
    webviewRendererOutDir: string
    electronRendererOutDir: string
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

  it('keeps Electron renderer output explicit during migration', () => {
    const packageJson = JSON.parse(readFileSync(join(webuiRoot, 'package.json'), 'utf8'))
    assert.match(packageJson.scripts['webview:compile'], /ECNUVPN_RENDERER_TARGET=webview/)
    assert.match(packageJson.scripts['desktop:compile'], /ECNUVPN_RENDERER_TARGET=electron/)
    assert.match(packageJson.scripts['desktop:package'], /ECNUVPN_RENDERER_TARGET=electron/)
    assert.match(packageJson.scripts['desktop:package:dir'], /ECNUVPN_RENDERER_TARGET=electron/)
    assert.match(packageJson.scripts['webview:package'], /package_ui_shell\.py/)
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

    assertUnixDesktopScriptPackagesWebView(macos, 'macos')
    assertUnixDesktopScriptPackagesWebView(linux, 'linux')
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
    assert.match(packageScript, /def validate_launch_args_targets\(/)
    assert.match(packageScript, /Launch args target not found/)
    assert.match(packageScript, /--verify-launch-targets-only/)
    assert.match(packageScript, /--package-dir/)

    const readme = readFileSync(join(webuiRoot, 'README.md'), 'utf8')
    assert.match(readme, /exv-ui\.args/)
    assert.match(readme, /--renderer-index/)
  })
})
