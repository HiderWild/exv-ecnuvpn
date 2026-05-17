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
  'vpn.connect',
  'vpn.disconnect',
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
  'helper.status',
  'logs.list',
])

let mainWindow: BrowserWindow | null = null
let statusTimer: NodeJS.Timeout | null = null
let logTimer: NodeJS.Timeout | null = null
let seenLogCount = 0
let cleanupPending = false

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
  throw new Error(`Native command returned non-JSON output: ${stdout.slice(0, 500)}`)
}

async function runDesktopRpc(action: string, payload: unknown = {}) {
  if (!validRpcActions.has(action)) {
    throw new Error(`Unknown desktop RPC action: ${action}`)
  }

  const exv = resolveExvPath()
  try {
    const { stdout } = await execFileAsync(
      exv,
      ['desktop-rpc', action, JSON.stringify(payload ?? {})],
      { windowsHide: true, maxBuffer: 1024 * 1024 * 4 },
    )
    const result = parseJsonOutput(stdout)
    if (result && result.ok === false) {
      throw new Error(result.error || result.message || 'Native desktop RPC failed')
    }
    return result
  } catch (error) {
    const execError = error as Error & { stdout?: string; stderr?: string }
    if (execError.stdout) {
      try {
        const result = parseJsonOutput(execError.stdout)
        if (result && result.ok === false) {
          throw new Error(result.error || result.message || 'Native desktop RPC failed')
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
    try {
      await execFileAsync('osascript', [
        '-e',
        `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
      ])
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error)
      throw new Error(msg)
    }
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

  if (process.platform === 'darwin') {
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const cmd = args.map((a) => shellQuote(a)).join(' ')
    try {
      await execFileAsync('osascript', [
        '-e',
        `do shell script ${JSON.stringify(`${shellQuote(exv)} ${cmd}`)} with administrator privileges`,
      ])
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error)
      if (msg.includes('User canceled') || msg.includes('not allowed')) {
        throw new Error('elevation_denied: User canceled administrator authorization')
      }
      throw error
    }
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
      // Inject cleanup_pending flag: set during disconnect until status shows disconnected
      status.cleanup_pending = cleanupPending
      if (status.connected === false) cleanupPending = false
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
  if (action === 'vpn.disconnect') cleanupPending = true
  return runDesktopRpc(action, payload)
})

ipcMain.handle('ecnu-vpn:service-command', async (_event, command: 'install' | 'uninstall') => {
  await runServiceCommandElevated(command)
  return runDesktopRpc('service.status')
})

ipcMain.handle('ecnu-vpn:connect-elevated', async (_event, payload?: unknown) => {
  const result = await runDesktopRpcElevated('vpn.connect', payload, 'status.get')
  // Mark the session as "elevated" so the frontend can distinguish
  // one-time osascript-authorized connections from helper-managed ones.
  if (result && typeof result === 'object' && result.connected) {
    result.session_mode = 'elevated'
  }
  return result
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