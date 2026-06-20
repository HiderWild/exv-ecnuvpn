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
    const app = readSource('src', 'App.vue')
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
    assert.match(app, /<ErrorDialog \/>[\s\S]*<\/AppWindowFrame>/)

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
    assert.match(dialog, /ui\.submitCredentialPrompt/)
    assert.match(vpnStore, /const missingUsername = !auth\.username\.trim\(\)/)
    assert.match(vpnStore, /const missingPassword = !\(auth\.remember_password && auth\.password_stored\)/)
    assert.match(vpnStore, /remember_password: credentials\.rememberPassword/)
    assert.match(vpnStore, /password: credentials\.rememberPassword \? credentials\.password/)
  })

  it('quick start supports quick/custom modes, import, skip, and service install default', () => {
    const dialog = readSource('src', 'components', 'QuickStartDialog.vue')
    const app = readSource('src', 'App.vue')

    assert.match(dialog, /mode = ref<'quick' \| 'custom'>\('quick'\)/)
    assert.match(dialog, /vpn-ct\.ecnu\.edu\.cn/)
    assert.match(dialog, /remember_password:\s*true/)
    assert.match(dialog, /installService/)
    assert.match(dialog, /TokenInput/)
    assert.match(dialog, /settingsForm\.mtu/)
    assert.match(dialog, /settingsForm\.dtls/)
    assert.match(dialog, /importConfig/)
    assert.match(dialog, /skip\(\)/)
    assert.match(app, /<QuickStartDialog \/>/)
  })
})
