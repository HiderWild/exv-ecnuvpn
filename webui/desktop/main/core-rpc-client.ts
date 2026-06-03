import type { ChildProcess } from 'node:child_process'
import { createInterface } from 'node:readline'

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
    this.process = process
    this.setupReadline()
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
    if (this.closed) {
      return Promise.reject(new Error('CoreRpcClient is closed'))
    }
    if (!this.process.stdin || this.process.stdin.destroyed) {
      return Promise.reject(new Error('Core process stdin is unavailable'))
    }

    const id = this.nextId++
    const message: RpcRequest = { id, action, payload }

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`RPC request ${action} (id=${id}) timed out after ${CORE_REQUEST_TIMEOUT_MS}ms`))
      }, CORE_REQUEST_TIMEOUT_MS)

      this.pending.set(id, { resolve, reject, timer })

      const line = JSON.stringify(message) + '\n'
      this.process.stdin!.write(line, (err) => {
        if (err) {
          clearTimeout(timer)
          this.pending.delete(id)
          reject(new Error(`Failed to write to core stdin: ${err.message}`))
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
    if (!this.process.stdout) return
    const rl = createInterface({ input: this.process.stdout })
    rl.on('line', (line: string) => {
      this.handleLine(line)
    })

    // Also capture stderr for logging.
    if (this.process.stderr) {
      const errRl = createInterface({ input: this.process.stderr })
      errRl.on('line', (line: string) => {
        if (line.trim()) {
          console.error(`[core:stderr] ${line}`)
        }
      })
    }
  }

  private handleLine(raw: string) {
    const line = raw.trim()
    if (!line) return

    let parsed: RpcResponse
    try {
      parsed = JSON.parse(line) as RpcResponse
    } catch {
      // Non-JSON line – log and ignore.
      console.error(`[CoreRpcClient] non-JSON line: ${line.slice(0, 200)}`)
      return
    }

    // Event push: {"event": "...", "data": {...}}
    if (parsed.event) {
      for (const listener of this.eventListeners) {
        listener(parsed.event, parsed.data)
      }
      return
    }

    // Response: must have an id that matches a pending request.
    if (typeof parsed.id === 'number' && this.pending.has(parsed.id)) {
      const pending = this.pending.get(parsed.id)!
      this.pending.delete(parsed.id)
      clearTimeout(pending.timer)

      if (parsed.ok === false) {
        const code = typeof parsed.code === 'string' ? parsed.code : undefined
        const message = parsed.message || 'Core RPC returned ok=false'
        const error = new Error(code ? `${code}: ${message}` : message) as Error & { code?: string }
        if (code) error.code = code
        pending.reject(error)
      } else {
        pending.resolve(parsed)
      }
      return
    }

    // Unknown message – log and discard.
    console.error(`[CoreRpcClient] unhandled message: ${line.slice(0, 200)}`)
  }
}
