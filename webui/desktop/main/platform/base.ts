import type {
  DesktopCliCommand,
  DesktopRpcAction,
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

export type CliInstallStatus = {
  installed: boolean
  installPath: string
  targetPath: string
  availableInPath: boolean
  warning?: string
}

export type DesktopPlatformContext = {
  execFileAsync: (
    file: string,
    args: string[],
    options?: NativeExecOptions,
  ) => Promise<{ stdout: string; stderr: string }>
  resolveExvPath: () => string
  resolveRuntimeDir: (exv?: string) => string | undefined
  resolveStateDir: () => string
  nativeExecOptions: (exv: string, extra?: { maxBuffer?: number }) => NativeExecOptions
  parseJsonOutput: (stdout: string) => unknown
  throwRpcResultError: (result: RpcErrorResult) => never
  runDesktopRpc: (action: DesktopRpcAction, payload?: unknown) => Promise<unknown>
}

export interface DesktopPlatformRunner {
  resolveExvName: () => string
  resolveExvCandidates: (root: string) => string[]
  resolveRuntimeBinaryName: () => string
  resolveRuntimeCandidates: (root: string, resourcesPath: string, isPackaged: boolean, exv: string, runtimeBinaryName: string) => string[]
  shouldQuitOnWindowClose: () => boolean
  runCliCommand: (
    context: DesktopPlatformContext,
    command: DesktopCliCommand,
  ) => Promise<CliInstallStatus>
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
