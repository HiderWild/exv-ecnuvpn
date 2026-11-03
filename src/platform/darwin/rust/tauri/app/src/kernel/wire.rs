//! wire ↔ UI 镜像类型的机械映射（win32 `kernel/wire.rs` 的 darwin 移植）。
//!
//! Darwin 前端全量接入 win32 UI 后，UI 侧镜像类型与请求组装照抄 win32 契约
//! （`exv_vpn_wire::generated` 共用同一 proto 生成代码）。与 win32 的两处平台差异：
//!   * principal digest 派生源：win32 用当前用户 SID（`sha256("sid:{sid}")`），
//!     darwin 沿用既有 Core 契约的 uid（`sha256("uid:{uid}")`）；
//!   * 随机 id 源：win32 用 uuid crate，darwin 用既有 getrandom（16 字节）。
//!
//! 政策（common.proto）：snapshots/events 永不携带口令、cookie、原始证书或自由文本
//! 诊断栈——UI 侧 RuntimeState 是 redacted 精简视图（无 attempt/lease/proof 内部
//! refs），本模块负责裁剪。

use exv_vpn_wire::generated::{self as wire};
use sha2::{Digest, Sha256};

use super::error::AppError;
use super::logs::LogEvent;
use super::state::{
    ConnectPhase, OperationReply, OperationResult, ProxyTunAdapter, ProxyTunDetection,
    ReconnectStatus, RuntimeEvent, RuntimeEventKind, RuntimeSnapshot, RuntimeState,
    SelfHealStatus, ServiceStatus, SessionIdentity, SystemProxyDetection, VpnError,
};
use super::stats::{RuntimeStats, StatsPhase};

// ---------------------------------------------------------------------------
// 请求组装（UI 意图 → wire 请求）
// ---------------------------------------------------------------------------

/// 当前进程用户的 principal digest（darwin：uid 派生 `sha256("uid:{uid}")`，
/// 与 darwin Core 侧验证同源）。解析失败 → `None`（fail closed，调用方拒绝组装）。
#[must_use]
pub(crate) fn current_principal_digest() -> Option<[u8; 32]> {
    // SAFETY: `geteuid` 无参数，只读取当前普通用户凭证。
    let uid = unsafe { libc::geteuid() };
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(format!("uid:{uid}").as_bytes()));
    Some(out)
}

/// 组装 `KernelControl.Connect` 请求：UI `ConnectIntent` → 完整 wire `ConnectIntent`
/// （lookup_key：principal_digest = 当前用户 uid 派生、method=CONNECT、随机
/// runtime_epoch；`operation_id` 由调用方（命令层）生成并传入——**同一 id 必须用于
/// 请求与回传**，core 以 lookup_key.operation_id 关联状态事件；request_digest 32
/// 字节；profile 从 UI 引用）。
///
/// UI 提交的结构化一次性凭据在此编码到既有 `secret_payload`（core 侧会零化其副本；
/// darwin core 当前从自身配置读取凭据，本载荷按 win32 契约透传）。
///
/// # Errors
/// 随机 id 生成失败 → `AppError::Internal`。
pub(crate) fn connect_request(
    intent: &super::client::ConnectIntent,
    operation_id: Vec<u8>,
) -> Result<wire::ConnectRequest, AppError> {
    let principal_digest = current_principal_digest()
        .ok_or_else(|| AppError::Internal("connect: cannot derive principal digest (uid)".to_string()))?;
    let runtime_epoch = random_16()?;
    // request_digest 契约 = 32 字节；16 字节随机 id 经 SHA-256 派生为 32 字节。
    let request_digest = sha256(&random_16()?);
    let profile = (!intent.profile_ref.is_empty()).then(|| wire::ConnectionProfileRef {
        identity_digest: sha256(intent.profile_ref.as_bytes()),
    });
    Ok(wire::ConnectRequest {
        auth_binding: None,
        intent: Some(wire::ConnectIntent {
            lookup_key: Some(wire::OperationLookupKey {
                principal_digest: principal_digest.to_vec(),
                method: wire::OperationMethod::Connect as i32,
                runtime_epoch,
                operation_id,
            }),
            request_digest,
            profile,
        }),
        secret_payload: secret_payload_for(intent)?,
    })
}

/// 在 Tauri→Core 边界将组件可见的结构化凭据编码进既有秘密载荷。这个 JSON 不会返回
/// Vue，也不作为错误内容；未提供凭据时保留旧 Rust 内部调用者的 payload 兼容路径。
fn secret_payload_for(intent: &super::client::ConnectIntent) -> Result<Vec<u8>, AppError> {
    if let Some(credentials) = &intent.credentials {
        let engine_payload = EngineCredentialPayload {
            version: 1,
            username: &credentials.username,
            password: &credentials.password,
        };
        return serde_json::to_vec(&engine_payload)
            .map_err(|_| AppError::Internal("connect: cannot encode credential payload".to_string()));
    }
    Ok(intent
        .secret_payload
        .clone()
        .unwrap_or_default()
        .into_bytes())
}

/// engine 只识别凭据包版本与用户名/密码；前端持久化选择属于 host 语义，绝不穿透本包。
#[derive(serde::Serialize)]
struct EngineCredentialPayload<'a> {
    version: u32,
    username: &'a str,
    password: &'a str,
}

/// 组装 `KernelControl.Stop` 请求（method=STOP 的完整意图；principal 派生同
/// Connect；`operation_id` 由调用方生成并传入——同 connect 关联契约）。
///
/// # Errors
/// 随机 id 生成失败 → `AppError::Internal`。
pub(crate) fn stop_request(operation_id: Vec<u8>) -> Result<wire::StopRequest, AppError> {
    let principal_digest = current_principal_digest()
        .ok_or_else(|| AppError::Internal("stop: cannot derive principal digest (uid)".to_string()))?;
    Ok(wire::StopRequest {
        intent: Some(wire::StopIntent {
            lookup_key: Some(wire::OperationLookupKey {
                principal_digest: principal_digest.to_vec(),
                method: wire::OperationMethod::Stop as i32,
                runtime_epoch: random_16()?,
                operation_id,
            }),
            request_digest: sha256(&random_16()?),
        }),
    })
}

/// 组装 `KernelControl.RespondInteraction` 请求。
///
/// `runtime_epoch` 应来自交互提示事件；UI 尚未透传该字段（命令签名只有
/// interaction_id + response_payload），此处用确定性占位（16 字节 0）——core 校验
/// 仅要求 16 字节长度，darwin core 侧该 RPC 返回 typed `Unimplemented`（按 win32
/// 同款错误路径透传给前端既有认证失败错误面）。
#[must_use]
pub(crate) fn interaction_response(
    interaction_id: Vec<u8>,
    response_payload: Vec<u8>,
) -> wire::InteractionResponse {
    wire::InteractionResponse {
        interaction_id,
        runtime_epoch: vec![0u8; 16],
        response_payload,
    }
}

/// 组装 `KernelControl.ServiceControl` 请求（W3-4/P4 v1：UI action → wire oneof
/// exactly-one；query 是非提权读，变更动作由 core typed 拒绝携带 CLI 指引）。
#[must_use]
pub(crate) fn service_control_request(
    action: &super::state::ServiceControlAction,
) -> wire::ServiceControlRequest {
    use exv_vpn_wire::generated::service_control_request::Action;
    let action = match action {
        super::state::ServiceControlAction::Query => {
            Action::Query(wire::ServiceStatusQuery {})
        }
        super::state::ServiceControlAction::Install => Action::Install(wire::ServiceInstall {}),
        super::state::ServiceControlAction::Uninstall => {
            Action::Uninstall(wire::ServiceUninstall { full_cleanup: false })
        }
        super::state::ServiceControlAction::Start => Action::Start(wire::ServiceStart {}),
        super::state::ServiceControlAction::RotateKey => {
            Action::RotateKey(wire::ServiceRotateKey {})
        }
    };
    wire::ServiceControlRequest {
        action: Some(action),
    }
}

/// 仅实验性完整卸载显式请求恢复与完整清理。
pub(crate) fn full_uninstall_request() -> wire::ServiceControlRequest {
    wire::ServiceControlRequest {
        action: Some(wire::service_control_request::Action::Uninstall(
            wire::ServiceUninstall { full_cleanup: true },
        )),
    }
}

/// wire `ServiceControlReply` → UI `ServiceControlReply`（W3-4/P4 v1 透传形态）。
#[must_use]
pub(crate) fn service_control_reply_from_wire(
    reply: &wire::ServiceControlReply,
) -> super::state::ServiceControlReply {
    super::state::ServiceControlReply {
        service_version: None,
        service_status: reply.service_status.as_ref().map(service_status_from_wire),
        ok: reply.ok,
        message: reply.message.clone(),
    }
}

/// 16 字节随机 id（win32 用 uuid crate；darwin 沿用 getrandom，语义等价）。
fn random_16() -> Result<Vec<u8>, AppError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| AppError::Internal("entropy unavailable".to_string()))?;
    Ok(bytes.to_vec())
}

/// 32 字节 SHA-256（wire 身份 ref 的确定性派生）。
fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

// ---------------------------------------------------------------------------
// wire → UI：快照 / 事件 / 回复 / 日志
// ---------------------------------------------------------------------------

/// wire `RuntimeSnapshot` → UI `RuntimeSnapshot`（redacted：无内部 refs；
/// stats-wire 方案 A：`stats` 随快照携带，`None` = 尚无样本；C5-wire：`proxy_tun`
/// 随快照携带，`None` = 未探测/探测失败；R1：`operation_id` 随快照携带，hex16，
/// 空 = 无在途操作；S2：`system_proxy` 随快照携带；S4：`reconnect` 随快照携带；
/// EXV_UNFREEZE 2026-09-05：`self_heal` 随快照携带）。
#[must_use]
pub(crate) fn snapshot_from_wire(
    wire_snap: &wire::RuntimeSnapshot,
    monotonic_tick: u64,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        session_identity: wire_snap.session_identity.as_ref().map(|identity| SessionIdentity {
            protocol: identity.protocol.clone(),
            username: identity.username.clone(),
            server: identity.server.clone(),
        }),
        engine_reconnect: wire_snap.engine_reconnect.as_ref().map(|state| super::state::EngineReconnect { attempt: state.attempt, next_retry_at_ms: state.next_retry_at_ms, reason: state.reason.clone() }),
        stop_completion: wire_snap.stop_completion.as_ref().map(stop_completion_from_wire),
        runtime: runtime_state_from_wire(wire_snap.state.as_ref()),
        monotonic_tick,
        stats: wire_snap.stats.as_ref().map(stats_from_wire),
        proxy_tun: wire_snap
            .proxy_tun
            .as_ref()
            .map(proxy_tun_detection_from_wire),
        operation_id: opt_hex(&wire_snap.operation_id),
        service_status: wire_snap
            .service_status
            .as_ref()
            .map(service_status_from_wire),
        mode: wire_snap.mode.clone(),
        system_proxy: wire_snap
            .system_proxy
            .as_ref()
            .map(system_proxy_detection_from_wire),
        reconnect: wire_snap.reconnect.as_ref().map(reconnect_status_from_wire),
        self_heal: wire_snap.self_heal.as_ref().map(self_heal_status_from_wire),
    }
}

fn stop_completion_from_wire(value: &wire::StopCompletion) -> super::state::StopCompletion {
    super::state::StopCompletion {
        stopped: value.stopped, blocks_new_connect: value.blocks_new_connect,
        errors: value.errors.iter().map(vpn_error_from_wire).collect(),
    }
}

/// wire `RuntimeEvent` → UI `RuntimeEvent`（UI snapshot 内嵌同一 monotonic_tick；
/// 事件级 operation_id 与快照内嵌的保持一致，前端二选一即可）。
#[must_use]
pub(crate) fn event_from_wire(ev: &wire::RuntimeEvent) -> RuntimeEvent {
    let operation_id = opt_hex(&ev.operation_id);
    let snapshot = ev
        .snapshot
        .as_ref()
        .map(|s| snapshot_from_wire(s, ev.monotonic_tick))
        .unwrap_or_else(|| RuntimeSnapshot {
            session_identity: None,
            engine_reconnect: None,
            stop_completion: None,
            runtime: RuntimeState::Idle {
                last_cleanup_at_ms: None,
            },
            monotonic_tick: ev.monotonic_tick,
            stats: None,
            proxy_tun: None,
            operation_id: operation_id.clone(),
            service_status: None,
            mode: String::new(),
            system_proxy: None,
            reconnect: None,
            self_heal: None,
        });
    RuntimeEvent {
        monotonic_tick: ev.monotonic_tick,
        kind: if ev.kind == wire::RuntimeEventKind::Snapshot as i32 {
            RuntimeEventKind::Snapshot
        } else {
            RuntimeEventKind::Transition
        },
        snapshot,
        operation_id,
    }
}

/// wire `OperationReply` → UI `OperationReply`（terminal oneof → pending /
/// succeeded / failed；`terminal: None` = **pending**（R1w 异步受理，终态走 status
/// 通道），不是失败）。`operation_id` 由调用方（命令层）回填其生成的那个 id。
#[must_use]
pub(crate) fn operation_reply_from_wire(reply: &wire::OperationReply) -> OperationReply {
    let result = match reply.terminal.as_ref().and_then(|t| t.result.as_ref()) {
        Some(wire::operation_terminal::Result::Succeeded(receipt)) => OperationResult::Succeeded {
            effect_id: receipt
                .effect_id
                .iter()
                .map(hex8)
                .collect::<String>()
                .into(),
            authority_epoch: receipt
                .authority_fence
                .as_ref()
                .and_then(|f| (f.authority_epoch != 0).then_some(f.authority_epoch))
                .or(Some(receipt.ownership_version)),
        },
        Some(wire::operation_terminal::Result::Failed(failed)) => OperationResult::Failed {
            error: failed.error.as_ref().map(vpn_error_from_wire),
        },
        // terminal: None = 尚无终局（R1w pending：core 已受理，终态走 status 通道）。
        None => OperationResult::Pending,
    };
    OperationReply {
        stop_completion: reply.terminal.as_ref().and_then(|terminal| terminal.stop_completion.as_ref()).map(stop_completion_from_wire),
        result,
        operation_id: None,
    }
}

/// wire `LogEvent` → UI `LogEvent`（字段一一对应；fields 为无 secret 的键值元数据）。
#[must_use]
pub(crate) fn log_event_from_wire(ev: &wire::LogEvent) -> LogEvent {
    LogEvent {
        level: ev.level.clone(),
        component: ev.component.clone(),
        code: ev.code.clone(),
        message: ev.message.clone(),
        // wire fields 是 HashMap；UI 用 BTreeMap（确定性序列化顺序）。
        fields: ev
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        timestamp_ms: ev.timestamp_ms,
    }
}

/// wire `RuntimeStats` → UI `RuntimeStats`（stats-wire 方案 A；字段一一对应）。
#[must_use]
fn stats_from_wire(w: &wire::RuntimeStats) -> RuntimeStats {
    RuntimeStats {
        rx_bytes: w.rx_bytes,
        tx_bytes: w.tx_bytes,
        rx_rate_bps: w.rx_rate_bps,
        tx_rate_bps: w.tx_rate_bps,
        latency_ms: w.latency_ms,
        phase: stats_phase_from_wire(w.phase),
        engine_sequence: w.engine_sequence,
        sample_tick: w.sample_tick,
    }
}

/// wire `StatsPhase` int → UI `StatsPhase`（未知/未指定 → `Unspecified`）。
#[must_use]
fn stats_phase_from_wire(phase: i32) -> StatsPhase {
    match phase {
        1 => StatsPhase::Idle,
        2 => StatsPhase::Connecting,
        3 => StatsPhase::Connected,
        4 => StatsPhase::Stopping,
        5 => StatsPhase::Failed,
        _ => StatsPhase::Unspecified,
    }
}

/// wire `ProxyTunDetection` → UI `ProxyTunDetection`（C5-wire；字段一一对应）。
#[must_use]
fn proxy_tun_detection_from_wire(w: &wire::ProxyTunDetection) -> ProxyTunDetection {
    ProxyTunDetection {
        detected: w.detected,
        adapters: w
            .adapters
            .iter()
            .map(|a| ProxyTunAdapter {
                name: a.name.clone(),
                description: a.description.clone(),
                if_index: a.if_index,
                kind: a.kind.clone(),
            })
            .collect(),
        route_policy: w.route_policy.clone(),
    }
}

/// wire `SystemProxyDetection` → UI `SystemProxyDetection`（S2；字段一一对应）。
#[must_use]
fn system_proxy_detection_from_wire(w: &wire::SystemProxyDetection) -> SystemProxyDetection {
    SystemProxyDetection {
        mode: w.mode.clone(),
        endpoint_count: w.endpoint_count,
        bypass_merged: w.bypass_merged,
        topology: w.topology.clone(),
    }
}

/// wire `ReconnectStatus` → UI `ReconnectStatus`（S4；字段一一对应）。
#[must_use]
fn reconnect_status_from_wire(w: &wire::ReconnectStatus) -> ReconnectStatus {
    ReconnectStatus {
        auto_reconnect: w.auto_reconnect,
        max_attempts: w.max_attempts,
        current_attempt: w.current_attempt,
        active: w.active,
    }
}

/// wire `SelfHealStatus` → UI `SelfHealStatus`（EXV_UNFREEZE 2026-09-05；字段一一
/// 对应，stage 未知码原样透传——前端 fail-safe 渲染，error_code 仅 failed 非空）。
#[must_use]
fn self_heal_status_from_wire(w: &wire::SelfHealStatus) -> SelfHealStatus {
    SelfHealStatus {
        stage: w.stage.clone(),
        old_pid: w.old_pid,
        new_pid: w.new_pid,
        error_code: w.error_code.clone(),
    }
}

/// wire `ServiceStatus` → UI `ServiceStatus`（S3/D5 字段一一对应 + R3 `health_state`，
/// 仅展示）。W3-4/P4 v1 起 darwin core 的 `ServiceControl` query 携带服务代理探测
/// 的真实五态词汇；快照（GetSnapshot/WatchEvents）注入仍是 v2 挂账——快照路径携带
/// `None`，前端按 unknown 渲染。
#[must_use]
fn service_status_from_wire(w: &wire::ServiceStatus) -> ServiceStatus {
    ServiceStatus {
        installed: w.installed,
        state: w.state.clone(),
        binary_path: (!w.binary_path.is_empty()).then(|| w.binary_path.clone()),
        health_state: (!w.health_state.is_empty()).then(|| w.health_state.clone()),
    }
}

/// wire `VpnError` 的稳定 reason id → 用户文案表（阶段③ 单一文案来源的壳侧
/// 镜像；与前端 `frontend/src/lib/copy-tables.ts` 的 `REASON_COPY` 注释互指 +
/// 两侧一致性测试钉住——新增 id 必须两表同步补行）。文案与 win32 壳同源
/// （D9 计划 §3.3；NRPT 两行以 D8 版为准）。
#[must_use]
fn reason_message(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "DARWIN_ATRUST_SESSION_INVALIDATED" => "学校登录会话已失效，请重新网页登录后连接。",
        "DARWIN_ATRUST_AUTH_STAGE_GATEWAY" => "网页登录网关与本次连接不一致，请重新连接并登录正确网关。",
        "DARWIN_ATRUST_AUTH_STAGE_SOURCE" => "学校网关未确认当前网页登录会话，请重新网页登录后重试。",
        "DARWIN_ATRUST_AUTH_STAGE_RECEIVER" => "向学校网关交接登录会话失败，请检查网络后重试。",
        "DARWIN_ATRUST_AUTH_STAGE_TICKET" => "获取学校认证票据失败，请检查网络后重新登录。",
        "DARWIN_ATRUST_AUTH_STAGE_EXCHANGE" => "学校认证票据兑换失败，请重新网页登录后重试。",
        "DARWIN_ATRUST_AUTH_STAGE_VERIFIED" => "学校网关未确认交接后的会话，请重新网页登录后重试。",
        "DARWIN_ATRUST_AUTH_STAGE_RESOURCES" => "读取学校授权资源失败，请检查网络后重新连接。",
        "DARWIN_ATRUST_RESOLVER_CONFLICT" => "校园 DNS 配置位置已有其他工具的文件，请调整校园域配置或按提示手动处理。",
        "DARWIN_ATRUST_RESOLVER_WRITE_FAILED" => "写入校园 DNS 配置失败，请检查系统权限后重新连接。",
        "DARWIN_ATRUST_RESOLVER_SIGNAL_FAILED" => "通知系统装载校园 DNS 配置失败，请重新连接；仍失败请导出日志反馈。",
        "DARWIN_ATRUST_RESOLVER_GATE_NOT_LOADED" => "系统尚未装载校园 DNS 配置，请重新连接；仍失败请导出日志反馈。",

        "darwin/stop_wait" => "隧道执行任务尚未退出，停止未完成。请重新断开重试；仍失败请导出日志反馈。",
        "darwin/route_cleanup" => "隧道已请求停止，但本次连接路由删除失败。请重新断开重试；仍失败请导出日志反馈。",
        "darwin/cstp_close" => "隧道协议读写任务已结束，但关闭时报告错误。可重新断开确认；仍失败请导出日志反馈。",
        "darwin/pump_join" => "数据转发线程已退出，但退出时报告异常。可重新断开确认；仍失败请导出日志反馈。",
        "darwin/pipeline_task" => "隧道执行任务异常退出，停止过程报告错误。可重新断开确认；仍失败请导出日志反馈。",
        "darwin/stop_unconfirmed" => "停止请求未取得服务完成回执，尚不能确认停止结果。请重新断开重试。",
        "darwin/stop_task" => "停止任务异常退出，尚不能确认停止结果。请重新断开重试。",
        "darwin/stop_incomplete" => "服务未完成本次停止清理。请重新断开重试；仍失败请导出日志反馈。",
        "auth/login_rejected" => "账号或密码未通过学校网关验证，请重新输入后再试。",
        "auth/saml_required" => "该服务器要求网页单点登录，当前版本暂不支持；请联系网络管理员或改用 aTrust 协议。",
        "auth/credentials_missing" => "缺少连接所需的账号或密码，请填写后再连接。",
        "transport/session_failed" => "无法连接 VPN 服务器，请检查网络后重试；持续失败请导出日志反馈。",
        "platform/nrpt_takeover_failed" => "校园 DNS 规则冲突的自动处理未成功（已保留完整备份，未删除规则）。请重启电脑后重试；仍失败请导出日志反馈。",
        "platform/nrpt_foreign_rebuild" => "检测到其他软件正在持续重建校园 DNS 规则，连接无法建立。请重启电脑，或在对应网络软件中处理后重试；EXV 已停止自动移除。",
        "platform/route_apply" => "配置本机网络失败，连接未建立。请重试；仍失败请导出日志反馈。",
        "platform/data_plane_start" => "连接准备未完成，请重试；持续失败请导出日志反馈。",
        "platform/previous_cleanup" => "上次连接残留的网络设置清理失败，请重试；持续失败请重启电脑后再试。",
        "platform/assembly_internal" => "连接过程中发生内部错误，请重试；持续失败请导出日志反馈。",
        "platform/assembly_cancelled" => "本次连接已取消，如需继续请重新点击连接。",
        _ => return None,
    })
}

/// wire `VpnError` → UI `VpnError`（redacted：稳定 code 字符串 + reason id 文案；
/// 自由文本诊断栈永不进 UI——wire 的 native 字段是 redacted 类别，不展开）。
/// EXV_UNFREEZE 2026-10-09（tag 8）：已知 reason id → 静态用户文案；未知 id 与
/// 空 reason 维持空 message（前端 formatError 回落 code 标签，不透出 id 原文）。
#[must_use]
/// 将本次停止的原错误薄映射到已有 UI message；旧端空值保持原文案。
fn stop_error_message(err: &wire::VpnError, copy: &str) -> String {
    if err.stage != wire::ErrorStage::Teardown as i32 {
        return copy.to_owned();
    }
    let detail = err.operation_detail.as_deref().map(|text|
        text.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect::<String>()
            .split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect::<String>()
    ).unwrap_or_default();
    let native = err.native.as_ref().map(|native| {
        let namespace = match wire::NativeErrorNamespace::try_from(native.namespace).ok() {
            Some(wire::NativeErrorNamespace::Win32) => "Win32",
            Some(wire::NativeErrorNamespace::Hresult) => "HRESULT",
            Some(wire::NativeErrorNamespace::Ntstatus) => "NTSTATUS",
            Some(wire::NativeErrorNamespace::PosixErrno) => "POSIX errno",
            Some(wire::NativeErrorNamespace::AppleOsStatus) => "Apple OSStatus",
            Some(wire::NativeErrorNamespace::AppleMach) => "Apple Mach",
            Some(wire::NativeErrorNamespace::AppleNetworkExtension) => "Apple NetworkExtension",
            _ => "原生错误",
        };
        format!("{namespace}: {}", native.code)
    });
    if detail.is_empty() && native.is_none() { return copy.to_owned(); }
    let base = if copy.is_empty() { "停止清理失败" } else { copy };
    let mut message = base.to_owned();
    if !detail.is_empty() { message.push_str("："); message.push_str(&detail); }
    if let Some(native) = native { message.push_str("（"); message.push_str(&native); message.push('）'); }
    message
}

#[must_use]
fn vpn_error_from_wire(err: &wire::VpnError) -> VpnError {
    // 原 EOF producer 的已知通信结束事实；保留旧端空 reason 的原回落。
    let message = if err.code == wire::ErrorCode::EffectUnknown as i32
        && err.stage == wire::ErrorStage::ConnectingControl as i32
        && err.reason == "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING"
    {
        "自动恢复缺少登录材料，请重新登录后连接。"
    } else if err.code == wire::ErrorCode::EffectUnknown as i32
        && err.stage == wire::ErrorStage::ProtocolSession as i32
        && err.reason == "session/engine_control_stream_ended"
    {
        "与本地 EXV 引擎的通信已中断，当前 VPN 连接已断开。"
    } else {
        (!err.reason.is_empty()).then(|| reason_message(err.reason.as_str())).flatten().unwrap_or("")
    };
    VpnError {
        code: wire::ErrorCode::try_from(err.code)
            .map(|c| c.as_str_name().to_string())
            .unwrap_or_else(|_| "ERROR_CODE_UNSPECIFIED".to_string()),
        message: stop_error_message(err, message),
        reason: err.reason.clone(),
    }
}

/// wire `runtime_snapshot::State` → UI `RuntimeState`（redacted 精简视图）。
#[must_use]
fn runtime_state_from_wire(state: Option<&wire::runtime_snapshot::State>) -> RuntimeState {
    use wire::runtime_snapshot::State as S;
    match state {
        Some(S::Idle(_)) => RuntimeState::Idle {
            last_cleanup_at_ms: None,
        },
        Some(S::Connecting(s)) => {
            let attempt_id = s
                .attempt
                .as_ref()
                .and_then(|a| (!a.attempt_id.is_empty()).then(|| hex(&a.attempt_id)));
            RuntimeState::Connecting {
                phase: connect_phase_from_wire(s.phase),
                attempt_id,
                phase_index: connect_phase_index(s.phase),
            }
        }
        Some(S::AwaitingInteraction(s)) => RuntimeState::AwaitingInteraction {
            attempt_id: s
                .attempt
                .as_ref()
                .and_then(|a| (!a.attempt_id.is_empty()).then(|| hex(&a.attempt_id))),
            prompt_deadline_ms: s
                .prompt
                .as_ref()
                .and_then(|p| (p.deadline != 0).then_some(p.deadline)),
        },
        Some(S::Connected(connected)) => RuntimeState::Connected {
            session_established_at_ms: (connected.session_established_at_ms > 0)
                .then_some(connected.session_established_at_ms),
            summary: None,
        },
        Some(S::Stopping(_)) => RuntimeState::Stopping { reason: None },
        Some(S::Reconciling(s)) => RuntimeState::Reconciling {
            blocking_error: s
                .obligation
                .as_ref()
                .and_then(|o| o.blocking_error.as_ref())
                .map(vpn_error_from_wire),
        },
        Some(S::FailedClean(s)) => RuntimeState::FailedClean {
            error: s.last_error.as_ref().map(vpn_error_from_wire),
        },
        Some(S::FailedDirty(s)) => RuntimeState::FailedDirty {
            error: s.last_error.as_ref().map(vpn_error_from_wire),
            has_obligation: s.obligation.is_some(),
        },
        None => RuntimeState::Idle {
            last_cleanup_at_ms: None,
        },
    }
}

/// wire `ConnectPhase` int → UI `ConnectPhase`（未知/未指定回落 `ConnectingControl`）。
#[must_use]
fn connect_phase_from_wire(phase: i32) -> ConnectPhase {
    match phase {
        1 => ConnectPhase::ObservingOwnedState,
        2 => ConnectPhase::AcquiringPlatformLease,
        3 => ConnectPhase::ConnectingControl,
        4 => ConnectPhase::AwaitingInteraction,
        5 => ConnectPhase::NegotiatingTunnel,
        6 => ConnectPhase::ApplyingPlatformTunnel,
        7 => ConnectPhase::AttachingPacketBoundary,
        8 => ConnectPhase::StartingDataPlane,
        _ => ConnectPhase::ConnectingControl,
    }
}

/// wire `ConnectPhase` int → UI 进度序号（0..=7；未指定回落 ConnectingControl=2）。
#[must_use]
fn connect_phase_index(phase: i32) -> u8 {
    connect_phase_from_wire(phase).index()
}

/// 小写 hex 编码（UI attempt_id / effect_id / operation_id 的字符串展示）。
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 非空字节的 hex 编码（`Some`），空/缺省 → `None`（R1 operation_id：空 = 无在途
/// 操作）。
fn opt_hex(bytes: &[u8]) -> Option<String> {
    (!bytes.is_empty()).then(|| hex(bytes))
}

/// 单字节 hex（effect_id 展示复用）。
fn hex8(b: &u8) -> String {
    format!("{b:02x}")
}

// ---------------------------------------------------------------------------
// 单元测试：wire → UI 映射 + 请求组装。
// ---------------------------------------------------------------------------
