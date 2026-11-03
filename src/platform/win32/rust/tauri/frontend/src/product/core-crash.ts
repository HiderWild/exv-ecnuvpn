// T9（§2.9）未连接态 core 崩溃响应的前端状态：
//   * 弱提示横幅——壳侧保守死亡判定后广播 `exv://core-lifecycle`（crashed），横幅
//     非阻塞、可忽略，附「重启 core」按钮；
//   * 唤醒失败模态——关键业务动作（保存设置/连接/快速入门）的唤醒失败以
//     `kind === "core_wake_failed"` 到达，全局强报错模态提供「尝试重启程序 / 仅退出」。
//
// 状态收敛在本模块（模块级单例，同 `ui-transient` 模式）：App 壳负责事件订阅与
// 组件挂载，业务页面只调用 `handleCoreWakeFailure` 分流错误。

import { readonly, ref } from "vue";

import { commandErrorMessage } from "../lib/command-error";
import { pushToast } from "../lib/toast";
import { kernel, type CoreLifecycleEvent } from "../lib/ipc";

/** 弱提示横幅可见性（crashed 广播置位；recovered 广播/用户重启成功/用户忽略复位）。 */
const coreCrashed = ref(false);
/** 「重启 core」按钮的进行中标记（防连点）。 */
const coreRestartBusy = ref(false);
/** 唤醒失败模态可见性。 */
const coreWakeFailureVisible = ref(false);
/** 唤醒失败模态展示的错误说明（来自壳侧稳定文案；不含凭据）。 */
const coreWakeFailureMessage = ref("");

/** 壳事件订阅的入口：crashed → 显示横幅；recovered → 撤下横幅（幂等）。 */
export function announceCoreLifecycle(event: CoreLifecycleEvent): void {
  coreCrashed.value = event.state === "crashed";
}

/** 测试隔离：复位模块状态（生产路径不调用）。 */
export function resetCoreCrashState(): void {
  coreCrashed.value = false;
  coreRestartBusy.value = false;
  coreWakeFailureVisible.value = false;
  coreWakeFailureMessage.value = "";
}

/** 唤醒失败错误的识别：壳层在唤醒失败时以该 kind 拒绝业务命令。 */
export function isCoreWakeFailure(error: unknown): boolean {
  return typeof error === "object" && error !== null
    && (error as { kind?: unknown }).kind === "core_wake_failed";
}

/**
 * core 不可达错误的识别：壳层 ConfigSet 重试环（A1-1，2026-10-09 区域 A）耗尽后
 * 仍不可达时以该 kind 拒绝业务命令。与 `isCoreWakeFailure`（走全局强报错模态）
 * 不同：core 可能仍在恢复窗口内，正确动作是「稍候重试」，由各业务面自行分流为
 * 可重试文案（快速入门/设置页），不复用全局模态。
 */
export function isCoreUnreachable(error: unknown): boolean {
  return typeof error === "object" && error !== null
    && (error as { kind?: unknown }).kind === "core_unreachable";
}

/**
 * 业务面错误分流：唤醒失败 → 打开全局强报错模态并返回 true（调用方不再走 toast）；
 * 其余错误返回 false，由调用方沿用既有处理。
 */
export function handleCoreWakeFailure(error: unknown): boolean {
  if (!isCoreWakeFailure(error)) return false;
  coreWakeFailureMessage.value = commandErrorMessage(error, "后台核心进程无法恢复。");
  coreWakeFailureVisible.value = true;
  return true;
}

/** 弱提示横幅的绑定句柄（App 壳挂载 `CoreCrashBanner` 使用）。 */
export function useCoreCrashBanner() {
  return {
    visible: readonly(coreCrashed),
    busy: readonly(coreRestartBusy),
    dismiss(): void {
      // 用户可忽略弱提示（§2.9 第 1 条：不阻塞任何界面）；core 再崩时壳会重新广播。
      coreCrashed.value = false;
    },
    restart: restartCoreFromBanner,
  };
}

/**
 * 横幅「重启 core」按钮：走壳侧 `core_restart`（确认死亡才重拉，活着幂等成功）。
 * 成功 → 撤横幅 + 成功 toast；失败 → 错误 toast（弱路径不弹模态，横幅保留）。
 */
async function restartCoreFromBanner(): Promise<void> {
  if (coreRestartBusy.value) return;
  coreRestartBusy.value = true;
  try {
    await kernel.coreRestart();
    coreCrashed.value = false;
    pushToast("后台服务已恢复。", "success");
  } catch (error) {
    pushToast(commandErrorMessage(error, "重启后台服务失败，请稍后重试。"), "error");
  } finally {
    coreRestartBusy.value = false;
  }
}

/** 唤醒失败模态的绑定句柄（App 壳挂载 `CoreWakeFailureModal` 使用）。 */
export function useCoreWakeFailureModal() {
  return {
    visible: readonly(coreWakeFailureVisible),
    message: readonly(coreWakeFailureMessage),
    async restartApp(runner: () => Promise<void> = () => kernel.appRestart()): Promise<void> {
      try {
        await runner();
      } catch (error) {
        // relaunch 失败（极少见）不静默：保留模态并提示。
        pushToast(commandErrorMessage(error, "重启程序失败，请手动退出后重新启动 EXV。"), "error");
      }
    },
    async exitApp(runner: () => Promise<void> = () => kernel.appExit()): Promise<void> {
      try {
        await runner();
      } catch (error) {
        pushToast(commandErrorMessage(error, "退出失败，请手动关闭 EXV 窗口。"), "error");
      }
    },
  };
}
