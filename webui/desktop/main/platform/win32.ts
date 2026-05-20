import { app } from 'electron'
import { existsSync, unlinkSync } from 'node:fs'
import { dirname, join } from 'node:path'

import type {
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'
import {
  psArray,
  psQuote,
  psRuntimeEnvPrefix,
  readNewLogLines,
  type DesktopPlatformContext,
  type DesktopPlatformRunner,
} from './base.js'

const runner: DesktopPlatformRunner = {
  resolveExvName() {
    return 'exv.exe'
  },

  shouldQuitOnWindowClose() {
    return true
  },

  resolveExvCandidates(root: string) {
    return [
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
    let offset = 0
    const poll = setInterval(() => {
      const next = readNewLogLines(logPath, offset)
      offset = next.offset
      for (const line of next.lines) context.emitServiceProgress(command, line)
    }, 250)

    const inner = [
      '$ErrorActionPreference = "Continue"',
      psRuntimeEnvPrefix(context.resolveRuntimeDir(exv)).trim(),
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
      await context.execFileAsync('powershell.exe', [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-Command',
        `$p = ${ps}; if ($p.ExitCode -ne 0) { exit $p.ExitCode }`,
      ], { windowsHide: true })
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
    }

    context.emitServiceProgress(command, `Service ${command} command completed.`)
  },

  async runDesktopRpcElevated(
    context: DesktopPlatformContext,
    action: DesktopRpcAction,
    payload: unknown,
    followupAction: DesktopRpcAction,
  ) {
    const exv = context.resolveExvPath()
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const ps = psRuntimeEnvPrefix(context.resolveRuntimeDir(exv)) + [
      'Start-Process',
      '-FilePath', psQuote(exv),
      '-ArgumentList', psArray(args),
      '-WorkingDirectory', psQuote(dirname(exv)),
      '-Verb', 'RunAs',
      '-Wait',
    ].join(' ')
    await context.execFileAsync('powershell.exe', [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      ps,
    ], { windowsHide: true })
    return context.runDesktopRpc(followupAction)
  },
}

export default runner