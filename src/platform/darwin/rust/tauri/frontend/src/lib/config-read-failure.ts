import type { ConfigPayload, ConfigReadFailure } from "./ipc";

export function configReadFailureMessage(failure: Pick<ConfigReadFailure, "path">): string {
  return `读取配置文件失败：${failure.path}。EXV 未修改原文件。请检查文件和上级目录的访问权限后重试，或查看日志。`;
}

/** items 不能把结构化读取失败伪装为有效空配置。 */
export function assertConfigReadable(payload: Pick<ConfigPayload, "read_failure">): void {
  if (payload.read_failure) throw new Error(configReadFailureMessage(payload.read_failure));
}
