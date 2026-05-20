import { existsSync, readFileSync } from 'node:fs'

import type {
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'

export type RpcErrorResult = {
  ok?: boolean
  error?: string
  message?: string
  code?: string
}

export type NativeExecOptions = {
  windowsHide?: boolean
  cwd?: string
  env?: NodeJS.ProcessEnv
  maxBuffer?: number
}

export type DesktopPlatformContext = {
  execFileAsync: (
    file: string,
    args: string[],
    options?: NativeExecOptions,
  ) => Promise<{ stdout: string; stderr: string }>
  resolveExvPath: () => string
  resolveRuntimeDir: (exv?: string) => string | undefined
  nativeExecOptions: (exv: string, extra?: { maxBuffer?: number }) => NativeExecOptions
  parseJsonOutput: (stdout: string) => unknown
  throwRpcResultError: (result: RpcErrorResult) => never
  runDesktopRpc: (action: DesktopRpcAction, payload?: unknown) => Promise<unknown>
  emitServiceProgress: (command: DesktopServiceCommand, line: string) => void
}

export interface DesktopPlatformRunner {
  resolveExvName: () => string
  resolveExvCandidates: (root: string) => string[]
  resolveRuntimeBinaryName: () => string
  resolveRuntimeCandidates: (root: string, resourcesPath: string, isPackaged: boolean, exv: string, runtimeBinaryName: string) => string[]
  shouldQuitOnWindowClose: () => boolean
  runServiceCommandElevated: (
    context: DesktopPlatformContext,
    command: DesktopServiceCommand,
  ) => Promise<void>
  runDesktopRpcElevated: (
    context: DesktopPlatformContext,
    action: DesktopRpcAction,
    payload: unknown,
    followupAction: DesktopRpcAction,
  ) => Promise<unknown>
}

export function shellQuote(value: string) {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

export function psQuote(value: string) {
  return `'${value.replace(/'/g, `''`)}'`
}

export function psArray(values: string[]) {
  return `@(${values.map((value) => psQuote(value)).join(', ')})`
}

export function psRuntimeEnvPrefix(runtimeDir: string | undefined) {
  return runtimeDir ? `$env:ECNUVPN_RUNTIME_DIR = ${psQuote(runtimeDir)}; ` : ''
}

export function readNewLogLines(logPath: string, offset: number) {
  if (!existsSync(logPath)) {
    return { offset, lines: [] as string[] }
  }
  const content = readFileSync(logPath, 'utf8')
  const chunk = content.slice(offset)
  const lines = chunk
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
  return { offset: content.length, lines }
}