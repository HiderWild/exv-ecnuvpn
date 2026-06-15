import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

function readWebui(pathFromWebuiRoot: string) {
  return readFileSync(resolve(process.cwd(), pathFromWebuiRoot), 'utf8')
}

function bodyOfFunction(source: string, name: string) {
  const marker = `function ${name}`
  const markerIndex = source.indexOf(marker)
  assert.notEqual(markerIndex, -1, `missing ${marker}`)

  const paramsOpen = source.indexOf('(', markerIndex)
  assert.notEqual(paramsOpen, -1, `missing parameter list for ${marker}`)

  let paramsDepth = 0
  let paramsClose = -1
  for (let i = paramsOpen; i < source.length; i++) {
    const ch = source[i]
    if (ch === '(') paramsDepth++
    if (ch === ')') {
      paramsDepth--
      if (paramsDepth === 0) {
        paramsClose = i
        break
      }
    }
  }
  assert.notEqual(paramsClose, -1, `unterminated parameter list for ${marker}`)

  const openBrace = source.indexOf('{', paramsClose)
  assert.notEqual(openBrace, -1, `missing body for ${marker}`)

  let depth = 0
  for (let i = openBrace; i < source.length; i++) {
    const ch = source[i]
    if (ch === '{') depth++
    if (ch === '}') {
      depth--
      if (depth === 0) {
        return source.slice(openBrace + 1, i)
      }
    }
  }

  assert.fail(`unterminated body for ${marker}`)
}

function bodyOfObjectProperty(source: string, property: string) {
  const markerIndex = source.indexOf(`${property}: {`)
  assert.notEqual(markerIndex, -1, `missing object property ${property}`)

  const openBrace = source.indexOf('{', markerIndex)
  assert.notEqual(openBrace, -1, `missing object body for ${property}`)

  let depth = 0
  for (let i = openBrace; i < source.length; i++) {
    const ch = source[i]
    if (ch === '{') depth++
    if (ch === '}') {
      depth--
      if (depth === 0) {
        return source.slice(openBrace + 1, i)
      }
    }
  }

  assert.fail(`unterminated object body for ${property}`)
}

describe('desktop service routing policy', () => {
  it('does not expose the legacy elevated serviceCommand fallback', () => {
    const main = readWebui('desktop/main/index.ts')
    const contract = readWebui('desktop/shared/desktop-contract.ts')
    const platformBase = readWebui('desktop/main/platform/base.ts')
    const platformWin32 = readWebui('desktop/main/platform/win32.ts')
    const platformDarwin = readWebui('desktop/main/platform/darwin.ts')
    const platformLinux = readWebui('desktop/main/platform/linux.ts')

    assert.doesNotMatch(contract, /serviceCommand|DesktopServiceCommand|desktopServiceCommands/)
    assert.doesNotMatch(main, /serviceCommand|runServiceCommandElevated|waitForServiceCommandStatus/)
    assert.doesNotMatch(platformBase, /runServiceCommandElevated|DesktopServiceCommand|emitServiceProgress/)
    assert.doesNotMatch(platformWin32, /runServiceCommandElevated|DesktopServiceCommand|service\s+\$\{command\}/)
    assert.doesNotMatch(platformDarwin, /runServiceCommandElevated|DesktopServiceCommand|service\s+\$\{command\}/)
    assert.doesNotMatch(platformLinux, /runServiceCommandElevated|DesktopServiceCommand|\['service',\s*command\]/)
  })

  it('routes service install and uninstall through core RPC actions first', () => {
    const preload = readWebui('desktop/preload/index.ts')
    const serviceApi = bodyOfObjectProperty(preload, 'service')

    assert.match(preload, /SERVICE_ACTIONS/)
    assert.match(serviceApi, /install:\s*\(\)\s*=>\s*rpc\(SERVICE_ACTIONS\.INSTALL\)/)
    assert.match(serviceApi, /uninstall:\s*\(\)\s*=>\s*rpc\(SERVICE_ACTIONS\.UNINSTALL\)/)
    assert.doesNotMatch(serviceApi, /serviceCommand/)
  })

  it('allows service install during an active VPN session but rejects uninstall without disconnecting', () => {
    const store = readWebui('src/stores/vpn.ts')
    const installService = bodyOfFunction(store, 'installService')
    const uninstallService = bodyOfFunction(store, 'uninstallService')

    assert.doesNotMatch(installService, /ensureDisconnectedForServiceChange|disconnectFirst|disconnectElevated|await disconnect\(/)
    assert.match(uninstallService, /rejectActiveVpnForServiceUninstall/)
    assert.doesNotMatch(uninstallService, /disconnectFirst|disconnectElevated|await disconnect\(/)
  })

  it('keeps service management pages aligned with active VPN policy', () => {
    const servicePage = readWebui('src/pages/ServicePage.vue')
    const servicePageInstall = bodyOfFunction(servicePage, 'install')
    const servicePageUninstall = bodyOfFunction(servicePage, 'uninstall')

    assert.match(servicePageInstall, /runServiceAction\('install'\)/)
    assert.doesNotMatch(servicePageInstall, /disconnectFirst|runServiceAction\('install',\s*true\)/)
    assert.match(servicePageUninstall, /requestError/)
    assert.doesNotMatch(servicePageUninstall, /disconnectFirst|runServiceAction\('uninstall',\s*true\)/)

    const settingsPage = readWebui('src/pages/SettingsPage.vue')
    const toggleService = bodyOfFunction(settingsPage, 'toggleService')

    assert.match(toggleService, /vpn\.serviceInstalled/)
    assert.match(toggleService, /requestError/)
    assert.doesNotMatch(toggleService, /disconnectFirst|runServiceToggle\(true\)/)
  })
})
