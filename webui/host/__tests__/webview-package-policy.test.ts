import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()
const requireFromWebui = createRequire(join(webuiRoot, 'package.json'))
const { getBuildLayout } = requireFromWebui('./scripts/build-layout.cjs') as {
  getBuildLayout: () => {
    rendererTarget: string
    rendererOutDir: string
    webviewRendererOutDir: string
    electronRendererOutDir: string
  }
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
})
