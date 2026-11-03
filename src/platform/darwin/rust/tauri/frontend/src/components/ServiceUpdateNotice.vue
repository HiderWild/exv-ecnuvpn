<script setup lang="ts">
import { computed, ref } from "vue";
import type { ServiceOperationCoordinator } from "../product/service-operations";
import type { ServiceVersion } from "../lib/ipc";
import { commandErrorMessage } from "../lib/command-error";

const props = defineProps<{ operations: ServiceOperationCoordinator | null; busy?: boolean }>();
const error = ref("");

/**
 * 服务是否「逻辑过旧」：已装修订号严格落后于本构建期望修订号。
 *
 * 2026-09-30 用户裁定（对齐 Clash Verge 的服务更新体验）：提示只看逻辑修订号
 * （`DARWIN_SERVICE_REVISION`，只随服务程序逻辑变化递增），不再看文件摘要比对
 * （`matches_bundle`）——App 升级但服务逻辑未变时，二进制也可能逐字节
 * 不同，按摘要提示会迫使每次升级都重输管理员密码更新服务。摘要比对保留为设置页/
 * 日志里的诊断事实；已装超前或读不到修订号（旧版无元数据）都不提示（后者由
 * 服务面板的健康五态另行呈现）。
 */
const serviceOutdated = computed(() => {
  const version: ServiceVersion | null | undefined = props.operations?.version.value;
  return version?.installed != null && version.installed < version.expected;
});

async function update(): Promise<void> {
  if (!props.operations || props.busy || props.operations.busy.value) return;
  error.value = "";
  try {
    const reply = await props.operations.run("update");
    if (!reply.ok) error.value = reply.message || "服务更新失败，请重试或查看日志。";
  } catch (cause) {
    error.value = commandErrorMessage(cause, "服务更新未完成，请重试并完成系统管理员授权。");
  }
}
</script>

<template>
  <div v-if="serviceOutdated || error" class="service-update" role="status" data-testid="service-update-notice">
    <p v-if="serviceOutdated">已安装的后台服务版本过旧（服务逻辑有变更）。更新服务后再连接；更新会断开当前连接，保留登录信息。</p>
    <p v-else>后台服务更新未完成，可重试或查看日志。</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <button type="button" :disabled="busy || operations?.busy.value" data-testid="service-update" @click="update">更新服务</button>
  </div>
</template>

<style scoped>
.service-update { flex: none; padding: 12px; margin-bottom: 12px; border: 1px solid var(--accent); border-radius: var(--radius-md); background: var(--surface-panel); color: var(--text-primary); font-size: var(--font-size-body); }
p { margin: 0 0 8px; }
button { padding: 6px 14px; border: 0; border-radius: var(--radius-md); background: var(--accent); color: var(--accent-on); cursor: pointer; }
button:disabled { opacity: .5; cursor: default; }
</style>
