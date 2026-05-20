import { app, BrowserWindow, ipcMain, Menu } from 'electron'
import { execFile } from 'node:child_process'
import { existsSync, readFileSync, unlinkSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import {
  desktopIpcChannels,
  desktopRpcActions,
  type DesktopDriverInstallTarget,
  type DesktopEventType,
  type DesktopRpcAction,
  type DesktopServiceCommand,
} from '../shared/desktop-contract.js'

const execFileAsync = promisify(execFile)
const __dirname = dirname(fileURLToPath(import.meta.url))

const validRpcActions = new Set<DesktopRpcAction>(desktopRpcActions)

type RpcErrorResult = {
  ok?: boolean
  error?: string
  message?: string
  code?: string
}

let mainWindow: BrowserWindow | null = null
let statusTimer: NodeJS.Timeout | null = null
let logTimer: NodeJS.Timeout | null = null
let seenLogCount = 0

function rendererUrl() {
  return process.env.VITE_DEV_SERVER_URL
}

function rendererIndex() {
  return join(__dirname, '..', '..', 'dist', 'index.html')
}

function repoRoot() {
  return resolve(__dirname, '..', '..', '..')
}

function resolveExvPath() {
  if (process.env.EXV_PATH && existsSync(process.env.EXV_PATH)) {
    return resolve(process.env.EXV_PATH)
  }

  const exeName = process.platform === 'win32' ? 'exv.exe' : 'exv'
  const packaged = join(process.resourcesPath, 'bin', exeName)
  if (app.isPackaged && existsSync(packaged)) {
    return packaged
  }

  const root = repoRoot()
  const candidates = process.platform === 'win32'
    ? [
        join(root, 'build', 'exv.exe'),
        join(root, 'build', 'Release', 'exv.exe'),
        join(root, 'build-desktop', 'exv.exe'),
        join(root, 'build-desktop', 'Release', 'exv.exe'),
      ]
    : [
        join(root, 'build', 'exv'),
        join(root, 'build-desktop', 'exv'),
      ]

  const found = candidates.find((candidate) => existsSync(candidate))
  if (found) return found
  return candidates[0]
}

function runtimeBinaryName() {
  return process.platform === 'win32' ? 'openconnect.exe' : 'openconnect'
}

function resolveRuntimeDir(exv = resolveExvPath()) {
  if (process.env.ECNUVPN_RUNTIME_DIR && existsSync(process.env.ECNUVPN_RUNTIME_DIR)) {
    return process.env.ECNUVPN_RUNTIME_DIR
  }

  const root = repoRoot()
  const candidates = app.isPackaged
    ? [join(process.resourcesPath, 'bin')]
    : [
        join(root, 'runtime', `${process.platform}-${process.arch}`),
        join(root, 'runtime', process.platform),
        dirname(exv),
      ]

  return candidates.find((candidate) => existsSync(join(candidate, runtimeBinaryName())))
}

function nativeEnv(exv = resolveExvPath()) {
  const env = { ...process.env }
  const runtimeDir = resolveRuntimeDir(exv)
  if (runtimeDir) {
    env.ECNUVPN_RUNTIME_DIR = runtimeDir
  }
  return env
}

function nativeExecOptions(exv: string, extra: { maxBuffer?: number } = {}) {
  return {
    windowsHide: true,
    cwd: dirname(exv),
    env: nativeEnv(exv),
    ...extra,
  }
}

function parseJsonOutput(stdout: string) {
  const lines = stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
  for (let i = lines.length - 1; i >= 0; --i) {
    try {
      return JSON.parse(lines[i])
    } catch {
      // Keep scanning in case a native dependency wrote diagnostic output.
    }
  }
  throw new Error(`Native command returned non-JSON output: ${stdout.slice(0, 500)}`)
}

function throwRpcResultError(result: RpcErrorResult): never {
  const error = new Error(
    result.error || result.message || 'Native desktop RPC failed',
  ) as Error & { code?: string }
  if (typeof result.code === 'string' && result.code) {
    error.code = result.code
  }
  throw error
}

function isServiceUsable(status: unknown) {
  return Boolean(
    status &&
      typeof status === 'object' &&
      'installed' in status &&
      'running' in status &&
      'available' in status &&
      (status as { installed?: unknown }).installed === true &&
      (status as { running?: unknown }).running === true &&
      (status as { available?: unknown }).available === true,
  )
}

function isServiceUninstalled(status: unknown) {
  return Boolean(
    status &&
      typeof status === 'object' &&
      'installed' in status &&
      (status as { installed?: unknown }).installed !== true,
  )
}

function delay(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function waitForServiceCommandStatus(command: 'install' | 'uninstall') {
  const deadline = Date.now() + 8000
  let lastStatus: unknown = null

  while (Date.now() < deadline) {
    lastStatus = await runDesktopRpc('service.status')
    if (command === 'install' && isServiceUsable(lastStatus)) {
      return lastStatus
    }
    if (command === 'uninstall' && isServiceUninstalled(lastStatus)) {
      return lastStatus
    }
    await delay(250)
  }

  return lastStatus ?? runDesktopRpc('service.status')
}

async function runDesktopRpc(action: DesktopRpcAction, payload: unknown = {}) {
  if (!validRpcActions.has(action)) {
    throw new Error(`Unknown desktop RPC action: ${action}`)
  }

  const exv = resolveExvPath()
  try {
    const { stdout } = await execFileAsync(
      exv,
      ['desktop-rpc', action, JSON.stringify(payload ?? {})],
      nativeExecOptions(exv, { maxBuffer: 1024 * 1024 * 4 }),
    )
    const result = parseJsonOutput(stdout)
    if (result && result.ok === false) {
      throwRpcResultError(result)
    }
    return result
  } catch (error) {
    const execError = error as Error & { stdout?: string; stderr?: string }
    if (execError.stdout) {
      try {
        const result = parseJsonOutput(execError.stdout)
        if (result && result.ok === false) {
          throwRpcResultError(result)
        }
        return result
      } catch (parseError) {
        if (parseError instanceof Error && parseError.message !== execError.message) {
          throw parseError
        }
      }
    }

    const message = execError.stderr?.trim() || execError.message || 'Native desktop RPC failed'
    throw new Error(message)
  }
}

function shellQuote(value: string) {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function psQuote(value: string) {
  return `'${value.replace(/'/g, `''`)}'`
}

function psArray(values: string[]) {
  return `@(${values.map((value) => psQuote(value)).join(', ')})`
}

function emitEvent(type: DesktopEventType, data: unknown) {
  if (!mainWindow || mainWindow.isDestroyed()) return
  mainWindow.webContents.send(desktopIpcChannels.event, { type, data })
}

function psRuntimeEnvPrefix(exv: string) {
  const runtimeDir = resolveRuntimeDir(exv)
  return runtimeDir ? `$env:ECNUVPN_RUNTIME_DIR = ${psQuote(runtimeDir)}; ` : ''
}

function readNewLogLines(logPath: string, offset: number) {
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

function emitServiceProgress(command: 'install' | 'uninstall', line: string) {
  emitEvent('service-progress', {
    command,
    message: line.replace(/\u001b\[[0-9;]*m/g, ''),
    timestamp: new Date().toISOString(),
  })
}

async function runServiceCommandElevated(command: DesktopServiceCommand) {
  const exv = resolveExvPath()
  emitServiceProgress(command, `Starting service ${command}...`)

  if (process.platform === 'win32') {
    const logPath = join(app.getPath('temp'), `ecnu-vpn-service-${command}-${Date.now()}.log`)
    let offset = 0
    const poll = setInterval(() => {
      const next = readNewLogLines(logPath, offset)
      offset = next.offset
      for (const line of next.lines) emitServiceProgress(command, line)
    }, 250)

    const inner = [
      '$ErrorActionPreference = "Continue"',
      psRuntimeEnvPrefix(exv).trim(),
      `Set-Location ${psQuote(dirname(exv))}`,
      `& ${psQuote(exv)} service ${command} *>&1 | ForEach-Object { $_; $_ | Out-File -FilePath ${psQuote(logPath)} -Append -Encoding utf8 }`,
      'exit $LASTEXITCODE',
    ].filter(Boolean).join('; ')

    const ps = [
      'Start-Process',
      '-FilePath', psQuote('powershell.exe'),
      '-ArgumentList', psArray([
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-Command',
        inner,
      ]),
      '-WorkingDirectory', psQuote(dirname(exv)),
      '-WindowStyle', 'Hidden',
      '-Verb', 'RunAs',
      '-Wait',
      '-PassThru',
    ].join(' ')
    try {
      await execFileAsync('powershell.exe', [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-Command',
        `$p = ${ps}; if ($p.ExitCode -ne 0) { exit $p.ExitCode }`,
      ], { windowsHide: true })
    } catch (error) {
      const next = readNewLogLines(logPath, offset)
      offset = next.offset
      for (const line of next.lines) emitServiceProgress(command, line)
      throw error
    } finally {
      clearInterval(poll)
      const next = readNewLogLines(logPath, offset)
      for (const line of next.lines) emitServiceProgress(command, line)
      try {
        if (existsSync(logPath)) unlinkSync(logPath)
      } catch {
        // Temporary progress logs are best-effort cleanup.
      }
    }
    emitServiceProgress(command, `Service ${command} command completed.`)
    return
  }

  if (process.platform === 'darwin') {
    const cmd = `${shellQuote(exv)} service ${command}`
    await execFileAsync('osascript', [
      '-e',
      `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
    ])
    emitServiceProgress(command, `Service ${command} command completed.`)
    return
  }

  await execFileAsync(exv, ['service', command], nativeExecOptions(exv))
  emitServiceProgress(command, `Service ${command} command completed.`)
}

async function runDesktopRpcElevated(
  action: DesktopRpcAction,
  payload: unknown,
  followupAction: DesktopRpcAction,
) {
  if (!validRpcActions.has(action)) {
    throw new Error(`Unknown desktop RPC action: ${action}`)
  }

  const exv = resolveExvPath()

  if (process.platform === 'win32') {
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const ps = psRuntimeEnvPrefix(exv) + [
      'Start-Process',
      '-FilePath', psQuote(exv),
      '-ArgumentList', psArray(args),
      '-WorkingDirectory', psQuote(dirname(exv)),
      '-Verb', 'RunAs',
      '-Wait',
    ].join(' ')
    await execFileAsync('powershell.exe', [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      ps,
    ], { windowsHide: true })
    return runDesktopRpc(followupAction)
  }

  if (process.platform === 'darwin') {
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const command = [shellQuote(exv), ...args.map((value) => shellQuote(value))].join(' ')
    const { stdout } = await execFileAsync('osascript', [
      '-e',
      `do shell script ${JSON.stringify(command)} with administrator privileges`,
    ], { maxBuffer: 1024 * 1024 * 4 })

    const result = parseJsonOutput(stdout)
    if (result && result.ok === false) {
      throwRpcResultError(result)
    }
    return result
  }

  return runDesktopRpc(action, payload)
}

async function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1180,
    height: 760,
    minWidth: 960,
    minHeight: 640,
    title: 'ECNU VPN',
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, '..', 'preload', 'index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  })
  Menu.setApplicationMenu(null)

  if (rendererUrl()) {
    await mainWindow.loadURL(rendererUrl()!)
  } else {
    await mainWindow.loadFile(rendererIndex())
  }

  startEventPump()
}

function startEventPump() {
  if (statusTimer || logTimer) return

  statusTimer = setInterval(async () => {
    if (!mainWindow || mainWindow.isDestroyed()) return
    try {
      const status = await runDesktopRpc('status.get')
      emitEvent('status', status)
      emitEvent('heartbeat', {})
    } catch {
      emitEvent('heartbeat', {})
    }
  }, 3000)

  logTimer = setInterval(async () => {
    if (!mainWindow || mainWindow.isDestroyed()) return
    try {
      const logs = await runDesktopRpc('logs.list', { lines: 500 })
      if (Array.isArray(logs)) {
        const next = logs.slice(seenLogCount)
        seenLogCount = logs.length
        for (const entry of next) {
          emitEvent('log', entry)
        }
      }
    } catch {
      // Log streaming is best-effort; explicit log list requests still surface errors.
    }
  }, 2500)
}

function stopEventPump() {
  if (statusTimer) clearInterval(statusTimer)
  if (logTimer) clearInterval(logTimer)
  statusTimer = null
  logTimer = null
}

ipcMain.handle(desktopIpcChannels.rpc, async (_event, action: DesktopRpcAction, payload?: unknown) => {
  return runDesktopRpc(action, payload)
})

ipcMain.handle(
  desktopIpcChannels.rpcElevated,
  async (_event, action: DesktopRpcAction, payload?: unknown, followupAction: DesktopRpcAction = 'status.get') => {
    return runDesktopRpcElevated(action, payload, followupAction)
  },
)

ipcMain.handle(desktopIpcChannels.serviceCommand, async (_event, command: DesktopServiceCommand) => {
  try {
    await runServiceCommandElevated(command)
    const status = await waitForServiceCommandStatus(command)
    if (command === 'install' && !isServiceUsable(status)) {
      throw new Error('Helper service was installed but is not available to the desktop client.')
    }
    if (
      command === 'uninstall' &&
      status &&
      typeof status === 'object' &&
      (status as { installed?: unknown }).installed === true
    ) {
      throw new Error('Helper service uninstall completed, but the service is still registered.')
    }
    return status
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    try {
      const status = await runDesktopRpc('service.status')
      if (status && typeof status === 'object' && status.installed) {
        return { ...status, warning: message }
      }
    } catch {
      // Preserve the original elevated command error below.
    }
    throw error
  }
})

ipcMain.handle(desktopIpcChannels.driverInstall, async (_event, driver: DesktopDriverInstallTarget) => {
  return runDesktopRpcElevated('drivers.install', { driver }, 'drivers.status')
})

app.whenReady().then(createWindow)

app.on('window-all-closed', () => {
  stopEventPump()
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    createWindow()
  }
})
