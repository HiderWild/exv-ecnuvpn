import type { ChildProcess } from 'node:child_process'

import { normalizeRpcSuccessResult } from './rpc-result.js'

// ---------------------------------------------------------------------------
// CoreRpcClient – JSON-RPC over stdin/stdout for a single core process
// ---------------------------------------------------------------------------

export interface RpcRequest {
  id: number
  action: string
  payload: unknown
}

export interface RpcResponse {
  id?: number
  ok?: boolean
  event?: string
  data?: unknown
  code?: string
  message?: string
}

export type EventListener = (event: string, data: unknown) => void

export const CORE_REQUEST_TIMEOUT_MS = 30_000

interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (reason: Error) => void
  timer: NodeJS.Timeout
}

export class CoreRpcClient {
  private process: ChildProcess
  private nextId = 1
  private pending = new Map<number, PendingRequest>()
  private eventListeners: EventListener[] = []
  private closed = false
  private ready: Promise<void> | null = null

  constructor(process: ChildProcess) {
    console.log('[CoreRpc] Constructor called, PID:', process.pid)
    this.process = process
    this.setupReadline()
    console.log('[CoreRpc] Setup complete')
  }

  onEvent(listener: EventListener) {
    this.eventListeners.push(listener)
  }

  isAlive(): boolean {
    if (this.closed) return false
    if (!this.process.stdin || this.process.stdin.destroyed) return false
    if (!this.process.stdout || this.process.stdout.destroyed) return false
    return this.process.exitCode === null
  }

  /** Wait until the process is alive and can respond to a ping. */
  waitForReady(timeoutMs: number): Promise<void> {
    if (this.ready) return this.ready
    this.ready = new Promise<void>((resolve, reject) => {
      const deadline = setTimeout(() => {
        reject(new Error('Core process not ready within timeout'))
      }, timeoutMs)
      const attempt = async () => {
        try {
          // Send a lightweight status.get as a readiness probe.
          await this.request('status.get', {})
          clearTimeout(deadline)
          resolve()
        } catch {
          // The process may still be starting up – retry once after a short delay.
          setTimeout(async () => {
            try {
              await this.request('status.get', {})
              clearTimeout(deadline)
              resolve()
            } catch (err) {
              clearTimeout(deadline)
              reject(err instanceof Error ? err : new Error(String(err)))
            }
          }, 500)
        }
      }
      void attempt()
    })
    return this.ready
  }

  /** Send a JSON-RPC request and return a promise for the response data. */
  request(action: string, payload: unknown): Promise<unknown> {
    console.log('[CoreRpc] request() called:', { action, payload: JSON.stringify(payload).slice(0, 100) })

    if (this.closed) {
      console.log('[CoreRpc] REJECTED: client is closed')
      return Promise.reject(new Error('CoreRpcClient is closed'))
    }
    if (!this.process.stdin || this.process.stdin.destroyed) {
      console.log('[CoreRpc] REJECTED: stdin unavailable or destroyed')
      return Promise.reject(new Error('Core process stdin is unavailable'))
    }

    const id = this.nextId++
    const message: RpcRequest = { id, action, payload }

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        console.log(`[CoreRpc] TIMEOUT: request ${action} (id=${id}) after ${CORE_REQUEST_TIMEOUT_MS}ms`)
        this.pending.delete(id)
        reject(new Error(`RPC request ${action} (id=${id}) timed out after ${CORE_REQUEST_TIMEOUT_MS}ms`))
      }, CORE_REQUEST_TIMEOUT_MS)

      this.pending.set(id, { resolve, reject, timer })

      const line = JSON.stringify(message) + '\n'
      console.log('[CoreRpc] Writing to stdin:', line.trim())
      console.log('[CoreRpc] Line length:', line.length, 'bytes')

      this.process.stdin!.write(line, (err) => {
        if (err) {
          console.error('[CoreRpc] Write ERROR:', err)
          clearTimeout(timer)
          this.pending.delete(id)
          reject(new Error(`Failed to write to core stdin: ${err.message}`))
        } else {
          console.log('[CoreRpc] Write SUCCESS for id:', id)
        }
      })
    })
  }

  close() {
    this.closed = true
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer)
      pending.reject(new Error('CoreRpcClient closed'))
    }
    this.pending.clear()
  }

  private setupReadline() {
    console.log('[CoreRpc] setupReadline() called')
    console.log('[CoreRpc] stdout available:', !!this.process.stdout)
    console.log('[CoreRpc] stderr available:', !!this.process.stderr)

    if (!this.process.stdout) {
      console.log('[CoreRpc] WARNING: stdout is not available!')
      return
    }

    // BYPASS readline and handle raw stdout data directly
    // This avoids potential buffering issues with createInterface on Windows
    let stdoutBuffer = ''
    this.process.stdout.on('data', (chunk: Buffer) => {
      console.log('[CoreRpc] STDOUT RAW chunk received:', chunk.length, 'bytes')
      const text = chunk.toString('utf8')
      console.log('[CoreRpc] STDOUT RAW content:', text)

      stdoutBuffer += text
      let newlineIndex
      while ((newlineIndex = stdoutBuffer.indexOf('\n')) !== -1) {
        const line = stdoutBuffer.substring(0, newlineIndex)
        stdoutBuffer = stdoutBuffer.substring(newlineIndex + 1)
        console.log('[CoreRpc] Extracted line:', line)
        this.handleLine(line)
      }
    })

    // Also capture stderr for logging.
    if (this.process.stderr) {
      // Handle stderr the same way - bypass createInterface
      let stderrBuffer = ''
      this.process.stderr.on('data', (chunk: Buffer) => {
        const text = chunk.toString('utf8')
        console.log('[CoreRpc] STDERR RAW:', text)

        stderrBuffer += text
        let newlineIndex
        while ((newlineIndex = stderrBuffer.indexOf('\n')) !== -1) {
          const line = stderrBuffer.substring(0, newlineIndex)
          stderrBuffer = stderrBuffer.substring(newlineIndex + 1)
          if (line.trim()) {
            console.error(`[core:stderr] ${line}`)
          }
        }
      })
    } else {
      console.log('[CoreRpc] WARNING: stderr is not available!')
    }

    // Monitor process lifecycle
    this.process.on('exit', (code, signal) => {
      console.log('[CoreRpc] Process exited:', { code, signal })
    })
    this.process.on('error', (err) => {
      console.error('[CoreRpc] Process error:', err)
    })
    this.process.on('close', (code, signal) => {
      console.log('[CoreRpc] Process closed:', { code, signal })
    })

    console.log('[CoreRpc] All event listeners attached (bypassing readline)')
  }

  private handleLine(raw: string) {
    const line = raw.trim()
    console.log('[CoreRpc] handleLine() called, trimmed length:', line.length)

    if (!line) {
      console.log('[CoreRpc] Ignoring empty line')
      return
    }

    let parsed: RpcResponse
    try {
      parsed = JSON.parse(line) as RpcResponse
      console.log('[CoreRpc] JSON parsed successfully:', parsed)
    } catch (err) {
      // Non-JSON line – log and ignore.
      console.error(`[CoreRpcClient] non-JSON line: ${line.slice(0, 200)}`)
      console.error('[CoreRpc] Parse error:', err)
      return
    }

    // Event push: {"event": "...", "data": {...}}
    if (parsed.event) {
      console.log('[CoreRpc] Handling event:', parsed.event)
      for (const listener of this.eventListeners) {
        listener(parsed.event, parsed.data)
      }
      return
    }

    // Response: must have an id that matches a pending request.
    if (typeof parsed.id === 'number' && this.pending.has(parsed.id)) {
      console.log('[CoreRpc] Handling response for id:', parsed.id)
      const pending = this.pending.get(parsed.id)!
      this.pending.delete(parsed.id)
      clearTimeout(pending.timer)

      if (parsed.ok === false) {
        console.log('[CoreRpc] Response is error:', parsed.code, parsed.message)
        const code = typeof parsed.code === 'string' ? parsed.code : undefined
        const message = parsed.message || 'Core RPC returned ok=false'
        const error = new Error(code ? `${code}: ${message}` : message) as Error & { code?: string }
        if (code) error.code = code
        pending.reject(error)
      } else {
        console.log('[CoreRpc] Response is success, resolving')
        pending.resolve(normalizeRpcSuccessResult(parsed))
      }
      return
    }

    // Unknown message – log and discard.
    console.error(`[CoreRpcClient] unhandled message (no pending request for id=${parsed.id}):`, line.slice(0, 200))
    console.log('[CoreRpc] Current pending requests:', Array.from(this.pending.keys()))
  }
}
