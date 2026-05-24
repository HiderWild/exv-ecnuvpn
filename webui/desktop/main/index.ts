import { app, BrowserWindow, ipcMain, Menu } from 'electron'
import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
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
import { platformRunner } from './platform/index.js'
import type { RpcErrorResult } from './platform/base.js'

const execFileAsync = promisify(execFile)
const __dirname = dirname(fileURLToPath(import.meta.url))

const validRpcActions = new Set<DesktopRpcAction>(desktopRpcActions)

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

  const exeName = platformRunner.resolveExvName()
  const packaged = join(process.resourcesPath, 'bin', exeName)
  if (app.isPackaged && existsSync(packaged)) {
    return packaged
  }

  const root = repoRoot()
  const candidates = platformRunner.resolveExvCandidates(root)
  const found = candidates.find((candidate) => existsSync(candidate))
  if (found) return found
  return candidates[0]
}

function resolveRuntimeDir(exv = resolveExvPath()) {
  if (process.env.ECNUVPN_RUNTIME_DIR && existsSync(process.env.ECNUVPN_RUNTIME_DIR)) {
    return process.env.ECNUVPN_RUNTIME_DIR
  }

  const root = repoRoot()
  const runtimeBinaryName = platformRunner.resolveRuntimeBinaryName()
  const candidates = app.isPackaged
    ? [join(process.resourcesPath, 'bin')]
    : platformRunner.resolveRuntimeCandidates(root, process.resourcesPath, app.isPackaged, exv, runtimeBinaryName)

  return candidates.find((candidate) => existsSync(join(candidate, runtimeBinaryName)))
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

function withDesktopRuntimeContext(payload: unknown) {
  const context = {
    home: app.getPath('home'),
  }

  if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
    return {
      ...(payload as Record<string, unknown>),
      ...context,
    }
  }

  return context
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
  const code = typeof result.code === 'string' && result.code ? result.code : undefined
  const message = result.message || result.error || 'Native desktop RPC failed'
  const error = new Error(
    code ? `${code}: ${message}` : message,
  ) as Error & { code?: string }
  if (code) {
    error.code = code
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

function emitEvent(type: DesktopEventType, data: unknown) {
  if (!mainWindow || mainWindow.isDestroyed()) return
  mainWindow.webContents.send(desktopIpcChannels.event, { type, data })
}

function emitServiceProgress(command: 'install' | 'uninstall', line: string) {
  emitEvent('service-progress', {
    command,
    message: line.replace(/\[[0-9;]*m/g, ''),
    timestamp: new Date().toISOString(),
  })
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
    return platformRunner.runDesktopRpcElevated({
      execFileAsync,
      resolveExvPath,
      resolveRuntimeDir,
      nativeExecOptions,
      parseJsonOutput,
      throwRpcResultError,
      runDesktopRpc,
      emitServiceProgress,
    }, action, withDesktopRuntimeContext(payload), followupAction)
  },
)

ipcMain.handle(desktopIpcChannels.serviceCommand, async (_event, command: DesktopServiceCommand) => {
  try {
    await platformRunner.runServiceCommandElevated({
      execFileAsync,
      resolveExvPath,
      resolveRuntimeDir,
      nativeExecOptions,
      parseJsonOutput,
      throwRpcResultError,
      runDesktopRpc,
      emitServiceProgress,
    }, command)
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
  return platformRunner.runDesktopRpcElevated({
    execFileAsync,
    resolveExvPath,
    resolveRuntimeDir,
    nativeExecOptions,
    parseJsonOutput,
    throwRpcResultError,
    runDesktopRpc,
    emitServiceProgress,
  }, 'drivers.install', { driver }, 'drivers.status')
})

app.whenReady().then(createWindow)

app.on('window-all-closed', () => {
  stopEventPump()
  if (platformRunner.shouldQuitOnWindowClose()) {
    app.quit()
  }
})

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    createWindow()
  }
})
