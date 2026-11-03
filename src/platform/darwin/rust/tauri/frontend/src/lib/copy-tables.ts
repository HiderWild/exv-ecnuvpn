// 阶段③（D9「需要处理」分流计划 §3.4）：失败 reason id → 用户文案的**唯一**静态
// 来源（前端侧）。壳侧 Rust 表（tauri/app/src/kernel/wire.rs 的 `reason_message`）
// 与本表注释互指 + 两侧一致性测试钉住（`copy-tables.test.ts` 与壳 wire.rs 测试
// 各自穷举断言），避免两表漂移。
//
// 纪律：
//   * 每行 = 一句话事实 + 行动指引；面向非技术用户；不出现内部代号（stage 原文、
//     reason id 原文）；未知 id 由消费方兜底文案处理（fail-safe，F-8）。
//   * 词表 v1（计划 §2.3）：新增 reason id 必须同步补文案行——穷举一致性测试
//     落地即红；两表（本表 + 壳 Rust 表）必须同步。
//   * NRPT 两行（nrpt_takeover_failed/nrpt_foreign_rebuild）以 D8 版文案为准
//     （2026-10-09 接缝终裁）；内容经 D8 阶段2 的接管失败路径产出，D9 只铺通道。

/** 稳定失败 reason id（engine `TunnelError.reason` → wire `VpnError.reason`，EXV_UNFREEZE 2026-10-09 tag 8）。 */
export const REASON_IDS = [
  "DARWIN_ATRUST_SESSION_INVALIDATED",
  "DARWIN_ATRUST_AUTH_STAGE_GATEWAY",
  "DARWIN_ATRUST_AUTH_STAGE_SOURCE",
  "DARWIN_ATRUST_AUTH_STAGE_RECEIVER",
  "DARWIN_ATRUST_AUTH_STAGE_TICKET",
  "DARWIN_ATRUST_AUTH_STAGE_EXCHANGE",
  "DARWIN_ATRUST_AUTH_STAGE_VERIFIED",
  "DARWIN_ATRUST_AUTH_STAGE_RESOURCES",
  "DARWIN_ATRUST_RESOLVER_CONFLICT",
  "DARWIN_ATRUST_RESOLVER_WRITE_FAILED",
  "DARWIN_ATRUST_RESOLVER_SIGNAL_FAILED",
  "DARWIN_ATRUST_RESOLVER_GATE_NOT_LOADED",

  "darwin/stop_wait",
  "darwin/route_cleanup",
  "darwin/cstp_close",
  "darwin/pump_join",
  "darwin/pipeline_task",
  "darwin/stop_unconfirmed",
  "darwin/stop_task",
  "darwin/stop_incomplete",

  "auth/login_rejected",
  "auth/saml_required",
  "auth/credentials_missing",
  "transport/session_failed",
  "platform/nrpt_takeover_failed",
  "platform/nrpt_foreign_rebuild",
  "platform/route_apply",
  "platform/data_plane_start",
  "platform/previous_cleanup",
  "platform/assembly_internal",
  "platform/assembly_cancelled",
] as const;

export type ReasonId = (typeof REASON_IDS)[number];

/** reason id → 用户文案（标题由 `presenter.failureTitleFor` 三分流决定，不在此表）。 */
export const REASON_COPY: Readonly<Record<ReasonId, string>> = {
  "DARWIN_ATRUST_SESSION_INVALIDATED": "学校登录会话已失效，请重新网页登录后连接。",
  "DARWIN_ATRUST_AUTH_STAGE_GATEWAY": "网页登录网关与本次连接不一致，请重新连接并登录正确网关。",
  "DARWIN_ATRUST_AUTH_STAGE_SOURCE": "学校网关未确认当前网页登录会话，请重新网页登录后重试。",
  "DARWIN_ATRUST_AUTH_STAGE_RECEIVER": "向学校网关交接登录会话失败，请检查网络后重试。",
  "DARWIN_ATRUST_AUTH_STAGE_TICKET": "获取学校认证票据失败，请检查网络后重新登录。",
  "DARWIN_ATRUST_AUTH_STAGE_EXCHANGE": "学校认证票据兑换失败，请重新网页登录后重试。",
  "DARWIN_ATRUST_AUTH_STAGE_VERIFIED": "学校网关未确认交接后的会话，请重新网页登录后重试。",
  "DARWIN_ATRUST_AUTH_STAGE_RESOURCES": "读取学校授权资源失败，请检查网络后重新连接。",
  "DARWIN_ATRUST_RESOLVER_CONFLICT": "校园 DNS 配置位置已有其他工具的文件，请调整校园域配置或按提示手动处理。",
  "DARWIN_ATRUST_RESOLVER_WRITE_FAILED": "写入校园 DNS 配置失败，请检查系统权限后重新连接。",
  "DARWIN_ATRUST_RESOLVER_SIGNAL_FAILED": "通知系统装载校园 DNS 配置失败，请重新连接；仍失败请导出日志反馈。",
  "DARWIN_ATRUST_RESOLVER_GATE_NOT_LOADED": "系统尚未装载校园 DNS 配置，请重新连接；仍失败请导出日志反馈。",

  "darwin/stop_wait": "隧道执行任务尚未退出，停止未完成。请重新断开重试；仍失败请导出日志反馈。",
  "darwin/route_cleanup": "隧道已请求停止，但本次连接路由删除失败。请重新断开重试；仍失败请导出日志反馈。",
  "darwin/cstp_close": "隧道协议读写任务已结束，但关闭时报告错误。可重新断开确认；仍失败请导出日志反馈。",
  "darwin/pump_join": "数据转发线程已退出，但退出时报告异常。可重新断开确认；仍失败请导出日志反馈。",
  "darwin/pipeline_task": "隧道执行任务异常退出，停止过程报告错误。可重新断开确认；仍失败请导出日志反馈。",
  "darwin/stop_unconfirmed": "停止请求未取得服务完成回执，尚不能确认停止结果。请重新断开重试。",
  "darwin/stop_task": "停止任务异常退出，尚不能确认停止结果。请重新断开重试。",
  "darwin/stop_incomplete": "服务未完成本次停止清理。请重新断开重试；仍失败请导出日志反馈。",

  // ---- auth 域 ----
  "auth/login_rejected": "账号或密码未通过学校网关验证，请重新输入后再试。",
  "auth/saml_required": "该服务器要求网页单点登录，当前版本暂不支持；请联系网络管理员或改用 aTrust 协议。",
  "auth/credentials_missing": "缺少连接所需的账号或密码，请填写后再连接。",
  // ---- transport 域 ----
  "transport/session_failed": "无法连接 VPN 服务器，请检查网络后重试；持续失败请导出日志反馈。",
  // ---- platform 域：NRPT 静默接管失败面（D8 版文案，接缝终裁）----
  "platform/nrpt_takeover_failed":
    "校园 DNS 规则冲突的自动处理未成功（已保留完整备份，未删除规则）。请重启电脑后重试；仍失败请导出日志反馈。",
  "platform/nrpt_foreign_rebuild":
    "检测到其他软件正在持续重建校园 DNS 规则，连接无法建立。请重启电脑，或在对应网络软件中处理后重试；EXV 已停止自动移除。",
  // ---- platform 域：装配 ----
  // T-05：仅无 native 码的路径携带此 id（engine 侧条件挂载）；有 native 码
  //（1168/5）时由壳侧 native 分档文案呈现，不查本表。
  "platform/route_apply": "配置本机网络失败，连接未建立。请重试；仍失败请导出日志反馈。",
  "platform/data_plane_start": "连接准备未完成，请重试；持续失败请导出日志反馈。",
  "platform/previous_cleanup": "上次连接残留的网络设置清理失败，请重试；持续失败请重启电脑后再试。",
  "platform/assembly_internal": "连接过程中发生内部错误，请重试；持续失败请导出日志反馈。",
  "platform/assembly_cancelled": "本次连接已取消，如需继续请重新点击连接。",
};

/**
 * 按稳定 reason id 查文案；未知/空 id 返回 null（消费方回落既有 stage/code 层，
 * 不透出 id 原文给非技术用户——fail-safe）。
 */
export function reasonCopyOf(reason: string | null | undefined): string | null {
  const trimmed = reason?.trim();
  if (!trimmed) return null;
  return (REASON_COPY as Readonly<Record<string, string>>)[trimmed] ?? null;
}

/**
 * T-22 阶段名标签表（阶段③ 修 F-8）：wire `ErrorStage`（15 值，`ERROR_STAGE_*`
 * 前缀）→ 中文名。**不是** `PRODUCT_PHASE_LABELS`（那是 8 键 ConnectPhase 表，
 * 词表错配点——2026-10-09 复审 2.3-3 裁定新增本表，勿混用）。与 win32/darwin
 * presenter 的措辞对齐（8 个重叠值与 PRODUCT_PHASE_LABELS 同词）。
 */
export const ERROR_STAGE_LABELS: Readonly<Record<string, string>> = {
  ERROR_STAGE_UNSPECIFIED: "准备连接",
  ERROR_STAGE_INGRESS: "接收连接请求",
  ERROR_STAGE_ADMISSION: "连接受理",
  ERROR_STAGE_OBSERVING_OWNED_STATE: "检查环境",
  ERROR_STAGE_ACQUIRING_PLATFORM_LEASE: "准备权限",
  ERROR_STAGE_CONNECTING_CONTROL: "连接控制",
  ERROR_STAGE_AWAITING_INTERACTION: "用户认证",
  ERROR_STAGE_NEGOTIATING_TUNNEL: "建立通道",
  ERROR_STAGE_APPLYING_PLATFORM_TUNNEL: "写入配置",
  ERROR_STAGE_ATTACHING_PACKET_BOUNDARY: "启用通道",
  ERROR_STAGE_STARTING_DATA_PLANE: "启动数据通道",
  ERROR_STAGE_PROTOCOL_SESSION: "协议会话",
  ERROR_STAGE_DATA_PLANE: "数据传输",
  ERROR_STAGE_TEARDOWN: "断开清理",
  ERROR_STAGE_RECOVERY: "恢复",
  ERROR_STAGE_JOURNAL: "本地记录",
};

/**
 * T-22 阶段名标签查询：未知/缺失 stage 回落「准备连接」（不出现英文原文——
 * F-8 的修复目标）。
 */
export function errorStageLabelOf(stage: string | null | undefined): string {
  const trimmed = stage?.trim();
  if (!trimmed) return "准备连接";
  return ERROR_STAGE_LABELS[trimmed] ?? "准备连接";
}
