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

  // INVARIANT: the aggregate-auth raw codes (auth_response_invalid /
  // auth_response_too_large) must not slip past the renderer as auth_failed —
  // a real password is correct in this scenario. The store either rewrites the
  // raw code to auth_protocol_mismatch or maps the raw key to the same
  // view_logs descriptor.
  it('renders aggregate-auth response codes as a protocol mismatch, not a credential failure', () => {
    for (const rawCode of ['auth_response_invalid', 'auth_response_too_large']) {
      const headerIdx = storeText.search(new RegExp(`\\b${rawCode}\\b`))
      assert.notEqual(headerIdx, -1,
        `${rawCode} should be addressed in the store (alias or descriptor)`)

      const window = storeText.slice(Math.max(0, headerIdx - 80), headerIdx + 320)
      assert.match(window, /auth_protocol_mismatch/,
        `${rawCode} should resolve to auth_protocol_mismatch`)
      assert.doesNotMatch(window, /error_type:\s*'auth_failed'/,
        `${rawCode} must not be presented as auth_failed`)
      assert.doesNotMatch(window, /recommended_action:\s*'retry_password'/,
        `${rawCode} must not recommend retry_password`)
    }
  })
})
