// Darwin 连接页偏好继续使用现有偏好文件；快速入门只维护一次性草稿。
import { computed } from "vue";
import { uiPrefsDraft, commitInstallServicePreference } from "./ui-prefs";
export const installServiceOnConnect = computed(() => uiPrefsDraft.value.install_service_on_connect);
export function setInstallServiceOnConnect(value: boolean): void {
  void commitInstallServicePreference(value);
}
