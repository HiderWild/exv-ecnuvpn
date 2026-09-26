
//! engine 崩溃自愈（P3 / plan D3）：liveness → respawn 全链路。
//!
//! core 与 engine 两进程架构中，engine 崩溃（core 存活）由 liveness 监测感知（engine 死
//! ≠ core 死）。本模块是 core 侧的自愈编排：
//!
//! 1. **teardown 兜底**：`HostComposition::on_helper_link_terminal`——撤销 admission +
//!    packet leg + 启动有界 teardown（engine 死亡 ≠ core 死亡，core 继续服务 UI）；
//! 2. **回收旧 engine 进程句柄**：`EngineSupervisor::verify_exit`（有界等待退出 /
//!    强制终止，engine 不得遗留）；
//! 3. **respawn 前 0 adapter/0 路由硬断言**（判据 4/5）：[`assert_no_residue`] 对
//!    [`ResidueProbe`] 的真实系统探测结果断言——崩溃后 Wintun adapter 进程句柄关闭
//!    内核清理（需验证）与路由（系统级，进程死不自动清）都归零才放行 respawn；
//! 4. **新 `EngineSupervisor`**（新提权拉起 + 拓扑门禁 elevated 验证）；
//! 5. **新 client**（双向认证 Named Pipe 连接新 engine）；
//! 6. **composition 身份重建**：engine PID 变 → 旧 composition 绑定的 engine peer
//!    （`compose_nonprivileged_host(&engine_peer)`）失效——重建为**全新 composition**
//!    （绑定新 peer；admission 重开、phase Idle）并重授权 gate/actor（"回 Idle"）；
//! 7. **共享 engine 槽换入新 client**（[`EngineSlot::swap`]——服务/转发器/KeepAlive
//!    下一轮即指向新 engine，无需 abort/重拉转发器）→ 新 supervisor 挂接 client。
//!
//! **不自动重连**（判据 5 / 凭据一次性零化强制）：engine 崩溃后用户**再点连接**走正常
//! 登录（凭据来自 host 磁盘 config+key.bin，非重放一次性 secret）。
//!
//! 真实 Wintun/提权类完整业务验收属 P4；本批 crate 级测试为主，`SystemResidueProbe`
//! 是轻量运行时证据（进程级、只读探测）。

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{Mutex, watch};

use exv_vpn_win32_ipc::peer_auth::VerifiedPipePeer;

use crate::composition::{HostComposition, compose_nonprivileged_host};
use crate::engine_lifecycle::{
    ElevatedEngineSpawner, EngineSlot, EngineSpawnError, EngineSupervisor,
};
use crate::grpc_control::{EngineControlGrpcClient, KernelEngineControl};
use crate::kernel_control_service::controller_peer_and_capability;
use crate::kernel_control_transport::lookup_account_name;
use crate::process_lifecycle::engine_control_pipe_name;

/// 回收旧 engine 的有界等待上界（ms）：engine 已死 → 立即 signaled；engine 挂死 →
/// 有界等待后强制终止（engine 不得遗留）。
const RESPAWN_OLD_EXIT_WAIT_MS: u32 = 3000;

// ---------------------------------------------------------------------------
// 0 残留硬断言（respawn 前置；判据 4/5）
// ---------------------------------------------------------------------------

/// 崩溃后隧道资源残留报告（[`ResidueProbe`] 的输出；`assert_no_residue` 的输入）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResidueReport {
    /// 仍存在的 engine（Wintun）adapter 数（friendly name == engine adapter 名）。
    pub exv_adapter_count: usize,
    /// 仍存在的孤儿路由数：绑定到**已不存在**接口 LUID 的 IPv4 路由行（进程死不自动清
    /// 的系统级残留）。
    pub orphan_route_count: usize,
}

impl ResidueReport {
    /// 是否零残留（0 adapter + 0 孤儿路由）。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.exv_adapter_count == 0 && self.orphan_route_count == 0
    }
}

/// 隧道资源残留探测 seam（测试注入 fake；生产 [`SystemResidueProbe`]）。
pub trait ResidueProbe: Send + Sync {
    /// 探测当前系统隧道资源残留。
    ///
    /// # Errors
    /// 系统枚举失败（`GetAdaptersAddresses`/`GetIpForwardTable2`）→ `String`（fail
    /// closed：探测不可用即按残留阻断 respawn，不静默放行）。
    fn probe(&self) -> Result<ResidueReport, String>;
}

/// **respawn 前 0 adapter/0 路由硬断言**（判据 4/5）。任一残留 → `Err`（阻断 respawn，
/// 报告残留事实）。
///
/// 0 残留语义：
/// - `exv_adapter_count == 0`：engine（Wintun）adapter 已由内核清理（进程句柄关闭）；
/// - `orphan_route_count == 0`：无绑定到已删除接口的路由残留（路由是系统级资源，进程
///   死不自动清——connect 中被杀（`process::exit` 不跑析构）可能残留，必须显式断言）。
pub fn assert_no_residue(report: &ResidueReport) -> Result<(), String> {
    if report.exv_adapter_count > 0 {
        return Err(format!(
            "respawn blocked: {} engine adapter(s) still present (residue)",
            report.exv_adapter_count
        ));
    }
    if report.orphan_route_count > 0 {
        return Err(format!(
            "respawn blocked: {} orphan route(s) bound to dead interface (residue)",
            report.orphan_route_count
        ));
    }
    Ok(())
}

/// 真实系统残留探测：`GetAdaptersAddresses`（AF_UNSPEC，全 adapter）+ `GetIpForwardTable2`
/// （IPv4 全路由行）。只读，不改任何系统状态（探测是证据采集，非行为）。
pub struct SystemResidueProbe {
    /// engine 创建的 Wintun adapter 名（`ENGINE_ADAPTER_NAME` = `ExvEngine`）。
    pub adapter_name: String,
}

impl SystemResidueProbe {
    /// 枚举全部 adapter：返回（engine adapter 计数, 现存接口 LUID 集合）。
    fn enumerate(&self) -> Result<(usize, HashSet<u64>), String> {
        use windows::Win32::NetworkManagement::IpHelper::{
            GAA_FLAG_INCLUDE_GATEWAYS, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
        };
        use windows::Win32::Networking::WinSock::AF_UNSPEC;

        /// `ERROR_BUFFER_OVERFLOW`：首次 `GetAdaptersAddresses` 只问尺寸。
        const ERROR_BUFFER_OVERFLOW: u32 = 111;

        let mut size: u32 = 0;
        // SAFETY: 首次调用只查询所需缓冲尺寸（adapter 缓冲 None；size 由系统填充）。
        let rc = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                None,
                &raw mut size,
            )
        };
        if rc != 0 && rc != ERROR_BUFFER_OVERFLOW {
            return Err(format!("GetAdaptersAddresses size query failed: {rc}"));
        }
        // Alignment: u64 buffer（IP_ADAPTER_ADDRESSES_LH 需 8 字节对齐）。
        let mut buf = vec![0u64; size as usize / 8 + 2];
        let ptr = buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // usize -> u32 是有意的：Win32 API 接收 u32 尺寸，u64 缓冲（8 字节对齐、至少
        // 请求的字节数）在支持的目标上恒适配 u32。
        #[allow(clippy::cast_possible_truncation)]
        let mut out_size = (buf.len() * 8) as u32;
        // SAFETY: 缓冲按请求尺寸分配并对齐；系统填充链表（buf 在调用期间存活）。
        let rc = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(ptr),
                &raw mut out_size,
            )
        };
        if rc != 0 {
            return Err(format!("GetAdaptersAddresses failed: {rc}"));
        }
        let mut exv_count = 0usize;
        let mut luids = HashSet::new();
        // SAFETY: 系统填充的链表；Next 链由 Length 界定（Windows 保证有效指针）。
        let mut cur = ptr;
        while !cur.is_null() {
            let a = unsafe { &*cur };
            // SAFETY: PWSTR 字符串以 NUL 结尾；to_string 读到 NUL。
            let name = unsafe { a.FriendlyName.to_string() }.unwrap_or_default();
            if name.eq_ignore_ascii_case(&self.adapter_name) {
                exv_count += 1;
            }
            // windows crate 中 `IP_ADAPTER_ADDRESSES_LH.Luid` 按 union 成员暴露，读取
            // `Value` 需 unsafe（与 resource crate 读取 `InterfaceLuid.Value` 同构）。
            let luid = unsafe { a.Luid.Value };
            luids.insert(luid);
            cur = a.Next;
        }
        Ok((exv_count, luids))
    }

    /// 捕获绑定到**不存在**接口 LUID 的 IPv4 路由行数（孤儿路由 = 系统级残留）。
    fn capture_orphan_routes(known_luids: &HashSet<u64>) -> Result<usize, String> {
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_TABLE2,
        };
        use windows::Win32::Networking::WinSock::AF_INET;

        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        // SAFETY: table 由系统分配；用毕必须 FreeMibTable。
        let rc = unsafe { GetIpForwardTable2(AF_INET, &raw mut table) }.0;
        if rc != 0 {
            return Err(format!("GetIpForwardTable2 failed: {rc}"));
        }
        if table.is_null() {
            return Ok(0);
        }
        // SAFETY: table 由系统填充；NumEntries 界内访问（显式 from_raw_parts）。
        let table_ref = unsafe { &*table };
        let rows = unsafe {
            std::slice::from_raw_parts(table_ref.Table.as_ptr(), table_ref.NumEntries as usize)
        };
        let mut orphan = 0usize;
        for r in rows {
            // SAFETY: 读结构字段 InterfaceLuid.Value（NET_LUID_LH 普通结构体）。
            let luid = unsafe { r.InterfaceLuid.Value };
            if !known_luids.contains(&luid) {
                orphan += 1;
            }
        }
        // SAFETY: 释放系统分配的表。
        unsafe { FreeMibTable(table.cast()) };
        Ok(orphan)
    }
}

impl ResidueProbe for SystemResidueProbe {
    fn probe(&self) -> Result<ResidueReport, String> {
        let (exv_adapter_count, luids) = self.enumerate()?;
        let orphan_route_count = Self::capture_orphan_routes(&luids)?;
        Ok(ResidueReport {
            exv_adapter_count,
            orphan_route_count,
        })
    }
}

// ---------------------------------------------------------------------------
// respawn 编排
// ---------------------------------------------------------------------------

/// respawn 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespawnOutcome {
    /// 新 engine 已拉起并接好（新 PID）。
    Respawned { old_pid: u32, new_pid: u32 },
}

/// respawn 失败（fail-closed typed 拒绝；阻断自愈并报告）。
#[derive(Debug)]
pub enum RespawnError {
    /// 残留探测失败（系统枚举不可用 → 按残留阻断，不静默放行）。
    ResidueProbe(String),
    /// 残留硬断言失败（0 adapter/0 路由未满足）。
    Residue(String),
    /// 新 engine 提权拉起/拓扑门禁失败。
    Spawn(EngineSpawnError),
    /// 新 client 连接失败。
    Connect(String),
    /// composition 身份重建失败。
    Rebuild(String),
}

impl std::fmt::Display for RespawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResidueProbe(e) => write!(f, "residue probe failed: {e}"),
            Self::Residue(e) => write!(f, "residue assertion failed: {e}"),
            Self::Spawn(e) => write!(f, "engine respawn failed: {e}"),
            Self::Connect(e) => write!(f, "engine reconnect failed: {e}"),
            Self::Rebuild(e) => write!(f, "composition rebuild failed: {e}"),
        }
    }
}

impl std::error::Error for RespawnError {}

// ---------------------------------------------------------------------------
// 自愈上报 seam（2026-09-05 host 自愈进展 UI 计划 §4.3）：把 respawn 阶段变化变为
// 可观测状态（真实实现桥接 EventBus lane + 日志聚合器；测试用 recording fake）。
// host 阶段变化点只经本 seam 上报，不反向依赖 kernel_control 服务内部。
// ---------------------------------------------------------------------------

/// 自愈阶段（wire `SelfHealStatus.stage` 的 host 侧枚举；计划 §4.2 冻结三态）。
///
/// 不设独立 `detected` 阶段（检测即 respawn，liveness 翻转与 `run_respawn` 开始间无
/// 用户可感知窗口——`Respawning` 的首次发布即「检测到崩溃」）；也不设 `reconnecting`
/// 阶段（respawn 不重放凭据，「连接重建」是用户重新点击连接，不是上报状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfHealStage {
    /// 检测到 engine 失联，respawn 编排进行中（含 teardown/残留探测/重建各步）。
    Respawning,
    /// 新 engine 已拉起并接好；原连接已终止，需用户重新连接。
    Succeeded,
    /// respawn 被阻/失败（fail-closed，不自动重试；携带 [`respawn_error_code`] 码）。
    Failed,
}

impl SelfHealStage {
    /// wire stage 字符串（计划 §4.2 冻结枚举；未知码由前端 fail-safe 渲染）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Respawning => "respawning",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

/// 自愈上报 seam：一次阶段变化的上报（lane 刷新 + 结构化日志由真实实现落地；随后的
/// 相位快照刷新发布由调用方在发布点驱动——`EventBus::publish_self_heal_refresh`）。
pub trait SelfHealReporter: Send + Sync {
    /// 上报一次自愈阶段变化。
    ///
    /// `old_pid` = 崩溃 engine PID（0 = 不可得）；`new_pid` = 新 engine PID（仅
    /// `Succeeded` 非零）；`error_code` = 仅 `Failed` 非空（[`respawn_error_code`] 码表）。
    fn report(&self, stage: SelfHealStage, old_pid: u32, new_pid: u32, error_code: &str);
}

/// `RespawnError` → 稳定 error_code（计划 §4.2 冻结码表；五变体穷尽映射，无秘密、
/// 无栈——与 wire `SelfHealStatus.error_code` 同源）。
#[must_use]
pub fn respawn_error_code(error: &RespawnError) -> &'static str {
    match error {
        RespawnError::ResidueProbe(_) => "residue_probe_failed",
        RespawnError::Residue(_) => "residue_assert_failed",
        RespawnError::Spawn(_) => "spawn_failed",
        RespawnError::Connect(_) => "connect_failed",
        RespawnError::Rebuild(_) => "rebuild_failed",
    }
}

/// 新 `EngineSupervisor` 的可注入工厂（测试注入 fake child，规避真实提权/elevation
/// 门禁；生产 [`ProductSupervisorFactory`] 走 `EngineSupervisor::spawn_product`）。
pub trait RespawnSupervisorFactory: Send + Sync {
    /// 提权拉起新 engine 并返回监督句柄（含拓扑门禁 elevated 验证）。
    ///
    /// # Errors
    /// [`EngineSpawnError`]（bin 缺失 / spawn 失败 / elevation 拒绝）。
    fn spawn_supervisor(&self) -> Result<EngineSupervisor, EngineSpawnError>;
}

/// 生产 supervisor 工厂：`EngineSupervisor::spawn_product`（提权拉起 + elevated 门禁）。
pub struct ProductSupervisorFactory;

impl RespawnSupervisorFactory for ProductSupervisorFactory {
    fn spawn_supervisor(&self) -> Result<EngineSupervisor, EngineSpawnError> {
        EngineSupervisor::spawn_product(&ElevatedEngineSpawner)
    }
}

/// 新 engine 控制面 client 的可注入连接器（测试注入 fake client）。
#[tonic::async_trait]
pub trait RespawnClientConnector: Send + Sync {
    /// 连接新 engine 控制面（双向认证 Named Pipe），返回共享 client + liveness 接收端。
    ///
    /// # Errors
    /// 拨号/认证失败 → `String`。
    async fn connect(
        &self,
        engine_pid: u32,
        user_sid: &str,
    ) -> Result<(Arc<Mutex<dyn KernelEngineControl>>, watch::Receiver<bool>), String>;
}

/// 生产 client 连接器：`EngineControlGrpcClient::connect`（管道名按 core PID 唯一——
/// respawn 后 core PID 不变，管道名不变；新 engine 建同名管道）。
pub struct GrpcClientConnector;

#[tonic::async_trait]
impl RespawnClientConnector for GrpcClientConnector {
    async fn connect(
        &self,
        engine_pid: u32,
        user_sid: &str,
    ) -> Result<(Arc<Mutex<dyn KernelEngineControl>>, watch::Receiver<bool>), String> {
        let client =
            EngineControlGrpcClient::connect(&engine_control_pipe_name(), engine_pid, user_sid)
                .await
                .map_err(|e| format!("engine control connect failed: {e:?}"))?;
        let liveness = client.liveness();
        let engine: Arc<Mutex<dyn KernelEngineControl>> = Arc::new(Mutex::new(client));
        Ok((engine, liveness))
    }
}

/// 崩溃自愈编排（P3）：liveness 触发后执行完整 respawn 链。
///
/// 持有 respawn 所需的全部 seam（supervisor 工厂 / client 连接器 / 残留探测）、共享
/// engine 槽（换点）、共享 composition（身份重建）与 UI peer（gate 重授权身份事实）。
pub struct CrashRecovery {
    /// 新 engine 监督句柄工厂。
    supervisor_factory: Arc<dyn RespawnSupervisorFactory>,
    /// 新 engine 控制面 client 连接器。
    connector: Arc<dyn RespawnClientConnector>,
    /// 崩溃后 0 残留探测（respawn 前硬断言）。
    residue: Arc<dyn ResidueProbe>,
    /// 共享 engine 控制面槽（respawn 换入新 client）。
    slot: EngineSlot,
    /// 共享 host composition（身份重建目标）。
    composition: Arc<Mutex<HostComposition>>,
    /// 已授权的 controller（UI）transport peer（重建后重授权 gate 的身份事实）。
    ui_peer: VerifiedPipePeer,
    /// core 进程用户 SID（engine peer 身份组装与 client 连接）。
    user_sid: String,
}

impl CrashRecovery {
    /// 构造崩溃自愈编排。
    #[must_use]
    pub fn new(
        supervisor_factory: Arc<dyn RespawnSupervisorFactory>,
        connector: Arc<dyn RespawnClientConnector>,
        residue: Arc<dyn ResidueProbe>,
        slot: EngineSlot,
        composition: Arc<Mutex<HostComposition>>,
        ui_peer: VerifiedPipePeer,
        user_sid: String,
    ) -> Self {
        Self {
            supervisor_factory,
            connector,
            residue,
            slot,
            composition,
            ui_peer,
            user_sid,
        }
    }

    /// 执行完整 respawn 链（判据 5）：
    ///
    /// 1. teardown 兜底（`on_helper_link_terminal`——engine 死 ≠ core 死）；
    /// 2. 回收旧 engine 进程句柄（`verify_exit` 有界等待/终止）；
    /// 3. **respawn 前 0 adapter/0 路由硬断言**（`assert_no_residue`）；
    /// 4. 新 `EngineSupervisor`（新提权 + elevated 门禁）；
    /// 5. 新 client（双向认证连接新 engine）；
    /// 6. **composition 身份重建**（engine PID 变 → 旧 peer 绑定失效；重建为全新
    ///    composition——admission 重开、phase Idle——并重授权 gate/actor）；
    /// 7. 共享 engine 槽换入新 client + 新 supervisor 挂接 client。
    ///
    /// `supervisor` 被替换为新监督句柄（旧句柄经 `verify_exit` 回收；respawn 前 0 残留
    /// 断言失败时**不**替换——core 保持旧 supervisor，用户可见报错）。
    ///
    /// # Errors
    /// [`RespawnError`]：残留探测/断言失败、spawn 失败、连接失败、重建失败。任一失败
    /// 都不替换 supervisor（不自动重连，用户再点连接）。
    pub async fn run_respawn(
        &self,
        supervisor: &mut EngineSupervisor,
    ) -> Result<RespawnOutcome, RespawnError> {
        // 1. teardown 兜底（engine 死 ≠ core 死：撤销 admission + packet leg + teardown）。
        self.composition.lock().await.on_helper_link_terminal();

        // 2. 回收旧 engine 进程句柄（有界等待退出/强制终止；engine 不得遗留）。
        let old_pid = supervisor.pid().unwrap_or(0);
        supervisor.verify_exit(RESPAWN_OLD_EXIT_WAIT_MS);

        // 3. respawn 前 0 adapter/0 路由硬断言（判据 4/5）。
        let report = self.residue.probe().map_err(RespawnError::ResidueProbe)?;
        assert_no_residue(&report).map_err(RespawnError::Residue)?;

        // 4. 新 EngineSupervisor（新提权拉起 + 拓扑门禁 elevated 验证）。
        let mut new_supervisor = self
            .supervisor_factory
            .spawn_supervisor()
            .map_err(RespawnError::Spawn)?;
        let new_pid = new_supervisor.pid().unwrap_or(0);

        // 5. 新 client（双向认证 Named Pipe 连接新 engine）。
        let (client, liveness) = self
            .connector
            .connect(new_pid, &self.user_sid)
            .await
            .map_err(RespawnError::Connect)?;

        // 6. composition 身份重建（engine PID 变 → 旧 peer 绑定失效；重建为全新
        //    composition——admission 重开、phase Idle——并重授权 gate/actor）。
        let new_peer = engine_peer_for(new_pid, &self.user_sid);
        self.rebuild_composition(&new_peer)
            .await
            .map_err(RespawnError::Rebuild)?;

        // 7. 共享 engine 槽换入新 client（服务/转发器/KeepAlive 下一轮即指向新 engine）
        //    + 新 supervisor 挂接 client。
        self.slot.swap(client.clone()).await;
        new_supervisor.attach_client(client, liveness);
        *supervisor = new_supervisor;

        Ok(RespawnOutcome::Respawned { old_pid, new_pid })
    }

    /// composition 身份重建：替换为绑定新 engine peer 的全新 composition（admission
    /// 重开、phase Idle、操作登记清空），并重授权 gate（controller transport peer 身份）
    /// + actor 绑定——"回 Idle/报错"的 Idle 侧。
    async fn rebuild_composition(&self, new_peer: &VerifiedPipePeer) -> Result<(), String> {
        rebind_composition(&self.composition, new_peer, &self.ui_peer).await
    }
}

/// composition 身份重建（respawn 与按需 provision 共用的实现）：
///
/// 替换为绑定新 engine peer 的全新 composition（admission 重开、phase Idle、操作登记
/// 清空），并以已验证的 controller（UI）peer 重授权 gate + 重绑 actor。
///
/// # Errors
/// compose 失败 / gate 重授权失败（fail closed——不留下半重建状态，回收决策归调用方）。
pub(crate) async fn rebind_composition(
    composition: &Arc<Mutex<HostComposition>>,
    new_peer: &VerifiedPipePeer,
    ui_peer: &VerifiedPipePeer,
) -> Result<(), String> {
    let mut composition = composition.lock().await;
    let fresh = compose_nonprivileged_host(new_peer)
        .map_err(|e| format!("compose for engine rebind failed: {e:?}"))?;
    *composition = fresh;
    // 重建后的 gate 未授权：用已验证的 controller（UI）peer 重授权（fail closed）。
    composition
        .kernel_gate()
        .authorize(ui_peer)
        .map_err(|e| format!("gate reauthorize after engine rebind failed: {e:?}"))?;
    let (actor_peer, capability) = controller_peer_and_capability();
    composition.bind_controller(actor_peer, capability);
    Ok(())
}

/// 组装 composition 绑定用的 engine peer 身份（PID + 用户 SID + account name）。
/// 与 core bin 的 `engine_peer_for` 同构（lib 侧复用点）。
pub(crate) fn engine_peer_for(pid: u32, user_sid: &str) -> VerifiedPipePeer {
    let account_name = lookup_account_name(user_sid);
    VerifiedPipePeer {
        process_id: pid,
        user_sid: user_sid.to_string(),
        logon_sid: None,
        account_name,
    }
}

// ---------------------------------------------------------------------------
// 单元测试：纯断言 + 编排链（fake seam；无真实进程/提权/Wintun）。
// ---------------------------------------------------------------------------
