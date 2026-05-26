import { dirname, join } from 'node:path'

import type {
  DesktopCliCommand,
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'
import type {
  CliInstallStatus,
  DesktopPlatformContext,
  DesktopPlatformRunner,
} from './base.js'

const runner: DesktopPlatformRunner = {
  resolveExvName() {
    return 'exv'
  },

  shouldQuitOnWindowClose() {
    return true
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

  async runServiceCommandElevated(
    context: DesktopPlatformContext,
    command: DesktopServiceCommand,
  ) {
    const exv = context.resolveExvPath()
    await context.execFileAsync(exv, ['service', command], context.nativeExecOptions(exv))
    context.emitServiceProgress(command, `Service ${command} command completed.`)
  },

  runCliCommand(
    context: DesktopPlatformContext,
    _command: DesktopCliCommand,
  ): Promise<CliInstallStatus> {
    const targetPath = context.resolveExvPath()
    return Promise.resolve({
      installed: false,
      installPath: '/usr/local/bin/exv',
      targetPath,
      availableInPath: false,
      warning: 'CLI install from the desktop app is not implemented on Linux yet.',
    })
  },

  runDesktopRpcElevated(
    context: DesktopPlatformContext,
    action: DesktopRpcAction,
    payload: unknown,
    _followupAction: DesktopRpcAction,
  ) {
    return context.runDesktopRpc(action, payload)
  },
}

export default runner
