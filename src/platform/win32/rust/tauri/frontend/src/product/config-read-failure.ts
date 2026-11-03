/**
 * S4（2026-10）：config.json 读失败的 UI 投影与文案。
 *
 * 语义分界（与 R-D3-C「损坏已恢复」通知严格区分）：
 *   * `config_recovery`（quick-start.ts）= 文件**读到了但内容损坏，已备份并
 *     恢复默认**——一次性 toast，应用照常运行；
 *   * 本模块 = 文件**读不了**（core 三档自愈：占用重试 → Owner=self 自修 DACL
 *     → 上抛——全部失败），文件字节未动（禁令：不得默认值重建/覆盖），前端
 *     弹明确错误模态；ACL 类失败提供「以管理员身份修复权限」runas 入口
 *     （仅自修失败后出现——Owner 不是自己时壳内无 WRITE_DAC，runas 是唯一路径）。
 */

/** Win32 `ERROR_ACCESS_DENIED`（ACL 类读失败，runas 修复入口适用）。 */
export const WIN32_ACCESS_DENIED = 5;
/** Win32 `ERROR_SHARING_VIOLATION`（占用，重试窗口耗尽；runas 修复无益）。 */
export const WIN32_SHARING_VIOLATION = 32;

/** core `ConfigPayload.read_failure` 的前端形状（wire 镜像）。 */
export interface ConfigReadFailureInfo {
  /** Win32 原始错误码；0 = 未知。 */
  win32_code: number;
  /** 读取失败的 config.json 全路径（本地诊断，无秘密）。 */
  path: string;
}

/** 错误模态的渲染投影（文案在此集中，测试钉住）。 */
export interface ConfigReadFailureView {
  /** `acl`：权限类（可 runas 修复）；`busy`：占用；`unreadable`：其他读失败；
   * `unreachable`：连 ConfigGet 回复都拿不到（core 不可达/壳外）。 */
  kind: "acl" | "busy" | "unreadable" | "unreachable";
  title: string;
  message: string;
  /** 仅 ACL 类显示「以管理员身份修复权限」按钮。 */
  canRepairPermissions: boolean;
}

function detailPath(info: ConfigReadFailureInfo): string {
  return info.path ? `文件：${info.path}` : "";
}

/**
 * 从 ConfigGet 回复提取读失败投影；无 `read_failure`（读取成功，含自愈成功）
 * 返回 `null`——调用方继续走快速入门/正常启动判定。
 */
export function configReadFailureView(payload: {
  read_failure?: ConfigReadFailureInfo | null;
}): ConfigReadFailureView | null {
  const info = payload.read_failure;
  if (!info) return null;
  const pathDetail = detailPath(info);
  if (info.win32_code === WIN32_ACCESS_DENIED) {
    return {
      kind: "acl",
      title: "无法读取配置文件",
      message:
        `本地配置文件无法读取（Windows 错误 5：拒绝访问），文件未被修改。${pathDetail ? `${pathDetail}。` : ""}` +
        "可以尝试以管理员身份修复文件权限后重试；修复只调整权限，不会改动文件内容。",
      canRepairPermissions: true,
    };
  }
  if (info.win32_code === WIN32_SHARING_VIOLATION) {
    return {
      kind: "busy",
      title: "配置文件被占用",
      message:
        `本地配置文件正被其他程序占用，暂时无法读取（Windows 错误 32），文件未被修改。${pathDetail ? `${pathDetail}。` : ""}` +
        "请关闭可能占用它的程序（如杀毒/备份软件）后重试。",
      canRepairPermissions: false,
    };
  }
  return {
    kind: "unreadable",
    title: "无法读取配置文件",
    message:
      `本地配置文件无法读取（Windows 错误 ${info.win32_code || "未知"}），文件未被修改。${pathDetail ? `${pathDetail}。` : ""}` +
      "请重试；若持续出现请导出日志反馈。",
    canRepairPermissions: false,
  };
}

/**
 * ConfigGet 调用本身失败（core 不可达/壳外预览）的投影：与读失败同一模态出口
 * （不再静默 console.error），但无 Win32 码、无修复入口。
 */
export function configReadUnreachableView(): ConfigReadFailureView {
  return {
    kind: "unreachable",
    title: "无法读取配置状态",
    message:
      "与后台核心服务的通信不可用，本次未能读取配置；请重试，或重启应用后再试。当前配置文件未被修改。",
    canRepairPermissions: false,
  };
}
