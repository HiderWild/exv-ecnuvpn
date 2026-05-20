import type {
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'
import type {
  DesktopPlatformContext,
  DesktopPlatformRunner,
} from './base.js'

const runner: DesktopPlatformRunner = {
  async runServiceCommandElevated(
    context: DesktopPlatformContext,
    command: DesktopServiceCommand,
  ) {
    const exv = context.resolveExvPath()
    await context.execFileAsync(exv, ['service', command], context.nativeExecOptions(exv))
    context.emitServiceProgress(command, `Service ${command} command completed.`)
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