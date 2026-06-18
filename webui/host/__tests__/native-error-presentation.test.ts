import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()
const storeText = readFileSync(join(webuiRoot, 'src', 'stores', 'vpn.ts'), 'utf8')
const globalTypeText = readFileSync(join(webuiRoot, 'src', 'types', 'ecnu-vpn.d.ts'), 'utf8')

const requiredNativeErrorCodes = [
  'auth_protocol_mismatch',
  'auth_rejected',
  'auth_challenge_required',
  'auth_group_required',
  'auth_expired',
  'csd_required_unsupported',
  'dtls_unavailable',
  'session_timeout',
  'idle_timeout',
  'unsupported_extra_args',
] as const

describe('native error presentation contract', () => {
  it('maps every native connection error code to a renderer descriptor', () => {
    for (const code of requiredNativeErrorCodes) {
      assert.match(storeText, new RegExp(`\\b${code}:\\s*\\{`), `${code} missing from contractErrorMap`)
      assert.match(storeText, new RegExp(`error_type:\\s*'${code}'`), `${code} missing as a typed presentation`)
    }
  })

  it('keeps the desktop global VPN error type in sync', () => {
    for (const code of requiredNativeErrorCodes) {
      assert.match(globalTypeText, new RegExp(`'${code}'`), `${code} missing from EcnuVpnApi VpnErrorType`)
    }
  })
})
