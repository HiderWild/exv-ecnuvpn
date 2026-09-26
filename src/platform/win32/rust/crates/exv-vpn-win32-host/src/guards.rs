
use crate::service_status::ServiceState;

/// 连接路由（服务状态 → 连接方式；M3 决策表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    /// SCM 已观察到 Running：直连服务 engine。
    Service,
    /// 已装未运行（Stopped/StartPending/StopPending/Paused/…/`Other`）：connect 内部
    /// bootstrap（`ensure_service_ready`）启动服务后连接——不要求用户先点 Start。
    PromptStart,
    /// 未安装：一次性（oneshot）连接（维持既有路径；安装由 UI 独立发请求）。
    Oneshot,
}

/// 连接守卫：服务生命周期状态 → 连接路由（纯函数，穷尽；`Other(u32)` 归 PromptStart，
/// 与「已装非运行」同语义）。
#[must_use]
pub fn decide_route(state: ServiceState) -> RouteDecision {
    match state {
        ServiceState::Running => RouteDecision::Service,
        ServiceState::NotInstalled => RouteDecision::Oneshot,
        _ => RouteDecision::PromptStart,
    }
}

/// 连接模式（快照 `mode` 展示用；记录最近一次连接路由的实际模式）。
///
/// 与 `grpc_control::ENGINE_KIND_*` 共享编码（0=auto / 1=service / 2=oneshot）：
/// `as_u8`/`from_u8` 是跨层编码契约（`selected_mode` 的 `AtomicU8` 存储沿用该编码，
/// 供 keepalive ticker 后台线程无锁读取）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceMode {
    Auto,
    Service,
    Oneshot,
}

impl ServiceMode {
    /// 存储编码（与 [`crate::grpc_control::ENGINE_KIND_*`] 一致）。
    #[must_use]
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Service => 1,
            Self::Oneshot => 2,
        }
    }

    /// 从存储编码解码（未知码 fail-closed → Auto）。
    #[must_use]
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Service,
            2 => Self::Oneshot,
            _ => Self::Auto,
        }
    }

    /// 稳定 wire 字符串（`RuntimeSnapshot.mode` 取值）。
    #[must_use]
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Service => "service",
            Self::Oneshot => "oneshot",
        }
    }

    /// 路由 → 记录模式（与 M3 决策表一致：`PromptStart` 记 Auto——服务 bootstrap 尚未
    /// 换入 service engine）。
    #[must_use]
    pub fn from_route(route: RouteDecision) -> Self {
        match route {
            RouteDecision::Service => Self::Service,
            RouteDecision::Oneshot => Self::Oneshot,
            RouteDecision::PromptStart => Self::Auto,
        }
    }
}

/// 隧道粗状态（决策输入；从引擎状态投影，不由本层拥有）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelCoarse {
    /// 隧道空闲（可发起连接 / 可执行服务生命周期变更）。
    Idle,
    /// 隧道忙碌（连接中/已连接/停止中/重连中——服务生命周期变更须先经
    /// `stop_engine_for_transition` 收敛）。
    Busy,
}

impl TunnelCoarse {
    /// 从 host 快照 runtime 状态投影（纯函数；`"idle"` 之外一律视为忙碌——fail-closed）。
    #[must_use]
    pub fn from_runtime_state(state: &str) -> Self {
        if state == "idle" {
            Self::Idle
        } else {
            Self::Busy
        }
    }
}

/// 阻止服务生命周期动作的跨机不变量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// 服务未安装（无可启动/卸载）。
    ServiceNotInstalled,
    /// 服务已运行（启动动作冗余；`ensure_service_ready` 不重复提交 start）。
    ServiceAlreadyRunning,
    /// 服务处于 SCM 过渡态（start/stop/pause/continue pending——操作与过渡竞态）。
    ServiceTransitioning,
    /// 隧道忙碌（卸载等变更要求隧道先收敛到空闲）。
    TunnelBusy,
}

/// 启动守卫：只有「已安装且已停止/未知」才需要提交 start（对齐 `ensure_service_ready`
/// 的 `needs_start`——不重复提交 Running/StartPending/StopPending/Paused）。
#[must_use]
pub fn can_start(service: ServiceState) -> Result<(), BlockReason> {
    match service {
        ServiceState::Stopped | ServiceState::NotInstalled | ServiceState::Other(_) => Ok(()),
        ServiceState::Running | ServiceState::Paused => Err(BlockReason::ServiceAlreadyRunning),
        ServiceState::StartPending
        | ServiceState::StopPending
        | ServiceState::PausePending
        | ServiceState::ContinuePending => Err(BlockReason::ServiceTransitioning),
    }
}

/// 卸载守卫（跨维度）：服务已安装 + 隧道空闲。运行中/过渡态交由
/// `stop_engine_for_transition` 先收敛（本守卫不拒绝——卸载语义是「先停引擎、再删服务」）。
#[must_use]
pub fn can_uninstall(service: ServiceState, tunnel: TunnelCoarse) -> Result<(), BlockReason> {
    if !service.is_installed() {
        return Err(BlockReason::ServiceNotInstalled);
    }
    if tunnel == TunnelCoarse::Busy {
        return Err(BlockReason::TunnelBusy);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 单元测试：守卫决策矩阵（穷尽性 + 与既有 handler/route 行为一致）。
// ---------------------------------------------------------------------------
