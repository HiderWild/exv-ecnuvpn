import { existsSync, lstatSync, readlinkSync } from 'node:fs'
import { dirname, join } from 'node:path'

import type {
  DesktopCliCommand,
  DesktopRpcAction,
} from '../../shared/desktop-contract.js'
import {
  shellQuote,
  type CliInstallStatus,
  type DesktopPlatformContext,
  type DesktopPlatformRunner,
  type RpcErrorResult,
} from './base.js'

const CLI_LINK_PATH = '/usr/local/bin/exv'

async function cliStatus(context: DesktopPlatformContext): Promise<CliInstallStatus> {
  const targetPath = context.resolveExvPath()
  let installed = false
  let warning: string | undefined

  if (existsSync(CLI_LINK_PATH)) {
    try {
      const stat = lstatSync(CLI_LINK_PATH)
      if (stat.isSymbolicLink()) {
        const linkTarget = readlinkSync(CLI_LINK_PATH)
        installed = linkTarget === targetPath
        if (!installed) {
          warning = `/usr/local/bin/exv points to ${linkTarget}, not this app.`
        }
      } else {
        warning = '/usr/local/bin/exv exists and is not a symlink.'
      }
    } catch {
      warning = 'Unable to inspect /usr/local/bin/exv.'
    }
  }

  let availableInPath = false
  try {
    const { stdout } = await context.execFileAsync('/bin/sh', ['-lc', 'command -v exv || true'])
    availableInPath = stdout.trim() === CLI_LINK_PATH
  } catch {
    availableInPath = installed
  }

  return { installed, installPath: CLI_LINK_PATH, targetPath, availableInPath, warning }
}

async function installCli(context: DesktopPlatformContext) {
  const targetPath = context.resolveExvPath()
  const cmd = [
    'set -e',
    `target=${shellQuote(targetPath)}`,
    `link=${shellQuote(CLI_LINK_PATH)}`,
    'mkdir -p "$(dirname "$link")"',
    'if [ -e "$link" ] && [ ! -L "$link" ]; then echo "/usr/local/bin/exv exists and is not a symlink" >&2; exit 1; fi',
    'if [ -L "$link" ] && [ "$(readlink "$link")" != "$target" ]; then echo "/usr/local/bin/exv points to another binary" >&2; exit 1; fi',
    'ln -sfn "$target" "$link"',
  ].join('; ')
  await context.execFileAsync('osascript', [
    '-e',
    `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
  ])
  return cliStatus(context)
}

async function uninstallCli(context: DesktopPlatformContext) {
  const targetPath = context.resolveExvPath()
  const cmd = [
    'set -e',
    `target=${shellQuote(targetPath)}`,
    `link=${shellQuote(CLI_LINK_PATH)}`,
    'if [ -L "$link" ] && [ "$(readlink "$link")" = "$target" ]; then rm -f "$link"; fi',
  ].join('; ')
  await context.execFileAsync('osascript', [
    '-e',
    `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
  ])
  return cliStatus(context)
}

const runner: DesktopPlatformRunner = {
  resolveExvName() {
    return 'exv'
  },

  shouldQuitOnWindowClose() {
    return false
  },

  resolveExvCandidates(root: string) {
    return [
      join(root, 'build', 'exv'),
      join(root, 'build-desktop', 'exv'),
    ]
  },

  resolveRuntimeBinaryName() {
    return 'openconnect'
  },

  resolveRuntimeCandidates(root: string, _resourcesPath: string, _isPackaged: boolean, exv: string, _runtimeBinaryName: string) {
    return [
      join(root, 'runtime', `${process.platform}-${process.arch}`),
      join(root, 'runtime', process.platform),
      dirname(exv),
    ]
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
    _followupAction: DesktopRpcAction,
  ) {
    const exv = context.resolveExvPath()
    const args = ['desktop-rpc', action, JSON.stringify(payload ?? {})]
    const command = [shellQuote(exv), ...args.map((value) => shellQuote(value))].join(' ')
    const { stdout } = await context.execFileAsync('osascript', [
      '-e',
      `do shell script ${JSON.stringify(command)} with administrator privileges`,
    ], { maxBuffer: 1024 * 1024 * 4 })

    const result = context.parseJsonOutput(stdout)
    if (result && typeof result === 'object' && (result as RpcErrorResult).ok === false) {
      context.throwRpcResultError(result as RpcErrorResult)
    }
    return result
  },
}

export default runner
