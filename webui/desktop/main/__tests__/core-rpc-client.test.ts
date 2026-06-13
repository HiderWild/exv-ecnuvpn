/**
 * E3: CoreRpcClient test suite
 *
 * Tests the JSON-RPC communication between Electron and the core process.
 * Uses mock child processes (PassThrough streams) to simulate stdin/stdout
 * without requiring the real core binary.
 *
 * Run:  npx tsx desktop/main/__tests__/core-rpc-client.test.ts
 */

import { describe, it, beforeEach, afterEach } from 'node:test'
import assert from 'node:assert/strict'
import { PassThrough } from 'node:stream'
import type { ChildProcess } from 'node:child_process'

import { CoreRpcClient } from '../core-rpc-client.js'
import { normalizeRpcSuccessResult } from '../rpc-result.js'

// Mock child process factory

interface MockProcess {
  child: ChildProcess
  stdin: PassThrough
  stdout: PassThrough
  stderr: PassThrough
  cleanup: () => void
}

function createMockProcess(): MockProcess {
  const stdin = new PassThrough()
  const stdout = new PassThrough()
  const stderr = new PassThrough()

  const child = {
    stdin,
    stdout,
    stderr,
    exitCode: null as number | null,
    killed: false,
    kill(_signal?: string) {
      stdout.end()
      stderr.end()
      return true
    },
    on(_event: string, _cb: (...args: unknown[]) => void) {
      return child
    },
  } as unknown as ChildProcess

  return {
    child,
    stdin,
    stdout,
    stderr,
    cleanup() {
      stdout.end()
      stderr.end()
    },
  }
}

function sendResponse(stdout: PassThrough, response: Record<string, unknown>) {
  stdout.write(JSON.stringify(response) + String.fromCharCode(10))
}

function readLine(stream: PassThrough, timeoutMs = 2000): Promise<string> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('readLine timed out')), timeoutMs)
    const onData = (chunk: Buffer) => {
      clearTimeout(timer)
      stream.removeListener('data', onData)
      resolve(chunk.toString().trim())
    }
    stream.on('data', onData)
  })
}
describe('RPC result normalization', () => {
  it('unwraps future JSON-RPC data envelopes for fallback parity', () => {
    const result = normalizeRpcSuccessResult({ ok: true, data: { connected: true, phase: 'connected' } })
    assert.deepEqual(result, { connected: true, phase: 'connected' })
  })

  it('strips only the transport ok marker from legacy success objects', () => {
    const result = normalizeRpcSuccessResult({ ok: true, connected: false, phase: 'idle' })
    assert.deepEqual(result, { connected: false, phase: 'idle' })
  })

  it('preserves non-envelope payloads unchanged', () => {
    const result = normalizeRpcSuccessResult([{ message: 'hello' }])
    assert.deepEqual(result, [{ message: 'hello' }])
  })
})

// Tests

describe('CoreRpcClient', () => {
  let mock: MockProcess
  let client: CoreRpcClient

  beforeEach(() => {
    mock = createMockProcess()
    client = new CoreRpcClient(mock.child)
  })

  afterEach(() => {
    client.close()
    mock.cleanup()
  })

  describe('request/response matching', () => {
    it('sends correct JSON-RPC message and resolves with the matching response', async () => {
      const outbound = readLine(mock.stdin)
      const responsePromise = client.request('status.get', {})
      const raw = await outbound
      const request = JSON.parse(raw)

      assert.equal(typeof request.id, 'number')
      assert.equal(request.action, 'status.get')
      assert.deepEqual(request.payload, {})

      sendResponse(mock.stdout, { id: request.id, ok: true, phase: 'idle', connected: false })
      const result = await responsePromise
      assert.deepEqual(result, { id: request.id, ok: true, phase: 'idle', connected: false })
    })

    it('assigns incrementing ids to sequential requests', async () => {
      const req1Line = readLine(mock.stdin)
      const p1 = client.request('status.get', {})
      const r1 = JSON.parse(await req1Line)
      sendResponse(mock.stdout, { id: r1.id, ok: true })
      await p1

      const req2Line = readLine(mock.stdin)
      const p2 = client.request('status.get', {})
      const r2 = JSON.parse(await req2Line)
      sendResponse(mock.stdout, { id: r2.id, ok: true })
      await p2

      assert.equal(r2.id, r1.id + 1, 'ids should increment')
    })

    it('resolves multiple in-flight requests independently', async () => {
      // Send first request and read its outbound line.
      const p1 = client.request('action.a', { x: 1 })
      const raw1 = await readLine(mock.stdin)
      const r1 = JSON.parse(raw1)

      // Send second request and read its outbound line.
      const p2 = client.request('action.b', { y: 2 })
      const raw2 = await readLine(mock.stdin)
      const r2 = JSON.parse(raw2)

      // Respond out of order: respond to r2 first, then r1.
      sendResponse(mock.stdout, { id: r2.id, ok: true, data: 'b-done' })
      sendResponse(mock.stdout, { id: r1.id, ok: true, data: 'a-done' })

      const [res1, res2] = await Promise.all([p1, p2])
      assert.equal(res1, 'a-done')
      assert.equal(res2, 'b-done')
    })
  })

  describe('event reception', () => {
    it('dispatches event messages to registered listeners', async () => {
      const events: Array<{ event: string; data: unknown }> = []
      client.onEvent((event, data) => events.push({ event, data }))

      sendResponse(mock.stdout, { event: 'status', data: { connected: true } })
      sendResponse(mock.stdout, { event: 'heartbeat', data: {} })

      await new Promise((r) => setTimeout(r, 50))

      assert.equal(events.length, 2)
      assert.equal(events[0].event, 'status')
      assert.deepEqual(events[0].data, { connected: true })
      assert.equal(events[1].event, 'heartbeat')
      assert.deepEqual(events[1].data, {})
    })

    it('does not treat event messages as responses', async () => {
      const events: Array<{ event: string; data: unknown }> = []
      client.onEvent((event, data) => events.push({ event, data }))

      const line = readLine(mock.stdin)
      const p = client.request('status.get', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, { event: 'log', data: { msg: 'hello' } })
      sendResponse(mock.stdout, { id: req.id, ok: true })

      const result = await p
      assert.equal(events.length, 1)
      assert.equal(events[0].event, 'log')
      assert.ok(result)
    })
  })
  describe('error responses', () => {
    it('rejects when response has ok=false with error code', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('bad.action', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, {
        id: req.id,
        ok: false,
        code: 'auth_failed',
        message: 'Invalid credentials',
      })

      try {
        await p
        assert.fail('Expected rejection')
      } catch (err) {
        assert.ok(err instanceof Error)
        assert.match(err.message, /auth_failed: Invalid credentials/)
        assert.equal((err as Error & { code?: string }).code, 'auth_failed')
      }
    })

    it('rejects with generic message when ok=false has no code', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('fail.action', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, { id: req.id, ok: false, message: 'Something went wrong' })

      try {
        await p
        assert.fail('Expected rejection')
      } catch (err) {
        assert.ok(err instanceof Error)
        assert.match(err.message, /Something went wrong/)
      }
    })
  })

  describe('close()', () => {
    it('rejects all pending requests on close', async () => {
      const line1 = readLine(mock.stdin)
      const line2 = readLine(mock.stdin)

      const p1 = client.request('action.1', {})
      const p2 = client.request('action.2', {})

      await line1
      await line2

      client.close()

      await assert.rejects(p1, (err: Error) => {
        assert.match(err.message, /CoreRpcClient closed/)
        return true
      })
      await assert.rejects(p2, (err: Error) => {
        assert.match(err.message, /CoreRpcClient closed/)
        return true
      })
    })

    it('rejects new requests after close', async () => {
      client.close()

      await assert.rejects(
        () => client.request('after.close', {}),
        (err: Error) => {
          assert.match(err.message, /CoreRpcClient is closed/)
          return true
        },
      )
    })
  })

  describe('isAlive()', () => {
    it('returns true when process is running', () => {
      assert.equal(client.isAlive(), true)
    })

    it('returns false after close()', () => {
      client.close()
      assert.equal(client.isAlive(), false)
    })
  })

  describe('waitForReady()', () => {
    it('resolves when the process responds to a readiness probe', async () => {
      const probeLine = readLine(mock.stdin)
      const readyPromise = client.waitForReady(3000)

      const req = JSON.parse(await probeLine)
      sendResponse(mock.stdout, { id: req.id, ok: true, phase: 'idle' })

      await readyPromise
    })

    it('rejects when the process does not respond within timeout', async () => {
      const freshMock = createMockProcess()
      const freshClient = new CoreRpcClient(freshMock.child)

      const probeLine = readLine(freshMock.stdin)
      const readyPromise = freshClient.waitForReady(200)

      await probeLine

      try {
        await readyPromise
        assert.fail('Expected rejection')
      } catch (err) {
        assert.ok(err instanceof Error)
        assert.ok(
          err.message.includes('timed out') || err.message.includes('not ready'),
          'Unexpected error: ' + err.message,
        )
      } finally {
        freshClient.close()
        freshMock.cleanup()
      }
    })
  })

  describe('payload contract', () => {
    it('resolves to inner data object when response has data property', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('vpn.connect', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, {
        id: req.id,
        ok: true,
        data: { connected: true, phase: 'connected' },
      })

      const result = await p
      assert.deepEqual(result, { connected: true, phase: 'connected' })
    })

    it('event messages with event property still go only to listeners, not response', async () => {
      const events: Array<{ event: string; data: unknown }> = []
      client.onEvent((event, data) => events.push({ event, data }))

      const line = readLine(mock.stdin)
      const p = client.request('status.get', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, { event: 'log', data: { msg: 'connecting' } })
      sendResponse(mock.stdout, {
        id: req.id,
        ok: true,
        data: { connected: false, phase: 'idle' },
      })

      const result = await p
      assert.deepEqual(result, { connected: false, phase: 'idle' })
      assert.equal(events.length, 1)
      assert.equal(events[0].event, 'log')
    })

    it('rejects error responses with code/message, not data', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('vpn.connect', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, {
        id: req.id,
        ok: false,
        code: 'no_config',
        message: 'VPN config not found',
      })

      try {
        await p
        assert.fail('Expected rejection')
      } catch (err) {
        assert.ok(err instanceof Error)
        assert.match(err.message, /no_config: VPN config not found/)
        assert.equal((err as Error & { code?: string }).code, 'no_config')
      }
    })
  })

  describe('edge cases', () => {
    it('ignores empty lines from stdout', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('edge.test', {})
      const req = JSON.parse(await line)

      mock.stdout.write(String.fromCharCode(10))
      mock.stdout.write('  ' + String.fromCharCode(10))
      mock.stdout.write(String.fromCharCode(10))
      sendResponse(mock.stdout, { id: req.id, ok: true })

      const result = await p
      assert.ok(result)
    })

    it('ignores non-JSON lines from stdout', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('edge.test2', {})
      const req = JSON.parse(await line)

      mock.stdout.write('not json at all' + String.fromCharCode(10))
      mock.stdout.write('another garbage line' + String.fromCharCode(10))
      sendResponse(mock.stdout, { id: req.id, ok: true })

      const result = await p
      assert.ok(result)
    })

    it('handles responses with unknown ids gracefully', async () => {
      const line = readLine(mock.stdin)
      const p = client.request('known.action', {})
      const req = JSON.parse(await line)

      sendResponse(mock.stdout, { id: 99999, ok: true })
      sendResponse(mock.stdout, { id: req.id, ok: true })

      const result = await p
      assert.ok(result)
    })
  })
})
