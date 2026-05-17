import { app, BrowserWindow, ipcMain, Menu } from 'electron'
import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const __dirname = dirname(fileURLToPath(import.meta.url))

const validRpcActions = new Set([
  'status.get',
  'status.get.direct',
  'vpn.connect',
  'vpn.disconnect',
  'vpn.connect.direct',
  'vpn.disconnect.direct',
  'config.getAuth',
  'config.saveAuth',
  'config.getSettings',
  'config.saveSettings',
  'config.getKey',
  'routes.list',
  'routes.add',
  'routes.remove',
  'routes.reset',
  'service.status',
  'runtime.status',
  'drivers.status',
  'drivers.install',
  'logs.list',
])

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

function resolveExvPath() {
  if (process.env.EXV_PATH && existsSync(process.env.EXV_PATH)) {
    return process.env.EXV_PATH
  }

  const exeName = process.platform === 'win32' ? 'exv.exe' : 'exv'
  const packaged = join(process.resourcesPath, 'bin', exeName)
  if (app.isPackaged && existsSync(packaged)) {
    return packaged
  }

  const repoRoot = resolve(__dirname, '..', '..', '..')
  const candidates = process.platform === 'win32'
    ? [
        join(repoRoot, 'build', 'Release', 'exv.exe'),
        join(repoRoot, 'build', 'exv.exe'),
        join(repoRoot, 'build-desktop', 'Release', 'exv.exe'),
        join(repoRoot, 'build-desktop', 'exv.exe'),
      ]
    : [
        join(repoRoot, 'build', 'exv'),
        join(repoRoot, 'build-desktop', 'exv'),
      ]

  const found = candidates.find((candidate) => existsSync(candidate))
  if (found) return found
  return candidates[0]
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
  return null
}

/** Build a structured error object that matches the VpnError protocol. */
function structuredError(
  error_type: string,
  message: string,
  recoverable = true,
  recommended_action = '',
): { ok: false; error_type: string; message: string; recoverable: boolean; recommended_action: string } {
  return { ok: false, error_type, message, recoverable, recommended_action }
}

async function runDesktopRpc(action: string, payload: unknown = {}): Promise<unknown> {
  if (!validRpcActions.has(action)) {
    return structuredError('unknown_action', `Unknown desktop RPC action: ${action}`, false)
  }

  const exv = resolveExvPath()
  try {
    const { stdout } = await execFileAsync(
      exv,
      ['desktop-rpc', action, JSON.stringify(payload ?? {})],
      { windowsHide: true, maxBuffer: 1024 * 1024 * 4 },
    )
    const result = parseJsonOutput(stdout)
    if (!result) {
      return structuredError('parse_failure', 'Native command returned non-JSON output', true, 'Retry the operation')
    }
    // Native returned a structured error with error_type — pass through
    if (result.ok === false && result.error_type) {
      // Ensure recoverable and recommended_action are present
      return {
        ok: false,
        error_type: result.error_type,
        message: result.message || 'Operation failed',
        recoverable: result.recoverable !== undefined ? result.recoverable : true,
        recommended_action: result.recommended_action || '',
      }
    }
    // Native returned {ok:false} without error_type — wrap in native_failure
    if (result.ok === false) {
      return structuredError('native_failure', result.error || result.message || 'Native desktop RPC failed')
    }
    return result
  } catch (error) {
    const execError = error as Error & { stdout?: string; stderr?: string }
    // Try to parse stdout from the failed exec
    if (execError.stdout) {
      const result = parseJsonOutput(execError.stdout)
      if (result && result.ok === false && result.error_type) {
        return {
          ok: false,
          error_type: result.error_type,
          message: result.message || 'Operation failed',
          recoverable: result.recoverable !== undefined ? result.recoverable : true,
          recommended_action: result.recommended_action || '',
        }
      }
      if (result && result.ok === false) {
        return structuredError('native_failure', result.error || result.message || 'Native desktop RPC failed')
      }
      if (result) {
        return result
      }
    }

    // Exec failed with stderr — return structured error, never throw
    const stderr = execError.stderr?.trim()
    if (stderr) {
      return structuredError('native_failure', stderr, true, 'Check that the native binary is available')
    }
    return structuredError('native_failure', execError.message || 'Native desktop RPC failed', true, 'Retry the operation')
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

async function runServiceCommandElevated(command: 'install' | 'uninstall') {
  const exv = resolveExvPath()

  if (process.platform === 'win32') {
    const ps = [
      'Start-Process',
      '-FilePath', psQuote(exv),
      '-ArgumentList', psQuote(`service ${command}`),
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
    return
  }

  if (process.platform === 'darwin') {
    const cmd = `${shellQuote(exv)} service ${command}`
    await execFileAsync('osascript', [
      '-e',
      `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
    ])
    return
  }

  await execFileAsync(exv, ['service', command], { windowsHide: true })
}

async function runDesktopRpcElevated(action: string, payload: unknown, followupAction: string) {
  const exv = resolveExvPath()

  if (process.platform === 'win32') {
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const ps = [
      'Start-Process',
      '-FilePath', psQuote(exv),
      '-ArgumentList', psArray(args),
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
      mainWindow.webContents.send('ecnu-vpn:event', { type: 'status', data: status })
      mainWindow.webContents.send('ecnu-vpn:event', { type: 'heartbeat', data: {} })
    } catch {
      mainWindow.webContents.send('ecnu-vpn:event', { type: 'heartbeat', data: {} })
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
          mainWindow.webContents.send('ecnu-vpn:event', { type: 'log', data: entry })
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

ipcMain.handle('ecnu-vpn:rpc', async (_event, action: string, payload?: unknown) => {
  return runDesktopRpc(action, payload)
})

ipcMain.handle('ecnu-vpn:vpn-command', async (_event, command: string, payload: Record<string, unknown> = {}) => {
  const actionMap: Record<string, string> = {
    connect: 'vpn.connect.direct',
    disconnect: 'vpn.disconnect.direct',
  }
  const rpcAction = actionMap[command]
  if (!rpcAction) {
    return structuredError('unknown_action', `Unknown VPN command: ${command}`, false)
  }
  try {
    // runDesktopRpcElevated runs the action via UAC elevation, then queries
    // status.get as the follow-up to confirm the result.
    const elevatedResult = await runDesktopRpcElevated(rpcAction, payload, 'status.get')
    // If the elevated RPC returned a structured error (including elevation_denied),
    // pass it through directly — no string matching needed.
    if (elevatedResult && typeof elevatedResult === 'object' && 'error_type' in elevatedResult) {
      return elevatedResult
    }
    return elevatedResult
  } catch (err: unknown) {
    // PowerShell Start-Process -Verb RunAs throws when the user cancels UAC.
    // Convert to elevation_denied structured error — no string matching.
    return structuredError(
      'elevation_denied',
      'User denied elevation request',
      true,
      'Install the helper service to avoid elevation prompts, or retry and accept the elevation request',
    )
  }
})

ipcMain.handle('ecnu-vpn:service-command', async (_event, command: 'install' | 'uninstall') => {
  await runServiceCommandElevated(command)
  return runDesktopRpc('service.status')
})

ipcMain.handle('ecnu-vpn:driver-install', async (_event, driver: 'wintun' | 'tap') => {
  try {
    return await runDesktopRpcElevated('drivers.install', { driver }, 'drivers.status')
  } catch (err: unknown) {
    return structuredError(
      'elevation_denied',
      'User denied elevation request for driver installation',
      true,
      'Retry and accept the elevation request, or install the driver manually',
    )
  }
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
