import type {
  DesktopRpcAction,
  DesktopServiceCommand,
} from '../../shared/desktop-contract.js'
import {
  shellQuote,
  type DesktopPlatformContext,
  type DesktopPlatformRunner,
  type RpcErrorResult,
} from './base.js'

const runner: DesktopPlatformRunner = {
  async runServiceCommandElevated(
    context: DesktopPlatformContext,
    command: DesktopServiceCommand,
  ) {
    const exv = context.resolveExvPath()
    const cmd = `${shellQuote(exv)} service ${command}`
    await context.execFileAsync('osascript', [
      '-e',
      `do shell script ${JSON.stringify(cmd)} with administrator privileges`,
    ])
    context.emitServiceProgress(command, `Service ${command} command completed.`)
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