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

    assert.match(windows, /ECNUVPN_WEBUI_DIST_DIR = Join-Path \$buildRoot 'webview\\dist'/)
    assert.match(windows, /function Invoke-WebViewPackage/)
    const windowsDesktop = powershellCase(windows, 'desktop')
    const windowsAll = powershellCase(windows, 'all')
    assert.match(windowsDesktop, /Invoke-WebViewPackage/)
    assert.match(windowsAll, /Invoke-WebViewPackage/)
    assert.doesNotMatch(windowsDesktop, /Invoke-ElectronCompile|Invoke-DesktopPackage/)
    assert.doesNotMatch(windowsAll, /Invoke-ElectronCompile|Invoke-DesktopPackage/)

    assert.match(macos, /ECNUVPN_WEBUI_DIST_DIR="\$BUILD_ROOT\/webview\/dist"/)
    assert.match(macos, /package_webview\(\)/)
    const macosDesktop = bashCase(macos, 'desktop')
    const macosAll = bashCase(macos, 'all')
    assert.match(macosDesktop, /package_webview/)
    assert.match(macosAll, /package_webview/)
    assert.doesNotMatch(macosDesktop, /compile_electron|package_desktop/)
    assert.doesNotMatch(macosAll, /compile_electron|package_desktop/)
  })
})
