import { app, BrowserWindow, ipcMain, Menu, nativeImage, Tray } from 'electron'
import { execFile } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import {
  desktopIpcChannels,
  desktopRpcActions,
  type DesktopCliCommand,
  type DesktopDriverInstallTarget,
  type DesktopEventType,
  type DesktopModalKind,
  type DesktopModalPayload,
  type DesktopRpcAction,
  type DesktopServiceCommand,
  type DesktopServiceInstallPromptResult,
  type DesktopWindowMode,
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
let appTray: Tray | null = null
let trayConnectionConnected = false
let trayConnectionBusy = false
let forceQuit = false
let closePromptOpen = false
let currentWindowMode: DesktopWindowMode = 'advanced'
let closePromptResolver: ((result: unknown) => void) | null = null
let activeModal: {
  kind: DesktopModalKind
  payload: DesktopModalPayload
  window: BrowserWindow
  resolve: (result: unknown) => void
} | null = null

type CloseAppChoice = 'tray' | 'quit'

type CloseAppPromptResult =
  | 'cancel'
  | CloseAppChoice
  | {
      action?: unknown
      remember?: unknown
    }

const advancedWindowBounds = {
  width: 972,
  height: 563,
}

const minimalWindowBounds = {
  width: 302,
  height: 118,
}

function rendererUrl() {
  return process.env.VITE_DEV_SERVER_URL
}

function rendererIndex() {
  return join(__dirname, '..', '..', 'dist', 'index.html')
}

function windowIconPath() {
  const packagedIcon = join(process.resourcesPath, 'icon.png')
  if (app.isPackaged && existsSync(packagedIcon)) return packagedIcon
  const devIcon = join(repoRoot(), 'webui', 'build-resources', 'icon.png')
  return existsSync(devIcon) ? devIcon : undefined
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

// Canonical state/log directory, resolved on the (non-elevated) desktop process
// so every exv invocation — including UAC-elevated ones whose ambient %APPDATA%
// differs — writes config, state and logs to the same place. Mirrors the C++
// platform defaults exactly so the CLI (`exv logs`) and the app agree.
function resolveStateDir() {
  if (process.platform === 'win32') {
    return join(app.getPath('appData'), 'ecnuvpn')
  }
  return join(app.getPath('home'), '.ecnuvpn')
}

function nativeEnv(exv = resolveExvPath()) {
  const env = { ...process.env }
  const runtimeDir = resolveRuntimeDir(exv)
  if (runtimeDir) {
    env.ECNUVPN_RUNTIME_DIR = runtimeDir
  }
  env.ECNUVPN_STATE_DIR = resolveStateDir()
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
    config_dir: resolveStateDir(),
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

function desktopPlatformContext() {
  return {
    execFileAsync,
    resolveExvPath,
    resolveRuntimeDir,
    resolveStateDir,
    nativeExecOptions,
    parseJsonOutput,
    throwRpcResultError,
    runDesktopRpc,
    emitServiceProgress,
  }
}

function boundsForWindowMode(mode: DesktopWindowMode) {
  return mode === 'minimal' ? minimalWindowBounds : advancedWindowBounds
}

function applyWindowMode(mode: DesktopWindowMode) {
  currentWindowMode = mode
  if (!mainWindow || mainWindow.isDestroyed()) return
  const bounds = boundsForWindowMode(mode)
  mainWindow.setTitle('EXV for ECNU')
  mainWindow.setMinimumSize(bounds.width, bounds.height)
  mainWindow.setSize(bounds.width, bounds.height)
}

async function loadRendererRoute(window: BrowserWindow, route: string) {
  const devUrl = rendererUrl()
  if (devUrl) {
    const url = new URL(devUrl)
    url.hash = route
    await window.loadURL(url.toString())
    return
  }

  await window.loadFile(rendererIndex(), { hash: route })
}

function modalBounds(kind: DesktopModalKind) {
  if (kind === 'password') return { width: 420, height: 270 }
  if (kind === 'close-app') return { width: 440, height: 312 }
  return { width: 420, height: 220 }
}

function defaultModalResult(kind: DesktopModalKind) {
  if (kind === 'close-app') return 'cancel'
  if (kind === 'confirm') return false
  if (kind === 'password') return null
  return 'dismiss'
}

function closeActiveModal(result: unknown) {
  const modal = activeModal
  activeModal = null
  setMainWindowModalBlocked(false)
  modal?.resolve(result)
  if (modal?.window && !modal.window.isDestroyed()) {
    modal.window.close()
  }
}

function resolveRendererClosePrompt(result: unknown) {
  const resolver = closePromptResolver
  closePromptResolver = null
  resolver?.(result)
}

function setMainWindowModalBlocked(blocked: boolean) {
  if (!mainWindow || mainWindow.isDestroyed()) return
  if (process.platform === 'win32' || process.platform === 'linux') {
    mainWindow.setEnabled(!blocked)
  }
}

async function openModal(kind: DesktopModalKind, payload: DesktopModalPayload): Promise<unknown> {
  if (!mainWindow || mainWindow.isDestroyed()) return defaultModalResult(kind)
  if (activeModal) return defaultModalResult(kind)

  return new Promise<unknown>((resolvePrompt) => {
    const bounds = modalBounds(kind)
    const modalWindow = new BrowserWindow({
      parent: mainWindow!,
      modal: true,
      frame: false,
      transparent: kind !== 'close-app',
      backgroundColor: kind === 'close-app' ? '#111827' : '#00000000',
      hasShadow: kind !== 'close-app',
      width: bounds.width,
      height: bounds.height,
      minWidth: bounds.width,
      minHeight: bounds.height,
      title: 'EXV for ECNU',
      resizable: false,
      maximizable: false,
      minimizable: false,
      autoHideMenuBar: true,
      icon: windowIconPath(),
      webPreferences: {
        preload: join(__dirname, '..', 'preload', 'index.js'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: false,
      },
    })

    activeModal = {
      kind,
      payload,
      window: modalWindow,
      resolve: resolvePrompt,
    }
    setMainWindowModalBlocked(true)
    modalWindow.on('closed', () => {
      if (activeModal?.window === modalWindow) {
        const modal = activeModal
        activeModal = null
        setMainWindowModalBlocked(false)
        modal.resolve(defaultModalResult(modal.kind))
      }
    })
    void loadRendererRoute(modalWindow, `/modal/${kind}`).catch(() => {
      closeActiveModal(defaultModalResult(kind))
    })
  })
}

async function openRendererClosePrompt(): Promise<unknown> {
  if (!mainWindow || mainWindow.isDestroyed()) return 'cancel'
  if (closePromptResolver) return 'cancel'

  return new Promise((resolvePrompt) => {
    closePromptResolver = resolvePrompt
    emitEvent('close-request', {})
  })
}

async function openServiceInstallPrompt(): Promise<DesktopServiceInstallPromptResult> {
  const result = await openModal('service-install', { kind: 'service-install' })
  return result === 'install' ? 'install' : 'dismiss'
}

function closePreferencePath() {
  return join(app.getPath('userData'), 'close-preference.json')
}

function readClosePreference(): CloseAppChoice | null {
  try {
    const parsed = JSON.parse(readFileSync(closePreferencePath(), 'utf8')) as { action?: unknown }
    return parsed.action === 'tray' || parsed.action === 'quit' ? parsed.action : null
  } catch {
    return null
  }
}

function saveClosePreference(action: CloseAppChoice) {
  const target = closePreferencePath()
  mkdirSync(dirname(target), { recursive: true })
  writeFileSync(target, JSON.stringify({ action }, null, 2))
}

function normalizeClosePromptResult(result: unknown): { action: CloseAppChoice | 'cancel'; remember: boolean } {
  if (result === 'tray' || result === 'quit' || result === 'cancel') {
    return { action: result, remember: false }
  }
  if (result && typeof result === 'object') {
    const raw = result as { action?: unknown; remember?: unknown }
    const action = raw.action === 'tray' || raw.action === 'quit' ? raw.action : 'cancel'
    return { action, remember: Boolean(raw.remember) }
  }
  return { action: 'cancel', remember: false }
}

function showMainWindow() {
  if (!mainWindow || mainWindow.isDestroyed()) return
  mainWindow.show()
  if (mainWindow.isMinimized()) mainWindow.restore()
  mainWindow.focus()
}

function ensureTray() {
  if (appTray) return appTray

  const iconPath = windowIconPath()
  const trayImage = iconPath ? nativeImage.createFromPath(iconPath) : nativeImage.createEmpty()
  appTray = new Tray(trayImage)
  appTray.setToolTip('EXV for ECNU')
  refreshTrayMenu()
  appTray.on('click', showMainWindow)
  return appTray
}

function refreshTrayMenu(connected = trayConnectionConnected) {
  trayConnectionConnected = connected
  if (!appTray) return

  appTray.setContextMenu(Menu.buildFromTemplate([
    { label: '显示 EXV for ECNU', click: showMainWindow },
    { type: 'separator' },
    {
      label: trayConnectionConnected ? '断开' : '连接',
      enabled: !trayConnectionBusy,
      click: () => { void toggleTrayConnection() },
    },
    { type: 'separator' },
    { label: '退出', click: () => { void disconnectThenQuit() } },
  ]))
}

function connectedFromStatus(status: unknown) {
  return Boolean(
    status &&
      typeof status === 'object' &&
      (status as { connected?: unknown }).connected === true,
  )
}

async function toggleTrayConnection() {
  if (trayConnectionBusy) return
  trayConnectionBusy = true
  refreshTrayMenu()
  try {
    const status = await runDesktopRpc('status.get')
    const connected = connectedFromStatus(status)
    refreshTrayMenu(connected)
    if (connected) {
      try {
        await runDesktopRpc('vpn.disconnect', { allow_direct_fallback: true })
      } catch {
        await platformRunner.runDesktopRpcElevated(
          desktopPlatformContext(),
          'vpn.disconnect',
          { allow_direct_fallback: true },
          'status.get',
        )
      }
    } else {
      await runDesktopRpc('vpn.connect')
    }
    const nextStatus = await runDesktopRpc('status.get')
    refreshTrayMenu(connectedFromStatus(nextStatus))
    emitEvent('status', nextStatus)
  } catch (error) {
    showMainWindow()
    const action = trayConnectionConnected ? '断开' : '连接'
    const message = error instanceof Error ? error.message : String(error)
    await openModal('confirm', {
      kind: 'confirm',
      message: `托盘${action}失败。${message}`,
    })
  } finally {
    trayConnectionBusy = false
    try {
      const status = await runDesktopRpc('status.get')
      refreshTrayMenu(connectedFromStatus(status))
    } catch {
      refreshTrayMenu()
    }
  }
}

async function disconnectThenQuit() {
  try {
    const status = await runDesktopRpc('status.get')
    if (status && typeof status === 'object' && (status as { connected?: unknown }).connected === true) {
      await platformRunner.runDesktopRpcElevated(desktopPlatformContext(), 'vpn.disconnect', {
        allow_direct_fallback: true,
      }, 'status.get')
    }
    forceQuit = true
    app.quit()
  } catch (error) {
    showMainWindow()
    const message = error instanceof Error ? error.message : String(error)
    await openModal('confirm', {
      kind: 'confirm',
      message: `断开 VPN 失败，程序仍保持打开。${message}`,
    })
  }
}

async function handleMainWindowClose() {
  if (closePromptOpen) return
  closePromptOpen = true
  try {
    showMainWindow()
    const remembered = readClosePreference()
    const result = remembered
      ? { action: remembered, remember: false }
      : normalizeClosePromptResult(
          await (currentWindowMode === 'minimal'
            ? openModal('close-app', { kind: 'close-app' })
            : openRendererClosePrompt()),
        )
    if (result.remember && result.action !== 'cancel') {
      saveClosePreference(result.action)
    }
    if (result.action === 'quit') {
      await disconnectThenQuit()
      return
    }
    if (result.action === 'tray') {
      ensureTray()
      mainWindow?.hide()
    }
  } finally {
    closePromptOpen = false
  }
}

async function initialWindowMode(): Promise<DesktopWindowMode> {
  try {
    const settings = await runDesktopRpc('config.getSettings')
    const minimalMode = settings && typeof settings === 'object'
      ? (settings as { minimal_mode?: unknown }).minimal_mode
      : undefined
    return minimalMode === true
      ? 'minimal'
      : 'advanced'
  } catch {
    return 'advanced'
  }
}

async function createWindow() {
  const mode = await initialWindowMode()
  currentWindowMode = mode
  const bounds = boundsForWindowMode(mode)
  mainWindow = new BrowserWindow({
    width: bounds.width,
    height: bounds.height,
    minWidth: bounds.width,
    minHeight: bounds.height,
    title: 'EXV for ECNU',
    resizable: false,
    maximizable: false,
    autoHideMenuBar: true,
    icon: windowIconPath(),
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

  mainWindow.on('close', (event) => {
    if (forceQuit) return
    event.preventDefault()
    void handleMainWindowClose()
  })

  startEventPump()
}

function startEventPump() {
  if (statusTimer || logTimer) return

  statusTimer = setInterval(async () => {
    if (!mainWindow || mainWindow.isDestroyed()) return
    try {
      const status = await runDesktopRpc('status.get')
      emitEvent('status', status)
      refreshTrayMenu(connectedFromStatus(status))
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
    return platformRunner.runDesktopRpcElevated(
      desktopPlatformContext(),
      action,
      withDesktopRuntimeContext(payload),
      followupAction,
    )
  },
)

ipcMain.handle(desktopIpcChannels.serviceCommand, async (_event, command: DesktopServiceCommand) => {
  try {
    await platformRunner.runServiceCommandElevated(desktopPlatformContext(), command)
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

ipcMain.handle(desktopIpcChannels.cliCommand, async (_event, command: DesktopCliCommand) => {
  return platformRunner.runCliCommand(desktopPlatformContext(), command)
})

ipcMain.handle(desktopIpcChannels.driverInstall, async (_event, driver: DesktopDriverInstallTarget) => {
  return platformRunner.runDesktopRpcElevated(
    desktopPlatformContext(),
    'drivers.install',
    { driver },
    'drivers.status',
  )
})

ipcMain.handle(desktopIpcChannels.windowMode, async (_event, mode: DesktopWindowMode) => {
  if (mode !== 'minimal' && mode !== 'advanced') {
    throw new Error(`Unknown window mode: ${String(mode)}`)
  }
  applyWindowMode(mode)
  return { ok: true, mode }
})

ipcMain.handle(desktopIpcChannels.serviceInstallPrompt, async () => {
  return openServiceInstallPrompt()
})

ipcMain.handle(desktopIpcChannels.passwordPrompt, async (_event, message: string) => {
  const result = await openModal('password', {
    kind: 'password',
    message: typeof message === 'string' ? message : '',
  })
  return typeof result === 'string' ? result : null
})

ipcMain.handle(desktopIpcChannels.confirmPrompt, async (_event, message: string) => {
  const result = await openModal('confirm', {
    kind: 'confirm',
    message: typeof message === 'string' ? message : '',
  })
  return result === true
})

ipcMain.handle(desktopIpcChannels.modalPayload, async () => {
  return activeModal?.payload ?? null
})

ipcMain.handle(desktopIpcChannels.modalResult, async (_event, result: unknown) => {
  closeActiveModal(result)
  return { ok: true }
})

ipcMain.handle(desktopIpcChannels.closePromptResult, async (_event, result: unknown) => {
  resolveRendererClosePrompt(result)
  return { ok: true }
})

app.whenReady().then(createWindow)

app.on('before-quit', () => {
  forceQuit = true
})

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
