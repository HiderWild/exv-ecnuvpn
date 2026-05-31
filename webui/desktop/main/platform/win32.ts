import { app } from 'electron'
import { existsSync, mkdirSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

import type {
  DesktopCliCommand,
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'
import {
  readNewLogLines,
  psQuote,
  type CliInstallStatus,
  type DesktopPlatformContext,
  type DesktopPlatformRunner,
  type RpcErrorResult,
} from './base.js'

function psSingleQuoted(value: string) {
  return `'${value.replace(/'/g, `''`)}'`
}

function writeElevatedScript(prefix: string, body: string) {
  const scriptPath = join(app.getPath('temp'), `${prefix}-${Date.now()}.ps1`)
  writeFileSync(scriptPath, body, 'utf8')
  return scriptPath
}

async function runPowerShellScriptElevated(
  context: DesktopPlatformContext,
  scriptPath: string,
  workingDirectory: string,
) {
  const command = [
    '$p = Start-Process',
    '-FilePath', psSingleQuoted('powershell.exe'),
    '-ArgumentList', `@('-NoProfile','-ExecutionPolicy','Bypass','-File',${psSingleQuoted(scriptPath)})`,
    '-WorkingDirectory', psSingleQuoted(workingDirectory),
    '-WindowStyle', 'Hidden',
    '-Verb', 'RunAs',
    '-Wait',
    '-PassThru',
    '; if ($p.ExitCode -ne 0) { exit $p.ExitCode }',
  ].join(' ')

  await context.execFileAsync('powershell.exe', [
    '-NoProfile',
    '-ExecutionPolicy',
    'Bypass',
    '-Command',
    command,
  ], { windowsHide: true, maxBuffer: 1024 * 1024 * 4 })
}

async function launchPowerShellScriptElevated(
  context: DesktopPlatformContext,
  scriptPath: string,
  workingDirectory: string,
) {
  const command = [
    '$ErrorActionPreference = "Stop";',
    'Start-Process',
    '-FilePath', psSingleQuoted('powershell.exe'),
    '-ArgumentList', `@('-NoProfile','-ExecutionPolicy','Bypass','-File',${psSingleQuoted(scriptPath)})`,
    '-WorkingDirectory', psSingleQuoted(workingDirectory),
    '-WindowStyle', 'Hidden',
    '-Verb', 'RunAs',
  ].join(' ')

  await context.execFileAsync('powershell.exe', [
    '-NoProfile',
    '-ExecutionPolicy',
    'Bypass',
    '-Command',
    command,
  ], { windowsHide: true, maxBuffer: 1024 * 1024 })
}

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function cliInstallDir() {
  const localAppData = process.env.LOCALAPPDATA || app.getPath('userData')
  return join(localAppData, 'ECNU-VPN', 'cli')
}

function cliShimPath() {
  return join(cliInstallDir(), 'exv.cmd')
}

function normalizePathForCompare(value: string) {
  return value.replace(/[\\/]+$/, '').toLowerCase()
}

function userPathEntries() {
  const pathValue = process.env.Path || process.env.PATH || ''
  return pathValue.split(';').map((entry) => entry.trim()).filter(Boolean)
}

function cliDirInProcessPath() {
  const wanted = normalizePathForCompare(cliInstallDir())
  return userPathEntries().some((entry) => normalizePathForCompare(entry) === wanted)
}

async function userPathFromRegistry(context: DesktopPlatformContext) {
  try {
    const { stdout } = await context.execFileAsync('powershell.exe', [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      '[Environment]::GetEnvironmentVariable("Path","User")',
    ], { windowsHide: true, maxBuffer: 1024 * 1024 })
    return stdout.trim()
  } catch {
    return ''
  }
}

async function setUserPath(context: DesktopPlatformContext, value: string) {
  await context.execFileAsync('powershell.exe', [
    '-NoProfile',
    '-ExecutionPolicy',
    'Bypass',
    '-Command',
    `[Environment]::SetEnvironmentVariable("Path", ${psQuote(value)}, "User")`,
  ], { windowsHide: true, maxBuffer: 1024 * 1024 })
}

async function cliStatus(context: DesktopPlatformContext): Promise<CliInstallStatus> {
  const installPath = cliShimPath()
  const targetPath = context.resolveExvPath()
  const installed = existsSync(installPath)
  const userPath = await userPathFromRegistry(context)
  const wanted = normalizePathForCompare(cliInstallDir())
  const availableInPath =
    userPath.split(';').map((entry) => entry.trim()).filter(Boolean)
      .some((entry) => normalizePathForCompare(entry) === wanted) ||
    cliDirInProcessPath()
  const status: CliInstallStatus = { installed, installPath, targetPath, availableInPath }

  if (installed) {
    try {
      const content = readFileSync(installPath, 'utf8')
      if (!content.includes(targetPath)) {
        status.warning = 'Existing exv command points to a different binary.'
      }
    } catch {
      status.warning = 'Unable to read existing exv command shim.'
    }
  }
  return status
}

async function installCli(context: DesktopPlatformContext): Promise<CliInstallStatus> {
  const installDir = cliInstallDir()
  const installPath = cliShimPath()
  const targetPath = context.resolveExvPath()
  mkdirSync(installDir, { recursive: true })
  writeFileSync(
    installPath,
    [
      '@echo off',
      `set "EXV_TARGET=${targetPath}"`,
      '"%EXV_TARGET%" %*',
      '',
    ].join('\r\n'),
    'utf8',
  )

  const userPath = await userPathFromRegistry(context)
  const entries = userPath.split(';').map((entry) => entry.trim()).filter(Boolean)
  const wanted = normalizePathForCompare(installDir)
  if (!entries.some((entry) => normalizePathForCompare(entry) === wanted)) {
    entries.push(installDir)
    await setUserPath(context, entries.join(';'))
  }
  return cliStatus(context)
}

async function uninstallCli(context: DesktopPlatformContext): Promise<CliInstallStatus> {
  const installDir = cliInstallDir()
  const installPath = cliShimPath()
  if (existsSync(installPath)) {
    rmSync(installPath, { force: true })
  }

  const userPath = await userPathFromRegistry(context)
  const wanted = normalizePathForCompare(installDir)
  const entries = userPath.split(';').map((entry) => entry.trim()).filter(Boolean)
    .filter((entry) => normalizePathForCompare(entry) !== wanted)
  if (entries.join(';') !== userPath) {
    await setUserPath(context, entries.join(';'))
  }
  return cliStatus(context)
}

async function readJsonFileWhenReady(
  context: DesktopPlatformContext,
  outputPath: string,
  logPath: string,
  timeoutMs: number,
) {
  const startedAt = Date.now()
  let lastParseError: unknown

  while (Date.now() - startedAt < timeoutMs) {
    if (existsSync(outputPath)) {
      const stdout = readFileSync(outputPath, 'utf8').trim()
      if (stdout.length > 0) {
        try {
          return context.parseJsonOutput(stdout)
        } catch (error) {
          lastParseError = error
        }
      }
    }
    await sleep(250)
  }

  if (existsSync(logPath)) {
    const detail = readFileSync(logPath, 'utf8').trim()
    if (detail) {
      throw new Error(detail)
    }
  }
  if (lastParseError instanceof Error) {
    throw lastParseError
  }
  throw new Error('Timed out waiting for elevated command output.')
}

const runner: DesktopPlatformRunner = {
  resolveExvName() {
    return 'exv.exe'
  },

  shouldQuitOnWindowClose() {
    return true
  },

  resolveExvCandidates(root: string) {
    return [
      join(root, 'build', 'windows', 'electron', 'native', 'bin', 'exv.exe'),
      join(root, 'webui', 'native', 'bin', 'exv.exe'),
      join(root, 'build', 'windows', 'electron', 'release', 'win-unpacked', 'resources', 'bin', 'exv.exe'),
      join(root, 'build-windows', 'cpp', 'exv.exe'),
      join(root, 'build', 'exv.exe'),
      join(root, 'build', 'Release', 'exv.exe'),
      join(root, 'build-desktop', 'exv.exe'),
      join(root, 'build-desktop', 'Release', 'exv.exe'),
    ]
  },

  resolveRuntimeBinaryName() {
    return 'openconnect.exe'
  },

  resolveRuntimeCandidates(root: string, _resourcesPath: string, _isPackaged: boolean, exv: string, _runtimeBinaryName: string) {
    return [
      join(root, 'runtime', `${process.platform}-${process.arch}`),
      join(root, 'runtime', process.platform),
      dirname(exv),
    ]
  },

  async runServiceCommandElevated(
    context: DesktopPlatformContext,
    command: DesktopServiceCommand,
  ) {
    const exv = context.resolveExvPath()
    context.emitServiceProgress(command, `Starting service ${command}...`)

    const logPath = join(app.getPath('temp'), `ecnu-vpn-service-${command}-${Date.now()}.log`)
    const runtimeDir = context.resolveRuntimeDir(exv)
    const stateDir = context.resolveStateDir()
    const scriptPath = writeElevatedScript(`ecnu-vpn-service-${command}`, [
      '$ErrorActionPreference = "Continue"',
      runtimeDir ? `$env:ECNUVPN_RUNTIME_DIR = ${psSingleQuoted(runtimeDir)}` : '',
      `$env:ECNUVPN_STATE_DIR = ${psSingleQuoted(stateDir)}`,
      `Set-Location ${psSingleQuoted(dirname(exv))}`,
      `& ${psSingleQuoted(exv)} service ${command} *>&1 | ForEach-Object { $_; $_ | Out-File -FilePath ${psSingleQuoted(logPath)} -Append -Encoding utf8 }`,
      'exit $LASTEXITCODE',
      '',
    ].filter(Boolean).join('\n'))
    context.emitServiceProgress(command, `Elevated script: ${scriptPath}`)
    context.emitServiceProgress(command, `Progress log: ${logPath}`)
    let offset = 0
    const poll = setInterval(() => {
      const next = readNewLogLines(logPath, offset)
      offset = next.offset
      for (const line of next.lines) context.emitServiceProgress(command, line)
    }, 250)

    try {
      await runPowerShellScriptElevated(context, scriptPath, dirname(exv))
    } catch (error) {
      const next = readNewLogLines(logPath, offset)
      offset = next.offset
      for (const line of next.lines) context.emitServiceProgress(command, line)
      throw error
    } finally {
      clearInterval(poll)
      const next = readNewLogLines(logPath, offset)
      for (const line of next.lines) context.emitServiceProgress(command, line)
      try {
        if (existsSync(logPath)) unlinkSync(logPath)
      } catch {
        // Temporary progress logs are best-effort cleanup.
      }
      try {
        if (existsSync(scriptPath)) unlinkSync(scriptPath)
      } catch {
        // Temporary elevated script cleanup is best-effort.
      }
    }

    context.emitServiceProgress(command, `Service ${command} command completed.`)
  },

  runCliCommand(
    context: DesktopPlatformContext,
    command: DesktopCliCommand,
  ) {
    if (command === 'install') return installCli(context)
    if (command === 'uninstall') return uninstallCli(context)
    return cliStatus(context)
  },

  async runDesktopRpcElevated(
    context: DesktopPlatformContext,
    action: DesktopRpcAction,
    payload: unknown,
    followupAction: DesktopRpcAction,
  ) {
    void followupAction
    const exv = context.resolveExvPath()
    const nonce = Date.now()
    const outputPath = join(app.getPath('temp'), `ecnu-vpn-elevated-rpc-${nonce}.json`)
    const logPath = join(app.getPath('temp'), `ecnu-vpn-elevated-rpc-${nonce}.log`)
    const payloadPath = join(app.getPath('temp'), `ecnu-vpn-elevated-rpc-payload-${nonce}.json`)
    const runtimeDir = context.resolveRuntimeDir(exv)
    const stateDir = context.resolveStateDir()
    writeFileSync(payloadPath, JSON.stringify(payload ?? {}), 'utf8')
    const args = ['desktop-rpc-file-output', action, payloadPath, outputPath]
    const scriptPath = writeElevatedScript('ecnu-vpn-elevated-rpc', [
      '$ErrorActionPreference = "Continue"',
      runtimeDir ? `$env:ECNUVPN_RUNTIME_DIR = ${psSingleQuoted(runtimeDir)}` : '',
      `$env:ECNUVPN_STATE_DIR = ${psSingleQuoted(stateDir)}`,
      `Set-Location ${psSingleQuoted(dirname(exv))}`,
      `& ${psSingleQuoted(exv)} ${args.map((arg) => psSingleQuoted(arg)).join(' ')} 2> ${psSingleQuoted(logPath)}`,
      `$code = $LASTEXITCODE`,
      'exit $code',
      '',
    ].filter(Boolean).join('\n'))

    try {
      await launchPowerShellScriptElevated(context, scriptPath, dirname(exv))
      const result = await readJsonFileWhenReady(context, outputPath, logPath, 180_000)
      if (result && typeof result === 'object' && (result as RpcErrorResult).ok === false) {
        context.throwRpcResultError(result as RpcErrorResult)
      }
      return result
    } catch (error) {
      if (existsSync(outputPath)) {
        const stdout = readFileSync(outputPath, 'utf8')
        try {
          const result = context.parseJsonOutput(stdout)
          if (result && typeof result === 'object' && (result as RpcErrorResult).ok === false) {
            context.throwRpcResultError(result as RpcErrorResult)
          }
          return result
        } catch {
          // Preserve the elevated process failure below.
        }
      }
      if (existsSync(logPath)) {
        const detail = readFileSync(logPath, 'utf8').trim()
        if (detail) {
          throw new Error(detail)
        }
      }
      throw error
    } finally {
      try {
        if (existsSync(outputPath)) unlinkSync(outputPath)
      } catch {
        // Temporary elevated RPC output cleanup is best-effort.
      }
      try {
        if (existsSync(logPath)) unlinkSync(logPath)
      } catch {
        // Temporary elevated RPC log cleanup is best-effort.
      }
      try {
        if (existsSync(payloadPath)) unlinkSync(payloadPath)
      } catch {
        // Temporary elevated RPC payload cleanup is best-effort.
      }
      try {
        if (existsSync(scriptPath)) unlinkSync(scriptPath)
      } catch {
        // Temporary elevated script cleanup is best-effort.
      }
    }
  },
}

export default runner
