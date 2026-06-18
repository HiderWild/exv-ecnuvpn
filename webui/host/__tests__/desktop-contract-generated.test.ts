import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import {
  CONFIG_ACTIONS,
  CONFIG_ALIASES,
  CORE_RPC_ACTIONS,
  CORE_RPC_REQUEST_FIELDS,
  CORE_RPC_RESPONSE_FIELDS,
  DESTRUCTIVE_CORE_RPC_ACTIONS,
  DESKTOP_RPC_ACTIONS,
  DESKTOP_RPC_ERROR_CODES,
  DESKTOP_RPC_ERROR_CODE_MAP,
  DESKTOP_RPC_EVENT_TYPES,
  DESKTOP_RPC_REQUEST_FIELDS,
  DESKTOP_RPC_RESPONSE_FIELDS,
  HELPER_OP_CONTRACTS,
  HELPER_FORBIDDEN_CREDENTIAL_FIELDS,
  HELPER_OPS,
  IPC_PROTOCOL_MAJOR,
  STANDARD_ERROR_CODES,
} from '../../desktop/shared/generated/system-contract.js'
import {
  configAliases,
  coreRpcActions,
  desktopRpcActions,
  desktopRpcErrorCodes,
  desktopEventTypes,
  destructiveCoreRpcActions,
  ipcProtocolMajor,
  standardErrorCodes,
} from '../../desktop/shared/desktop-contract.js'

function expectContains<T>(values: readonly T[], value: T) {
  assert.ok(values.includes(value), `expected ${String(value)} in ${values.join(', ')}`)
}

function readRepoJson(pathFromRoot: string): Record<string, any> {
  return JSON.parse(readFileSync(resolve(process.cwd(), '..', pathFromRoot), 'utf8'))
}

function manifest() {
  return readRepoJson('contracts/system.contract.json')
}

function snapshot() {
  return readRepoJson('contracts/generated/system_contract_snapshot.json')
}

describe('generated system contract', () => {
  it('keeps generated snapshot identical to the manifest', () => {
    assert.deepEqual(snapshot(), manifest())
  })

  it('generates desktop RPC action constants from the manifest', () => {
    assert.deepEqual(DESKTOP_RPC_ACTIONS, manifest().surfaces.desktop_rpc.actions)
    assert.deepEqual(desktopRpcActions, DESKTOP_RPC_ACTIONS)
    assert.ok(DESKTOP_RPC_ACTIONS.includes('vpn.authInteraction.get'))
    assert.ok(DESKTOP_RPC_ACTIONS.includes('vpn.authInteraction.respond'))
  })

  it('keeps persisted log mutation out of the desktop RPC surface', () => {
    const desktopActions: readonly string[] = DESKTOP_RPC_ACTIONS

    assert.ok(desktopActions.includes('logs.list'))
    assert.ok(!desktopActions.includes('logs.clear'))
    assert.deepEqual(manifest().modules.logs.actions, ['logs.list'])
  })

  it('captures desktop and core RPC envelopes', () => {
    assert.deepEqual(DESKTOP_RPC_REQUEST_FIELDS, ['id', 'action', 'payload'])
    expectContains(DESKTOP_RPC_RESPONSE_FIELDS, 'ok')
    expectContains(DESKTOP_RPC_RESPONSE_FIELDS, 'data')
    expectContains(DESKTOP_RPC_RESPONSE_FIELDS, 'code')
    expectContains(DESKTOP_RPC_RESPONSE_FIELDS, 'message')
    expectContains(DESKTOP_RPC_RESPONSE_FIELDS, 'event')

    assert.deepEqual(CORE_RPC_REQUEST_FIELDS, ['action', 'payload_json', 'request_id'])
    assert.deepEqual(CORE_RPC_RESPONSE_FIELDS, [
      'success',
      'payload_json',
      'error_code',
      'error_message',
      'request_id',
    ])

    assert(CORE_RPC_ACTIONS.includes('core.hello'))
    assert(CORE_RPC_ACTIONS.includes('config.import'))
    assert(CORE_RPC_ACTIONS.includes('maintenance.inspectCore'))
    assert(DESTRUCTIVE_CORE_RPC_ACTIONS.includes('key.reset'))
    assert(STANDARD_ERROR_CODES.includes('config_import_tampered_or_wrong_password'))
    assert.equal(IPC_PROTOCOL_MAJOR, 1)
  })

  it('exposes new core RPC actions for config import/export/reset, key reset, and maintenance', () => {
    expectContains(CORE_RPC_ACTIONS, 'config.import')
    expectContains(CORE_RPC_ACTIONS, 'config.export')
    expectContains(CORE_RPC_ACTIONS, 'config.reset')
    expectContains(CORE_RPC_ACTIONS, 'key.status')
    expectContains(CORE_RPC_ACTIONS, 'key.reset')
    expectContains(CORE_RPC_ACTIONS, 'maintenance.inspectCore')
    expectContains(CORE_RPC_ACTIONS, 'maintenance.killStaleCore')

    expectContains(DESTRUCTIVE_CORE_RPC_ACTIONS, 'config.reset')
    expectContains(DESTRUCTIVE_CORE_RPC_ACTIONS, 'key.reset')
    expectContains(DESTRUCTIVE_CORE_RPC_ACTIONS, 'maintenance.killStaleCore')
  })

  it('declares config canonical actions and legacy aliases', () => {
    const config = manifest().modules.config
    assert.deepEqual(CONFIG_ACTIONS, config.actions.map((action: { name: string }) => action.name))
    assert.deepEqual(
      CONFIG_ALIASES,
      Object.fromEntries(
        config.aliases.map((alias: { alias: string; target: string }) => [alias.alias, alias.target]),
      ),
    )

    expectContains(CONFIG_ACTIONS, 'config.getAuth')
    expectContains(CONFIG_ACTIONS, 'config.saveSettings')
    expectContains(CONFIG_ACTIONS, 'config.profile.get')
    assert.equal(CONFIG_ALIASES['config.get'], 'config.getSettings')
    assert.equal(CONFIG_ALIASES['config.save'], 'config.saveSettings')
    assert.equal(CONFIG_ALIASES['config.get_profile'], 'config.profile.get')
    assert.equal(CONFIG_ALIASES['config.save_profile'], 'config.profile.save')
  })

  it('re-exports generated core contract metadata through the desktop shared contract', () => {
    assert.deepEqual(coreRpcActions, CORE_RPC_ACTIONS)
    assert.deepEqual(destructiveCoreRpcActions, DESTRUCTIVE_CORE_RPC_ACTIONS)
    assert.deepEqual(configAliases, CONFIG_ALIASES)
    assert.deepEqual(standardErrorCodes, STANDARD_ERROR_CODES)
    assert.equal(ipcProtocolMajor, IPC_PROTOCOL_MAJOR)
  })

  it('removes duplicate legacy action maps from host-contract.ts once generated constants exist', () => {
    const source = readFileSync(resolve(process.cwd(), 'host/shared/host-contract.ts'), 'utf8')

    assert(!source.includes("export const CONFIG_ACTIONS = {"))
    assert(!source.includes("export const ROUTE_ACTIONS = {"))
  })

  it('keeps helper privileged contract credential-free', () => {
    const helper = manifest().modules.helper
    assert.deepEqual(HELPER_OPS, helper.ops.map((op: { name: string }) => op.name))
    assert.deepEqual(HELPER_OP_CONTRACTS, helper.ops)
    assert.deepEqual(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, helper.security.forbidden_fields)

    expectContains(HELPER_OPS, 'Hello')
    expectContains(HELPER_OPS, 'StartSession')
    expectContains(HELPER_OPS, 'ApplyTunnelConfig')
    expectContains(HELPER_OPS, 'Cleanup')
    expectContains(HELPER_OPS, 'Shutdown')
    expectContains(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, 'password')
    expectContains(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, 'cookie')
    expectContains(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, 'auth_token')
  })

  it('keeps desktop event and error constants aligned with the public desktop contract', () => {
    const desktop = manifest().surfaces.desktop_rpc
    const errorCodes = desktop.error_codes.map((entry: { code: string }) => entry.code)
    const errorCodeMap = Object.fromEntries(
      desktop.error_codes.map((entry: { key: string; code: string }) => [entry.key, entry.code]),
    )

    assert.deepEqual(DESKTOP_RPC_EVENT_TYPES, desktop.event_types)
    assert.deepEqual(DESKTOP_RPC_ERROR_CODES, errorCodes)
    assert.deepEqual(DESKTOP_RPC_ERROR_CODE_MAP, errorCodeMap)
    assert.deepEqual(desktopEventTypes, DESKTOP_RPC_EVENT_TYPES)
    assert.deepEqual(desktopRpcErrorCodes, DESKTOP_RPC_ERROR_CODE_MAP)
    assert.deepEqual(
      [...DESKTOP_RPC_ERROR_CODES].sort(),
      Object.values(desktopRpcErrorCodes).sort(),
    )
  })
})
