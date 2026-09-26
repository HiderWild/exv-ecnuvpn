// 纯前端偏好：连接页「安装服务后连接」的默认勾选（连接受理时检测到服务未安装，是否先装）。
//
// 归属切割（重要）：
//   * 本偏好**只属于连接页**。快速入门里的「安装服务」是那份清单的一次性条目——用户在提交
//     快速入门的一瞬间决定是否安装，作用域随提交结束，**不持久化、也不与本键共享**。
//   * 两者语义不同：连接页键回答“之后检测到没服务时 UI 默认勾选什么”；快速入门条目回答
//     “本次初始化要不要装服务”。不要合并成一个键。
//   * 不进 core 配置，也不走 `ui-prefs.ts` 的 Tauri 偏好文件通道。原因是 win32/darwin
//     两端的 `tauri/app/src/ui_prefs.rs` 是固定字段的 wire 结构体（字段全为 `Option` +
//     `skip_serializing_if`，无 `deny_unknown_fields`）：前端经 `ui_prefs_set` 传入未知键
//     会在反序列化阶段即被丢弃，`to_value` 得 `{}`，该键永远不会写盘；要让 ui-prefs
//     支持必须同步改两端 Rust，把纯 UI 意图扩散到跨宿主 wire 契约，不值得。
//   * 因此本偏好落在 WebView localStorage，纯前端读写、零 Rust 改动。
//
// 已知代价：清除 WebView 数据后会回落默认 `true`；默认方向是“连接前先安装服务”，因此
// 数据丢失只回到更安全的一侧，不会造成静默跳过安装。

import { ref, type Ref } from "vue";

/** localStorage 键名；仅连接页使用。 */
export const INSTALL_SERVICE_ON_CONNECT_KEY = "install_service_on_connect";

/** “从未设置过”与存储不可用时的兜底值。 */
export const DEFAULT_INSTALL_SERVICE_ON_CONNECT = true;

function readStoredPreference(): boolean {
  if (typeof window === "undefined") return DEFAULT_INSTALL_SERVICE_ON_CONNECT;
  try {
    // 仅显式 "false" 视为取消；缺键、坏值或属性访问抛错都回落默认 true。
    return window.localStorage.getItem(INSTALL_SERVICE_ON_CONNECT_KEY) !== "false";
  } catch {
    return DEFAULT_INSTALL_SERVICE_ON_CONNECT;
  }
}

/** 模块级单例：连接页各挂载点共享同一读写源。 */
export const installServiceOnConnect: Ref<boolean> = ref(readStoredPreference());

/** 用户切换：立即写回 localStorage；存储不可用时保留本次会话值（不阻断 UI）。 */
export function setInstallServiceOnConnect(value: boolean): void {
  installServiceOnConnect.value = value;
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(INSTALL_SERVICE_ON_CONNECT_KEY, value ? "true" : "false");
  } catch {
    // 与主题/强调色同一契约：存储不可用时保留会话内选择。
  }
}

/** 测试重置：按持久层重新读取（生产路径不调用）。 */
export function resetInstallServicePreference(): void {
  installServiceOnConnect.value = readStoredPreference();
}
