//! Darwin 服务代理的服务健康探测（W3-4/P4 v1：`ServiceControl` query-only）。
//!
//! darwin v1 的健康维度与 win32 SCM 六事实**不同构**（故不上提 common，词汇表经
//! proto 共享即可）：service agent 安装二进制存在 × 控制 socket 可连 ×（可选）Status
//! RPC accepted。本模块只做无特权只读探测（stat 固定安装目录 + UDS connect +
//! 既有 V1 Status 帧），探测结果仅用于状态上报，绝不触发安装/启动等副作用。
//!
//! 五态映射**只用既有 wire 词汇**（proto FROZEN，不发明新字符串）：
//!
//! | 事实组合 | `health_state` | installed | state |
//! |---|---|---|---|
//! | binary ∧ socket 可连 ∧（Status 未探测/失败/接受） | `healthy` | true | `running` |
//! | binary ∧ socket 可连 ∧ Status 被拒 | `installed_unavailable` | true | `stopped` |
//! | binary ∧ socket 不可连 | `installed_unavailable` | true | `stopped` |
//! | binary 缺 ∧ socket/state leaf 残留 | `payload_orphan` | false | `stopped` |
//! | 全无 | `not_installed` | false | `stopped` |
//!
//! **`scm_orphan` 盲点（如实声明）**：win32 的 `scm_orphan`（注册项在场而二进制缺）
//! 在 darwin v1 不可见。其对应事实是「系统服务描述在场而安装二进制缺失」，但该
//! 描述路径属于系统服务域——仓库守卫禁止 darwin 生产源码触碰（连路径字符串都不
//! 得出现），core 因此既不能 stat 它也不能引用它；且服务代理 daemon 死亡时描述
//! 事实对普通用户 core 本就不可得。v1 如实声明这一维度缺失，不伪造第五个事实，
//! 也不把 `payload_orphan` 误标为 `scm_orphan`。
//!
//! fail-soft：所有探测失败（stat 错误、connect 失败、Status 超时/拒绝）都保守回落
//! ——stat 失败视为缺失、connect 失败视为不通、Status 探测失败视为未探测（不把
//! 探针失败当成 service agent 失败），绝不 panic、绝不阻塞查询路径（有界超时）。

use std::{path::Path, time::Duration};

use tokio::{net::UnixStream, time::timeout};

use exv_vpn_wire::generated::ServiceStatus;

use crate::{
    service_agent_client::{self, SOCKET_PATH},
    elevation::current_core_credentials,
};

/// 服务代理固定安装目录下安装二进制的完整路径（与代理 `platform.rs` 的
/// `SERVICE_AGENT_BINARY_PATH` 同值；core 与代理各自双写字面量，改动需同步两边）。
pub(crate) const SERVICE_AGENT_BINARY_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-service-agent";
/// service agent-owned 固定 state leaf（卸载不彻底时的残留信号之一）。
const SERVICE_AGENT_STATE_PATH: &str = "/Library/Application Support/EXV/ServiceAgent/state.v1";
/// W2.5 service engine 固定控制端点（与 service agent `ENGINE_SOCKET_PATH` 同值；
/// 连接路径直连与探测共用）。
pub(crate) const SERVICE_ENGINE_SOCKET_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/engine.sock";
/// W2.5：连接路径修复阶梯后有界等 engine 端点的上界/轮询间隔（对齐
/// `service_lifecycle` READINESS 档位）。
pub(crate) const ENGINE_ENDPOINT_WAIT_BOUND: Duration = Duration::from_secs(15);
pub(crate) const ENGINE_ENDPOINT_WAIT_POLL: Duration = Duration::from_millis(250);
/// socket connect 探测与 Status RPC 的有界上限（fail-soft；daemon 单线程接受连接，
/// 忙碌窗口内查询最多等这么久就保守回落）。
const PROBE_BOUND: Duration = Duration::from_secs(3);

/// service agent 健康探测事实（全部无特权只读可得）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServiceAgentFacts {
    /// 固定安装目录下安装二进制存在。
    pub binary_present: bool,
    /// 控制 socket 文件存在（可连的前置；缺失即 daemon 从未安装或已完全卸载）。
    pub socket_file_present: bool,
    /// service agent-owned state leaf 存在（卸载不彻底残留信号）。
    pub state_leaf_present: bool,
    /// 控制 socket 可连（有界 UDS connect 成功）。
    pub socket_connectable: bool,
    /// Status RPC 结果：`Some(true)`=accepted、`Some(false)`=daemon 应答但拒绝本
    /// owner、`None`=未探测/探测失败（有界超时、socket 不通）。探针失败保守回落，
    /// 不视作 service agent 失败。
    pub status_accepted: Option<bool>,
    /// W2.5 service engine 端点维度：engine.sock 文件在场。
    pub engine_socket_present: bool,
    /// W2.5 service engine 端点维度：engine.sock 可连（有界 connect 成功；连接
    /// 立即释放——engine 对单 client 半途关闭只关该连接）。
    pub engine_socket_connectable: bool,
}

/// W2.5：service engine 端点是否 healthy（文件在+可连；权威规范：客观事实
/// 显示 down 才触发 core 修复介入）。
#[must_use]
pub(crate) fn engine_endpoint_healthy(facts: &ServiceAgentFacts) -> bool {
    facts.engine_socket_present && facts.engine_socket_connectable
}

/// darwin v1 可见的四种统一服务健康状态（win32 既有 wire 词汇的子集；词汇本身见
/// [`Self::as_wire_str`]，与 proto `ServiceStatus.health_state` 取值一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceAgentHealth {
    /// 已安装且 daemon 在服务（socket 可连；Status 未拒绝）。
    Healthy,
    /// 已安装（binary 在）但 daemon 不可用（socket 不通，或 Status 被拒）。
    InstalledUnavailable,
    /// 未安装（binary 缺）但 socket/state leaf 残留（上次卸载不彻底）。
    PayloadOrphan,
    /// 完全未安装。
    NotInstalled,
}

impl ServiceAgentHealth {
    /// 稳定 wire 字符串（`ServiceStatus.health_state` 既有取值；不发明新词汇）。
    #[must_use]
    pub(crate) const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::InstalledUnavailable => "installed_unavailable",
            Self::PayloadOrphan => "payload_orphan",
            Self::NotInstalled => "not_installed",
        }
    }
}

/// 从探测事实派生健康状态（纯函数、表驱动；win32 `derive_health` 的 darwin 对应）。
///
/// Status 探测失败（`None`）保守保留 connect 事实——socket 可连即 healthy；只有
/// daemon **显式拒绝**（`Some(false)`，如 core 不是 enrolled owner）才回落
/// `installed_unavailable（已装但对本` owner 不可用，如实而非误报 healthy）。
#[must_use]
pub(crate) fn derive_health(facts: &ServiceAgentFacts) -> ServiceAgentHealth {
    if !facts.binary_present {
        if facts.socket_file_present || facts.state_leaf_present {
            ServiceAgentHealth::PayloadOrphan
        } else {
            ServiceAgentHealth::NotInstalled
        }
    } else if facts.socket_connectable && facts.status_accepted != Some(false) {
        ServiceAgentHealth::Healthy
    } else {
        ServiceAgentHealth::InstalledUnavailable
    }
}

/// 把事实映射为 wire `ServiceStatus`（纯函数）。
///
/// `state` 用既有词汇：healthy=`running`；其余已装=`stopped`；未装占位 `stopped`
///（win32 同款：`installed=false` 时前端不读 state，见其 `ServiceState::as_wire_str`
/// 的 `NotInstalled` 占位注释）。`binary_path` 仅安装在场时给出固定安装路径，未装为空。
#[must_use]
pub(crate) fn wire_service_status(facts: &ServiceAgentFacts) -> ServiceStatus {
    let health = derive_health(facts);
    let (installed, state, binary_path) = match health {
        ServiceAgentHealth::Healthy => (true, "running", SERVICE_AGENT_BINARY_PATH),
        ServiceAgentHealth::InstalledUnavailable => (true, "stopped", SERVICE_AGENT_BINARY_PATH),
        ServiceAgentHealth::PayloadOrphan | ServiceAgentHealth::NotInstalled => (false, "stopped", ""),
    };
    ServiceStatus {
        installed,
        state: state.to_string(),
        binary_path: binary_path.to_string(),
        health_state: health.as_wire_str().to_string(),
    }
}

/// 无特权只读探测固定安装目录（fail-soft：stat 失败视为缺失）。
fn path_present(path: &str) -> bool {
    Path::new(path).exists()
}

/// 有界 UDS connect 探测（fail-soft：失败/超时视为不通；连接立即释放——daemon 对
/// 单个 client 的半途关闭只关该连接，不影响 listener）。
async fn socket_connectable() -> bool {
    match timeout(PROBE_BOUND, UnixStream::connect(SOCKET_PATH)).await {
        Ok(Ok(_stream)) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

/// 有界 Status RPC 探测（fail-soft：超时/不可达 → None；应答但拒绝 → Some(false)）。
async fn probe_status_accepted() -> Option<bool> {
    let credentials = current_core_credentials();
    match timeout(PROBE_BOUND, service_agent_client::request_status(credentials)).await {
        Ok(Ok(())) => Some(true),
        Ok(Err(service_agent_client::ServiceAgentStatusError::Rejected)) => Some(false),
        Ok(Err(service_agent_client::ServiceAgentStatusError::Unavailable)) | Err(_) => None,
    }
}

/// 采集 service agent 健康事实（`ServiceControl` query 的唯一探测入口）。
///
/// 顺序：三个廉价 stat → 有界 connect → socket 可连时才做有界 Status RPC（不通时
/// RPC 必然失败，直接省去）。全程无特权只读；任何一步失败都保守回落，不 panic。
pub(crate) async fn probe_service_agent_facts() -> ServiceAgentFacts {
    let binary_present = path_present(SERVICE_AGENT_BINARY_PATH);
    let socket_file_present = path_present(SOCKET_PATH);
    let state_leaf_present = path_present(SERVICE_AGENT_STATE_PATH);
    let socket_connectable = socket_connectable().await;
    let status_accepted = if socket_connectable {
        probe_status_accepted().await
    } else {
        None
    };
    let engine_socket_present = path_present(SERVICE_ENGINE_SOCKET_PATH);
    let engine_socket_connectable = if engine_socket_present {
        engine_socket_connectable().await
    } else {
        false
    };
    ServiceAgentFacts {
        binary_present,
        socket_file_present,
        state_leaf_present,
        socket_connectable,
        status_accepted,
        engine_socket_present,
        engine_socket_connectable,
    }
}

/// W2.5：有界探测 engine.sock 可连性（fail-soft；连接立即释放）。
async fn engine_socket_connectable() -> bool {
    match timeout(PROBE_BOUND, UnixStream::connect(SERVICE_ENGINE_SOCKET_PATH)).await {
        Ok(Ok(_stream)) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}
