
//! engine 侧特权初始化 + teardown（R1b 产品移植，S1.5 三 owner 拆分）。
//!
//! 本模块是 acceptance `scenarios/controlled::helper_apply` / `helper_stop` 的
//! **产品移植**——复用同一组 `exv-vpn-win32-resource` leaf seams（W18 地址 / W19
//! MTU / W20 路由+bypass / W21 DNS），不依赖 acceptance crate（test-only）。
//!
//! **R0 归因铁律（不得带回验收组装工件）**：
//! - `sleep_6s`（controlled.rs:2056）与 `sleep_8s`（:2088）是无条件 `thread::sleep`，
//!   纯验收组装工件——本移植**不带**；
//! - DAD 收敛轮询（:2094-2106，10s deadline）实测 DadState 恒 Tentative、纯烧
//!   budget——本移植**不带**（如需「路由可查」，R1c 用真实路由回读/短沉降替代）。
//! 移除三个等待后 apply ≈ 0.3s（R0 归因 §4）。
//!
//! **W24 路由残留根因修复（保留）**：路由安装后**回读生效行**（Windows 存有效
//! metric，实测 5 -> 10），restore 用回读行做全字段精确 remove——避免请求行全字段
//! 不匹配被 87 拒绝并静默容忍导致路由残留（W24 实测根因）。
//!
//! ## S1.5 三 owner 划界（D17 / D11）
//!
//! 本模块承载 **NIC owner**（adapter 句柄 + session 生命周期）与 **route owner**
//! （路由 + DNS + 地址族 apply，经 NIC 交出的 LUID 驱动）：
//!
//! - [`NicOwner::ensure`]：**首次连接**建 adapter（D12），存续复用；只做 adapter
//!   创建 + 接口级配置（DAD 禁用 / 接口启用）。断开**不拆**（D12）——adapter 由
//!   协调者持有到退出清理阶段才 close。
//! - [`NicOwner::apply_offer`]：在已 ensure 的 adapter 上按真实 CSTP offer 应用四族
//!   网络设置，返回 per-connection 的 [`RouteOwner`] + [`PlatformFacts`]。
//! - [`RouteOwner::clear`]：**减负断开**（D13）清理——逆序移除地址 / 路由 / MTU /
//!   DNS，**保留 adapter**（网卡以无地址惰性存续）。
//! - [`NicOwner`] Drop（adapter creator close）＝**退出清理**移除 adapter，连带四族
//!   配置——0 网卡残留兜底。
//!
//! DP-01 create-then-idle：本模块**不创建 Wintun session、不启动 ring worker**——数据
//! 面由 `crate::data_plane` 在 route 应用后接续（同一 adapter 上 `WintunSession::start`）。

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use exv_vpn_cstp::session::TunnelOffer;
use exv_vpn_win32_resource::dns::{DnsApplier, DnsCapture};
use exv_vpn_win32_resource::dns_types::{DnsFingerprint, DnsSettings};
use exv_vpn_win32_resource::ip_address::{
    IpAddressController, plan_addresses, restore_owned_addresses,
};
use exv_vpn_win32_resource::ip_helper_types::IpAddressRow;
use exv_vpn_win32_resource::mtu::{MtuController, MtuFamily, MtuSnapshot};
use exv_vpn_win32_resource::native_error::NativeError;
use exv_vpn_win32_resource::routes::{self, RouteInstallOutcome, RouteRow};
use exv_vpn_win32_resource::system_proxy_family::SystemProxyFamilyStep;
use exv_vpn_win32_resource::system_proxy_family_exec::{
    SystemProxyDiagnostic, SystemProxyRestoreOutcome, commit_system_proxy_apply,
    plan_system_proxy_apply_observed, restore_system_proxy_step,
};
use exv_vpn_win32_resource::wintun_adapter::WintunAdapter;
use exv_vpn_win32_resource::wintun_api::WintunLibrary;
use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceLuidToGuid, GetIfEntry, GetIpInterfaceEntry, InitializeIpInterfaceEntry,
    MIB_IFROW, MIB_IPINTERFACE_ROW, SetIfEntry, SetIpInterfaceEntry,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::Networking::WinSock::AF_INET;
use windows::core::GUID;

use crate::log_sink::{LogLevel, LogSink};
use crate::system_proxy_journal::SystemProxyJournal;

/// 系统代理观测统一进入产品日志，按成功、正常跳过、降级与清理失败分级。
fn log_system_proxy_stage(log: &LogSink, luid: u64, diagnostic: SystemProxyDiagnostic) {
    let level = match (diagnostic.stage, diagnostic.outcome) {
        ("restore", "failed") => LogLevel::Error,
        (
            _,
            "failed"
            | "pac_degraded"
            | "not_recorded"
            | "third_party_change"
            | "mismatch"
            | "wpad_without_pac_url"
            | "missing_user",
        ) => LogLevel::Warn,
        ("commit", "applied") | ("restore", "restored") => LogLevel::Info,
        _ => LogLevel::Debug,
    };
    let mut fields = vec![
        ("stage", diagnostic.stage.to_owned()),
        ("outcome", diagnostic.outcome.to_owned()),
        ("interface_luid", luid.to_string()),
        ("elapsed_ms", diagnostic.elapsed_ms.to_string()),
    ];
    if let Some((kind, code)) = diagnostic.error {
        fields.push(("error_kind", format!("{kind:?}")));
        fields.push(("native_code", code.to_string()));
    }
    if let Some(reason) = diagnostic.reason {
        fields.push(("reason", reason.to_owned()));
    }
    log.emit(
        level,
        "system-proxy",
        &format!("system-proxy.{}", diagnostic.stage),
        &format!(
            "系统代理阶段：{}，结果：{}",
            diagnostic.stage, diagnostic.outcome
        ),
        &fields
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );
}

fn proxy_diagnostic(
    stage: &'static str,
    outcome: &'static str,
    started: Instant,
    error: Option<&NativeError>,
) -> SystemProxyDiagnostic {
    SystemProxyDiagnostic {
        stage,
        outcome,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error: error.map(|e| (e.kind, e.code)),
        reason: error.and_then(
            exv_vpn_win32_resource::system_proxy_family_exec::safe_system_proxy_error_reason,
        ),
    }
}

/// 隧道接口类型（与 acceptance `TUNNEL_TYPE` 冻结一致）。
const TUNNEL_TYPE: &str = "EXV VPN";
/// 隧道路由安装 metric（与 acceptance 一致，WSP4 实测自动 metric 化）。
const ROUTE_METRIC: u32 = 5;

/// 解析一条路由目标字符串（C++ `parse_destination` 语义对齐）：
/// `"a.b.c.d"`（裸 IP）→ `/32` 主机路由；`"a.b.c.d/n"`（CIDR）→ `(ip, n)`。
/// 非法 IPv4 或 `n > 32` → `None`。
#[must_use]
fn parse_route_destination(route: &str) -> Option<(Ipv4Addr, u8)> {
    let (net, prefix) = match route.split_once('/') {
        Some((net, prefix)) => (net, prefix.parse::<u8>().ok()?),
        None => (route, 32),
    };
    let net: Ipv4Addr = net.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    Some((net, prefix))
}

/// `RouteOwner` 持有的路由清理义务。
#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnedRoute {
    /// Windows 已回读确认的完整生效行；按全字段 compare-and-remove。
    Effective(RouteRow),
    /// Create 已成功但回读与即时回滚均失败；teardown 按稳定身份重新回读再删除。
    Pending(RouteRow),
}

/// 只把本次 `CreateIpForwardEntry2` 真正创建的行交给 `RouteOwner`。
/// 已存在行属于系统/第三方 prestate，本连接只借用，clear/rollback 不得删除。
fn record_owned_route(
    installed_routes: &mut Vec<OwnedRoute>,
    ownership: &mut RouteOwnershipFacts,
    outcome: RouteInstallOutcome,
) {
    match outcome {
        RouteInstallOutcome::Created(actual) => {
            ownership.created += 1;
            installed_routes.push(OwnedRoute::Effective(actual));
        }
        RouteInstallOutcome::CreatedPending {
            submitted,
            readback_error,
            rollback_error,
        } => {
            ownership.pending += 1;
            tracing::warn!(
                readback_code = readback_error.code,
                rollback_code = rollback_error.code,
                "route created but readback and immediate rollback failed; retaining cleanup obligation"
            );
            installed_routes.push(OwnedRoute::Pending(submitted));
        }
        RouteInstallOutcome::Borrowed(_) => {
            ownership.borrowed += 1;
        }
    }
}

/// 记录每条路由的请求身份及真实回读结果，避免失败后只剩一个 API 名称。
fn install_route_logged(
    row: &RouteRow,
    role: &str,
    ifindex: u32,
    log: &crate::log_sink::LogSink,
) -> Result<RouteInstallOutcome, NativeError> {
    log.emit(LogLevel::Debug, "route", "route.install.started", "安装连接路由", &[
        ("role", role), ("network", &format!("{}/{}", row.network, row.prefix_len)),
        ("next_hop", &row.next_hop.to_string()), ("luid", &row.interface_luid.to_string()),
        ("tunnel_ifindex", &ifindex.to_string()), ("metric", &row.metric.to_string()),
    ]);
    let result = routes::install_with_outcome(row);
    match &result {
        Ok(outcome) => {
            let (ownership, actual) = match outcome {
                RouteInstallOutcome::Created(actual) => ("created", actual),
                RouteInstallOutcome::Borrowed(actual) => ("borrowed", actual),
                RouteInstallOutcome::CreatedPending { submitted, .. } => ("pending", submitted),
            };
            log.emit(LogLevel::Debug, "route", "route.install.completed", "路由安装结果", &[
                ("role", role), ("ownership", ownership), ("route", &format!("{actual:?}")),
            ]);
        }
        Err(error) => {
            let observed = routes::capture_rows(row.interface_luid).map(|rows| {
                rows.into_iter().filter(|actual| actual.dest_key() == row.dest_key()).collect::<Vec<_>>()
            });
            log.emit(LogLevel::Error, "route", "route.install.failed", "连接路由配置失败", &[
                ("role", role), ("requested_route", &format!("{row:?}")),
                ("native_code", &error.code.to_string()), ("detail", &error.message),
                ("observed_after_failure", &format!("{observed:?}")),
            ]);
        }
    }
    result
}

fn remove_owned_route(route: &OwnedRoute) -> Result<routes::RemoveOutcome, NativeError> {
    match route {
        OwnedRoute::Effective(row) => routes::remove(row),
        OwnedRoute::Pending(submitted) => routes::remove_created_pending(submitted),
    }
}

fn retry_owned_routes_with(
    owned: &mut Vec<OwnedRoute>,
    mut remove: impl FnMut(&OwnedRoute) -> Result<routes::RemoveOutcome, NativeError>,
) -> Result<(), String> {
    let mut failed = Vec::new();
    let mut first_error = None;
    for route in std::mem::take(owned).into_iter().rev() {
        match remove(&route) {
            Ok(_) => {}
            // 精确行被第三方修改后拒绝删除：保持既有 typed-skip 语义，不再声称拥有。
            Err(error) if error.code == 87 => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(format!("route-remove:{error:?}"));
                }
                failed.push(route);
            }
        }
    }
    // 上面按逆安装序执行；容器仍保存原安装序，下一次 retry 再逆序清理。
    failed.reverse();
    *owned = failed;
    match first_error {
        Some(error) => Err(format!("{error}; pending_routes={}", owned.len())),
        None => Ok(()),
    }
}

/// apply/rollback 后仍需由运行时持有并重试的外部路由清理义务。
///
/// 容器不实现 `Clone`：同一 created 路由只能有一个逻辑 owner。每次重试会移除已经
/// 成功清理、已经 absent 或发生第三方变更的项，只保留本次仍硬失败的行。
#[derive(Debug, Default)]
pub struct RouteCleanupObligations {
    routes: Vec<OwnedRoute>,
}

impl RouteCleanupObligations {
    fn from_routes(routes: Vec<OwnedRoute>) -> Self {
        Self { routes }
    }

    /// 是否已没有待清理路由。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// 当前仍待清理的 created 路由数，供产品诊断记录。
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// 逆安装序重试清理；硬失败项继续留在本容器中，调用方不得在错误后丢弃容器。
    pub fn retry_clear(&mut self) -> Result<(), String> {
        retry_owned_routes_with(&mut self.routes, remove_owned_route)
    }

    /// 接管另一批清理义务；用于运行时把连续失败产生的 owner 汇入同一 holder。
    pub fn append(&mut self, mut other: Self) {
        self.routes.append(&mut other.routes);
    }
}

/// platform apply 失败详情，以及回滚硬失败后必须由运行时继续持有的路由义务。
#[derive(Debug)]
pub struct RouteApplyError {
    /// 原有可读错误详情。
    pub detail: String,
    /// 本次 apply 真正创建、但回滚仍未清除的路由。
    pub pending_routes: RouteCleanupObligations,
    /// 路由 API 的原始系统码；供运行时传给前端，不从诊断字符串反向解析。
    pub route_native_code: Option<u32>,
}

impl RouteApplyError {
    fn with_pending(detail: impl Into<String>, pending_routes: RouteCleanupObligations) -> Self {
        Self {
            detail: detail.into(),
            pending_routes,
            route_native_code: None,
        }
    }
}

impl std::fmt::Display for RouteApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for RouteApplyError {}

impl From<String> for RouteApplyError {
    fn from(detail: String) -> Self {
        Self::with_pending(detail, RouteCleanupObligations::default())
    }
}

impl From<&str> for RouteApplyError {
    fn from(detail: &str) -> Self {
        Self::from(detail.to_owned())
    }
}

/// LUID → interface GUID（DNS API 键；WSP4 冻结转换）。resource 内 `luid_to_guid`
/// 是 `pub(crate)`，engine 侧复刻同一 5 行 Win32 调用（不 fork 语义）。
#[must_use]
fn luid_to_guid(luid: u64) -> Option<GUID> {
    let l = NET_LUID_LH { Value: luid };
    let mut guid = GUID::zeroed();
    // SAFETY: guid 由系统填充（ConvertInterfaceLuidToGuid 成功即有效 GUID）。
    if unsafe { ConvertInterfaceLuidToGuid(&raw const l, &raw mut guid) }.0 != 0 {
        return None;
    }
    Some(guid)
}

/// 禁用 IPv4 接口行的 DAD（`DadTransmits = 0`），使隧道地址立即进入 Preferred。
///
/// **R0 归因根因**（native-network-settings-facts / R0 §2.1）：Wintun 接口的 IPv4 地址
/// `DadState` 在 10s+ 内恒为 `Tentative`、从不收敛到 `Preferred`——Tentative 地址不能
/// 作源地址，业务流量（ping/SSH）无法发出。隧道适配器无重复地址风险，DAD 无意义且
/// 有害；标准解法是 `SetIpInterfaceEntry(DadTransmits=0)`（VPN 隧道接口通例）。
///
/// 在**地址 apply 之前**调用（DAD 参数是接口级配置，须先于地址创建生效）。接口级
/// 配置跨连接存续（S1.5：DAD 禁用随 adapter 建一次，重连不重复设）。
///
/// # Errors
///
/// `GetIpInterfaceEntry` / `SetIpInterfaceEntry` 失败 → `String`。
fn disable_ipv4_dad(luid: u64) -> Result<(), String> {
    let mut row = MIB_IPINTERFACE_ROW::default();
    // SAFETY: Initialize 写入行（文档模式：Family + InterfaceLuid + Get 填满）。
    unsafe { InitializeIpInterfaceEntry(std::ptr::addr_of_mut!(row)) };
    row.Family = AF_INET;
    // SAFETY: NET_LUID_LH 是 union；写成员是安全的。
    row.InterfaceLuid = NET_LUID_LH { Value: luid };
    // SAFETY: row 的 Family+Luid 有效；Get 填满行，返回 WIN32_ERROR（0 = 成功）。
    let rc = unsafe { GetIpInterfaceEntry(std::ptr::addr_of_mut!(row)).0 };
    if rc != 0 {
        return Err(format!("GetIpInterfaceEntry (dad): {rc}"));
    }
    // WSP4 冻结前提：Get 填充的 IPv4 行含 `SitePrefixLength=64`，直接 Set 会被 87 拒。
    row.SitePrefixLength = 0;
    row.DadTransmits = 0;
    // SAFETY: row 已 Get 填满 + 修正前提；Set 写回 DAD 参数（API 接受 *mut，读路径
    // 无并发写者，调用线程独占该行）。
    let rc = unsafe { SetIpInterfaceEntry(&raw mut row).0 };
    if rc != 0 {
        return Err(format!("SetIpInterfaceEntry (dad): {rc}"));
    }
    Ok(())
}

/// 接口启用（IpHelper `SetIfEntry`，生产 API——非 netsh；best-effort：顺序与
/// WSP3/W17 冻结路径一致：address → enable → route（路由安装前启用））。
fn enable_interface_best_effort(ifindex: u32) -> Result<(), String> {
    let mut row = MIB_IFROW {
        dwIndex: ifindex,
        ..Default::default()
    };
    // SAFETY: row 是有效输出参数；GetIfEntry 填充接口行。
    let rc = unsafe { GetIfEntry(&raw mut row) };
    if rc != 0 {
        return Err(format!("GetIfEntry failed: {rc}"));
    }
    row.dwAdminStatus = 1; // MIB_IF_ADMIN_STATUS_UP
    // SAFETY: row 已由 GetIfEntry 填充（身份字段一致），SetIfEntry 写回。
    let rc = unsafe { SetIfEntry(&raw const row) };
    if rc != 0 {
        return Err(format!("SetIfEntry failed: {rc}"));
    }
    Ok(())
}

/// 特权初始化后的 adapter/网络事实（供日志/证据；ApplyAccepted 不经此回包——R1w
/// pending 不携带事实）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlatformFacts {
    /// 已应用的接口地址 `address/prefix`。
    pub address_applied: Option<String>,
    /// 已应用的 MTU 值。
    pub mtu_applied: Option<u32>,
    /// 本次计划中的路由（隧道路由 + 校园路由）；实际所有权/观察结果见
    /// [`Self::route_ownership`]。
    pub routes_applied: Vec<String>,
    /// 每条路由安装的所有权结果；供产品日志区分本次创建、借用既有行与待清理行。
    pub route_ownership: RouteOwnershipFacts,
    /// 已应用的 DNS 服务器。
    pub dns_applied: Vec<String>,
    /// adapter LUID。
    pub luid: u64,
    /// adapter 接口 index。
    pub ifindex: u32,
}

/// 一次 platform apply 的互斥路由所有权结果计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteOwnershipFacts {
    /// Create 成功且回读确认、由本连接持有的行。
    pub created: usize,
    /// Create 报已存在、由本连接借用且断开时不得删除的行。
    pub borrowed: usize,
    /// Create 成功，但直接回读、即时回滚与替代观察都未能收敛；仍由本连接持有清理义务。
    pub pending: usize,
}

/// **NIC owner（D11/D17）**：创建者 adapter 句柄 + WintunLibrary + 名称，跨连接存续。
///
/// S1.5 生命周期（D12）：`NicOwner::ensure` **首次连接**建 adapter；断开**不拆**
/// （网卡以无地址惰性存续）；退出清理阶段由协调者 close（Drop = adapter creator
/// close 移除 adapter）。
///
/// **字段声明顺序即 Drop 顺序（W17 SAFETY-ORDER + 资源逆序）**：先 drop `adapter`
/// （调用仍位于已加载模块内的 `WintunCloseAdapter`），再 drop `_lib`（释放 DLL 加载）。
/// 保存函数指针不保持模块映射，因此该顺序属于安全性约束。
pub struct NicOwner {
    /// 创建者 adapter 句柄（drop = creator close 移除 adapter）。
    adapter: WintunAdapter,
    /// 保持 Wintun DLL 加载直到 adapter 完成 close；`_lib` 在 `adapter` 之后 drop。
    _lib: WintunLibrary,
    /// adapter 名称（create 名；宿主按此名可观测）。
    adapter_name: String,
}

/// 使用已加载 Wintun 模块初始化 NIC 时的失败边界。
///
/// adapter 创建及身份解析保留原始 Win32 错误，供上层生成可操作的依赖错误；创建后
/// 的接口配置失败保持现有字符串语义，由上层按平台配置失败处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NicInitializationError {
    /// `WintunCreateAdapter` 或 adapter 身份解析返回的原始系统错误。
    Native(NativeError),
    /// adapter 创建后的接口配置失败。
    Configuration(String),
}

impl NicInitializationError {
    /// `NicOwner::ensure` 的既有 `String` 契约；新调用者应直接匹配 typed 变体。
    fn into_legacy_string(self) -> String {
        match self {
            Self::Native(error) => format!("wintun-create:{error:?}"),
            Self::Configuration(detail) => detail,
        }
    }
}

// SAFETY: `NicOwner` 持有 `WintunAdapter`（HANDLE + close export 副本）与
// `WintunLibrary`（HMODULE + exports 函数指针副本）——两者自身非 `Send`（原始
// HANDLE 指针）。`unsafe impl Send` 的论证镜像旧 `PlatformTunnel` 的既存 impl：
// 句柄值只经方法（`&self`/`&mut self`）访问、`Drop`（adapter creator close / lib
// free）恰好执行一次，且本类型恒处于协调者的 `Mutex` 串行访问下（
// `tunnel_runtime::RealTunnelRuntime.nic`）。移动句柄值跨线程安全——底层资源属
// 内核/系统，与线程无关。
unsafe impl Send for NicOwner {}

impl NicOwner {
    /// **首次连接建 adapter（D12）**：`WintunLibrary::load` + `WintunAdapter::create`
    /// + 接口级配置（DAD 禁用 + 接口启用，均跨连接存续）。重复调用由协调者保证
    /// 不触发（`nic` 已存在则复用）。
    ///
    /// # Errors
    ///
    /// lib 加载 / adapter 创建 / DAD 设置失败 → typed `String`。
    pub fn ensure(dll: &Path, adapter_name: &str) -> Result<Self, String> {
        let lib = WintunLibrary::load(dll).map_err(|e| format!("wintun-load:{e:?}"))?;
        Self::ensure_with_library(lib, adapter_name)
            .map_err(NicInitializationError::into_legacy_string)
    }

    /// 使用登录前已经校验并加载的模块，避免认证后再次依赖失效的 DLL 路径，并保留
    /// adapter 创建失败的原始 Win32 错误。
    pub fn ensure_with_library(
        lib: WintunLibrary,
        adapter_name: &str,
    ) -> Result<Self, NicInitializationError> {
        let (adapter, _open) = WintunAdapter::create(&lib, adapter_name, TUNNEL_TYPE)
            .map_err(NicInitializationError::Native)?;
        let luid = adapter.luid();
        // 禁用 DAD（地址 apply 之前；R0 归因根因——Wintun IPv4 地址恒 Tentative）。
        // 接口级配置：建一次，跨连接存续。
        disable_ipv4_dad(luid).map_err(NicInitializationError::Configuration)?;
        // 接口启用（在隧道路由安装之前——WSP3/W17 冻结顺序：address → enable → route）。
        let _ = enable_interface_best_effort(adapter.ifindex());
        Ok(Self {
            adapter,
            _lib: lib,
            adapter_name: adapter_name.to_string(),
        })
    }

    /// 接口 LUID（route owner 驱动 + 数据面接续用）。
    #[must_use]
    pub fn luid(&self) -> u64 {
        self.adapter.luid()
    }

    /// 接口 index（证据）。
    #[must_use]
    pub fn ifindex(&self) -> u32 {
        self.adapter.ifindex()
    }

    /// adapter 创建者句柄引用（`data_plane::EngineDataPlane::start` 直接在该句柄上
    /// `WintunSession::start`，不再 open-by-name）。
    #[must_use]
    pub fn adapter(&self) -> &WintunAdapter {
        &self.adapter
    }

    /// WintunLibrary 引用（`WintunSession::start` 需要）。
    #[must_use]
    pub fn library(&self) -> &WintunLibrary {
        &self._lib
    }

    /// adapter 名称（create 名；宿主按此名可观测）。
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// 在已 ensure 的 adapter 上应用一次连接的 offer（地址/DNS/路由，经 LUID 驱动）
    /// ——**route owner** 的 per-connection 状态生产。
    ///
    /// `offer` 是**真实 CSTP 协商**的隧道计划；`campus_routes` 是 config 驱动的校园
    /// 路由。返回 [`RouteOwner`]（per-connection 应用态；`clear` 供减负断开清理）+
    /// [`PlatformFacts`]（日志/证据）。
    ///
    /// # Errors
    ///
    /// 任一 leaf 失败 → [`RouteApplyError`]；[`RouteOwner::apply`] 内部逆序回滚，
    /// 路由删除硬失败时由错误携带 [`RouteCleanupObligations`] 交给运行时继续持有。
    /// 应用真实 CSTP offer 的四族网络状态 + **网关 bypass /32 族**（2026-09-08
    /// 计划 T2）+ **系统代理豁免族**（设计 §5.3）。
    ///
    /// `bypass` 是由 VGDC 解析出的网关地址 + 物理出口发现结果构造的 `/32` 精确行
    /// （[`exv_vpn_win32_resource::routes::gateway_bypass_row`]；engine 装配点构造）。
    /// `Some` 时先于全部隧道路由安装（I1），`clear` 逆序**最后**移除（I4）；
    /// `None`（无 bypass，兼容既有编排）时零动作。
    ///
    /// `core_user_sid` 为发起连接的用户 SID：`Some` 时 RouteOwner 额外执行
    /// 系统代理 family（豁免条目由校园路由派生，写入该用户 HKU 的 ProxyOverride；
    /// apply 记账/清账经 `journal` 账本——崩溃恢复的持久点）；`None`（engine 未知
    /// 发起用户）→ 跳过该族（fail-closed 不猜测写谁）。
    ///
    /// # Errors
    ///
    /// 任一 leaf 失败 → [`RouteApplyError`]；[`RouteOwner::apply`] 内部逆序回滚，
    /// 路由删除硬失败时由错误携带 [`RouteCleanupObligations`] 交给运行时继续持有。
    pub fn apply_offer(
        &self,
        offer: &TunnelOffer,
        campus_routes: &[String],
        bypass: Option<&exv_vpn_win32_resource::routes::RouteRow>,
        core_user_sid: Option<String>,
        journal: &SystemProxyJournal,
    ) -> Result<(RouteOwner, PlatformFacts), RouteApplyError> {
        // 接口启用（路由安装前；WSP3 顺序 address → enable → route）。接口级配置
        // 在 ensure 时已启用，但地址 apply 后再次启用是既有 W17 路径，保持幂等。
        let _ = enable_interface_best_effort(self.adapter.ifindex());
        RouteOwner::apply(
            self.luid(),
            self.adapter.ifindex(),
            offer,
            campus_routes,
            bypass,
            core_user_sid,
            journal,
        )
    }
}

/// **Route owner（D11/D17）**：per-connection 已应用的四族网络状态（地址/MTU/路由/
/// DNS），经 NIC 交出的 LUID 驱动。
///
/// 生命周期：每次连接经 [`RouteOwner::apply`] 创建；**减负断开**（D13）经
/// [`RouteOwner::clear`] 逆序清理（保留 adapter）；退出清理随协调者丢弃（Drop 不
/// 额外动作——地址/路由已由 clear 移除，adapter 由 NicOwner close）。
pub struct RouteOwner {
    /// 目标 LUID（地址/路由控制器键）。
    luid: u64,
    /// 本 apply 创建的地址行（clear 精确删除；pre-existing 永不 owned）。
    owned_addresses: Vec<IpAddressRow>,
    /// 本 apply 真正创建的路由清理义务；既有 borrowed 行永不进入本表。
    installed_routes: Vec<OwnedRoute>,
    /// MTU `(applied, original)`（clear 输入；V4 族）。
    mtu_applied: Option<MtuSnapshot>,
    mtu_original: Option<MtuSnapshot>,
    /// DNS applied fingerprint + 原始快照（GUID-keyed）。
    dns_fingerprint: DnsFingerprint,
    dns_original: DnsSettings,
    dns_guid: GUID,
    /// 系统代理豁免族已生效状态（设计 §5.3；`None` = 未生效——SID 缺失 /
    /// 无系统代理 / PAC fail-closed / 记账或 commit 失败降级）。
    system_proxy: Option<SystemProxyApplied>,
    /// 系统代理账本句柄（与运行时共享同一 store；clear 清账用。禁用态 no-op）。
    journal: SystemProxyJournal,
}

/// 系统代理豁免族已应用状态（clear 的 compare-and-restore 输入）。
///
/// v0x03 步骤记录自带写入后指纹（`step.written_fingerprint`）——不再重复存指纹字段。
struct SystemProxyApplied {
    /// 本连接的步骤记录（prestate + desired + SID + 写入后指纹；journal payload 同源）。
    step: SystemProxyFamilyStep,
    /// PAC 开环包装的 loopback 端点（`Some` = Automatic 模式已改指 AutoConfigURL；
    /// teardown 还原注册表后 drop 关闭，遵循 §5.6 零扰动顺序。Arc 共享：commit 按值
    /// 消费计划后调用方仍持有 teardown 所有权）。
    pac_endpoint: Option<Arc<exv_vpn_win32_resource::system_proxy_pac::PacEndpoint>>,
    /// 本步骤追加前 journal 的记录数（clear 成功后 `clear_tail(keep)` 的前缀基准——
    /// 只丢自己的严格后缀，§4.4）。
    journal_len_before: usize,
}

/// 记账先于效果（WJ-1，§5.2 测试 12 的可注入 seam）：`append` 成功才调用 `commit`；
/// 记账失败（false）⇒ **commit 不被调用**，返回 [`RecordCommitOutcome::NotRecorded`]。
///
/// Manual 与 PAC 两态共用同一裁决路径——无持久意图则无效果（§4.2 J50-WAL）。
fn record_then_commit(
    append_ok: bool,
    commit: impl FnOnce() -> Result<(), NativeError>,
) -> RecordCommitOutcome {
    if !append_ok {
        return RecordCommitOutcome::NotRecorded;
    }
    match commit() {
        Ok(()) => RecordCommitOutcome::Committed,
        Err(e) => RecordCommitOutcome::CommitFailed(e),
    }
}

/// [`record_then_commit`] 的结果（apply 侧降级分派）。
enum RecordCommitOutcome {
    /// 记账 + commit 均成功 → 系统代理族生效，clear 时负责还原与清账。
    Committed,
    /// 记账失败 → 降级「未豁免」，commit 未被调用（连接继续，best-effort）。
    NotRecorded,
    /// 记账成功但 commit 硬失败 → 降级（注册表零写入或写入失败；账本记录保留，
    /// 下次回放按 AlreadyClean/SkipForfeit 自然收敛）。
    CommitFailed(NativeError),
}

impl RouteOwner {
    /// 五族 leaf apply（bypass /32（`Option`，物理出口）→ address → MTU →
    /// 隧道路由 + 校园路由 → DNS，admission-first）。
    ///
    /// **all-or-nothing（D16）**：任一 leaf 失败 → 内部逆序回滚已生效族（DNS → 路由
    /// （含 bypass 行）→ MTU → 地址）。地址族 `plan_addresses` 只 apply 新增行
    /// （pre-existing 不 owned）；路由族安装后回读生效行（W24）。路由回滚若硬失败，
    /// 错误会携带剩余 cleanup owner，避免在返回路径遗失本次 created 行。
    ///
    /// # Errors
    ///
    /// 任一 leaf 失败（admission-first 拒绝 / Win32 调用失败）→ [`RouteApplyError`]；
    /// 已完成可完成的回滚，未清除路由由 `pending_routes` 明确交给调用方。
    fn apply(
        luid: u64,
        ifindex: u32,
        offer: &TunnelOffer,
        campus_routes: &[String],
        bypass: Option<&RouteRow>,
        core_user_sid: Option<String>,
        journal: &SystemProxyJournal,
    ) -> Result<(Self, PlatformFacts), RouteApplyError> {
        // ---- admission-first：plan 整体校验（W22 语义，零效果拒绝）。 ----
        let address: Ipv4Addr = offer.ipv4_address;
        let prefix: u8 = offer.prefix;
        if prefix > 32 {
            return Err("invalid prefix".into());
        }
        let mtu = u32::from(offer.mtu);
        for route in offer.routes.iter().chain(campus_routes.iter()) {
            parse_route_destination(route).ok_or("route shape")?;
        }
        for ns in &offer.dns_servers {
            let _: Ipv4Addr = *ns;
        }

        // 局部可回滚状态（失败逆序恢复）。
        let mut owned_addresses: Vec<IpAddressRow> = Vec::new();
        let mut installed_routes: Vec<OwnedRoute> = Vec::new();
        let mut route_ownership = RouteOwnershipFacts::default();
        let mut mtu_state: Option<(MtuSnapshot, MtuSnapshot)> = None;
        let mut dns_state: Option<(DnsFingerprint, DnsSettings, GUID)> = None;
        let mut system_proxy_state: Option<SystemProxyApplied> = None;
        let mut route_native_code = None;
        let route_log = journal.log();

        let apply = (|| -> Result<(), String> {
            // ---- address 族（W18 leaf：pre-existing 不 owned；owned 行记录供 clear）。 ----
            let controller = IpAddressController::new(luid);
            let captured_addr = controller
                .capture()
                .map_err(|e| format!("addr-capture:{e:?}"))?;
            let addr_row = IpAddressRow::new(address, luid, prefix);
            let addr_plan = plan_addresses(&captured_addr, &[addr_row]);
            for row in &addr_plan.to_add {
                controller
                    .apply(row)
                    .map_err(|e| format!("address-apply:{e:?}"))?;
            }
            owned_addresses = addr_plan.to_add.clone();

            // 接口启用（路由安装前；WSP3 顺序 address → enable → route）。
            let _ = enable_interface_best_effort(ifindex);

            // ---- MTU 族（W19 leaf：Set 前原始快照；clear 输入）。 ----
            let mtu_ctrl = MtuController::new(luid, MtuFamily::V4);
            let mtu_original = mtu_ctrl
                .capture()
                .map_err(|e| format!("mtu-capture:{e:?}"))?;
            mtu_ctrl
                .apply(mtu)
                .map_err(|e| format!("mtu-apply:{e:?}"))?;
            let mtu_applied = MtuSnapshot::new(luid, MtuFamily::V4, mtu);
            mtu_state = Some((mtu_applied, mtu_original));

            // ---- 网关 bypass /32 族（2026-09-08 计划 T2）：VGDC 解析地址驱动的
            //      物理出口精确行，先于全部隧道路由入表（I1）——网关所在的校园
            //      网段随后整段进隧道，路由表层对网关的选路由本 /32 覆盖到物理
            //      出口（第二层防御；第一层是 socket binder）。行构造不经无源
            //      GetBestRoute2（上游 TUN 默认路由在表时会捕获 TUN——facts §3
            //      增补，2026-09-08）。本次真正创建的行 push 首位 ⇒ clear 逆序
            //      **最后**移除、失败回滚最后撤（I4）；pre-existing 行只借用，不纳入
            //      owner。 ----
            if let Some(bypass) = bypass {
                // 不再按 destination 跨接口清扫：同 gateway /32 可能属于系统、上游
                // TUN 或其它 VPN。Create 成功才取得所有权；已存在精确行只借用。
                let outcome = install_route_logged(bypass, "gateway_bypass", ifindex, &route_log)
                    .map_err(|e| {
                        route_native_code = Some(e.code);
                        format!("bypass-install:{e:?}")
                    })?;
                record_owned_route(&mut installed_routes, &mut route_ownership, outcome);
            }

            // ---- 隧道路由族 + 校园路由族（W20 leaf：精确行安装，回读生效行，逆序清理）。 ----
            for route in offer.routes.iter().chain(campus_routes.iter()) {
                let (net, prefix) = parse_route_destination(route).ok_or("route shape")?;
                // Wintun 是直连接口。把本机 VPN 地址当 next-hop 时，Windows 会将
                // 入表行规范为 0.0.0.0；再用本机地址精确 Get 会返回 1168。提交、回读、
                // 清理必须使用同一真实身份；外层 gateway bypass 仍保留其真实下一跳。
                let row = RouteRow::new(net, prefix, Ipv4Addr::UNSPECIFIED, luid, ROUTE_METRIC);
                // W24 + ownership：API 返回回读生效行；只有本次真正创建的行进入
                // installed_routes。既有校园路由只借用，断开不得删除。
                let outcome = install_route_logged(&row, "tunnel", ifindex, &route_log)
                    .map_err(|e| {
                        route_native_code = Some(e.code);
                        format!("route-install:{e:?}")
                    })?;
                record_owned_route(&mut installed_routes, &mut route_ownership, outcome);
            }

            // ---- DNS 族（W21 leaf：applied fingerprint + 原始快照）。 ----
            let guid = luid_to_guid(luid).ok_or("luid-to-guid:failed")?;
            let dns_original =
                DnsCapture::capture(&guid).map_err(|e| format!("dns-capture:{e:?}"))?;
            let dns_settings = DnsSettings::new(
                offer.dns_servers.iter().map(ToString::to_string).collect(),
                Vec::new(),
            );
            let dns_fingerprint =
                DnsApplier::apply(&guid, &dns_settings).map_err(|e| format!("dns-apply:{e:?}"))?;
            dns_state = Some((dns_fingerprint, dns_original, guid));

            // ---- 系统代理豁免族（设计 §5.3 + 2026-09-05 账本计划 §4.2 J50-WAL）。
            //      豁免条目 = 校园路由按拍板规则派生（IP 精确直通 + /8-/16-/24 整段
            //      转通配）。执行序（冻结）：内联回放清活跃遗留 → plan（注册表零
            //      写入）→ journal append（持久点）→ commit（效果点）。best-effort：
            //      记账失败降级「未豁免」不 commit、commit 失败降级不断连接——附加
            //      功能不得阻断连接（业务流优先）。
            if let Some(sid) = &core_user_sid {
                let proxy_log = journal.log();
                let observe = |event| log_system_proxy_stage(&proxy_log, luid, event);
                let proxy_started = Instant::now();
                let desired: Vec<String> = campus_routes
                    .iter()
                    .filter_map(|r| {
                        exv_vpn_win32_resource::system_proxy_override::cidr_to_wildcard_opt(r)
                            .map(|e| e.as_str().to_owned())
                    })
                    .collect();
                proxy_log.emit(
                    LogLevel::Debug,
                    "system-proxy",
                    "system-proxy.bypass-input",
                    "系统代理豁免输入已派生",
                    &[
                        ("ifindex", &ifindex.to_string()),
                        ("campus_route_count", &campus_routes.len().to_string()),
                        ("desired_entry_count", &desired.len().to_string()),
                        (
                            "unrepresentable_route_count",
                            &campus_routes
                                .len()
                                .saturating_sub(desired.len())
                                .to_string(),
                        ),
                    ],
                );
                // apply 前置不变量（§4.2）：journal 仍存在活跃步骤记录（如上次清除
                // compact 失败遗留）→ 先内联回放清除，保证「记录中的 prestate 恒为
                // 真实注册表状态」。失败只记日志，不阻断连接。
                journal.replay_pending(&journal.log());
                observe(proxy_diagnostic("replay", "checked", proxy_started, None));
                // 本步骤追加前的记录数 = clear 时前缀清除基准（只丢自己的严格后缀）。
                let journal_len_before = journal.record_count();
                match plan_system_proxy_apply_observed(sid, &desired, &observe) {
                    Ok(plan) if plan.registry_effect => {
                        let step = plan.step.clone();
                        let pac_endpoint = plan.endpoint.clone();
                        // WJ-1 记账先于效果：append 失败 ⇒ commit 不被调用（降级
                        // 「未豁免」；PAC 端点随 plan drop 关闭，注册表零写入）。
                        let commit_started = Instant::now();
                        match record_then_commit(journal.append_step(&step), || {
                            commit_system_proxy_apply(sid, plan)
                        }) {
                            RecordCommitOutcome::Committed => {
                                observe(proxy_diagnostic(
                                    "commit",
                                    "applied",
                                    commit_started,
                                    None,
                                ));
                                // 回读仅作观测：验证效果，不改变既有应用或恢复决策。
                                let readback_started = Instant::now();
                                match exv_vpn_win32_resource::system_proxy::capture_for_user(sid) {
                                    Ok(current) => {
                                        let matched = exv_vpn_win32_resource::system_proxy_family_exec::fingerprint_prestate(&current) == step.written_fingerprint;
                                        observe(proxy_diagnostic(
                                            "readback",
                                            if matched { "matched" } else { "mismatch" },
                                            readback_started,
                                            None,
                                        ));
                                    }
                                    Err(e) => observe(proxy_diagnostic(
                                        "readback",
                                        "failed",
                                        readback_started,
                                        Some(&e),
                                    )),
                                }
                                system_proxy_state = Some(SystemProxyApplied {
                                    step,
                                    pac_endpoint,
                                    journal_len_before,
                                });
                            }
                            RecordCommitOutcome::NotRecorded => {
                                observe(proxy_diagnostic(
                                    "commit",
                                    "not_recorded",
                                    commit_started,
                                    None,
                                ));
                            }
                            RecordCommitOutcome::CommitFailed(e) => {
                                observe(proxy_diagnostic(
                                    "commit",
                                    "failed",
                                    commit_started,
                                    Some(&e),
                                ));
                            }
                        }
                    }
                    Ok(_) => {} // 零效果（Disabled/纯 WPAD/merged==原值/PAC fail-closed）：零动作零账本。
                    Err(e) => {
                        observe(proxy_diagnostic("plan", "failed", proxy_started, Some(&e)));
                    }
                }
            } else {
                log_system_proxy_stage(
                    &journal.log(),
                    luid,
                    proxy_diagnostic("plan", "missing_user", Instant::now(), None),
                );
            }
            Ok(())
        })();

        if let Err(e) = apply {
            // all-or-nothing 回滚（D16）：逆序恢复已生效族（DNS → 路由 → MTU → 地址）。
            if let Some((fingerprint, original, guid)) = dns_state.take() {
                let _ = DnsApplier::restore(&guid, &fingerprint, &original);
            }
            let mut pending_routes =
                RouteCleanupObligations::from_routes(std::mem::take(&mut installed_routes));
            if let Err(remove_error) = pending_routes.retry_clear() {
                tracing::warn!(
                    detail = %remove_error,
                    pending_routes = pending_routes.len(),
                    "route rollback remove failed; transferring cleanup obligation"
                );
            }
            if let Some((applied, original)) = mtu_state.take() {
                let ctrl = MtuController::new(applied.luid, applied.family);
                let _ = ctrl.compare_and_restore(&applied, &original);
            }
            let controller = IpAddressController::new(luid);
            if let Ok(current) = controller.capture() {
                for row in restore_owned_addresses(&owned_addresses, &current) {
                    let _ = controller.delete(&row);
                }
            }
            let mut error = RouteApplyError::with_pending(e, pending_routes);
            error.route_native_code = route_native_code;
            return Err(error);
        }

        let mut routes_applied: Vec<String> = offer.routes.clone();
        routes_applied.extend_from_slice(campus_routes);
        let facts = PlatformFacts {
            address_applied: Some(format!("{address}/{prefix}")),
            mtu_applied: Some(mtu),
            routes_applied,
            route_ownership,
            dns_applied: offer.dns_servers.iter().map(ToString::to_string).collect(),
            luid,
            ifindex,
        };
        let (fingerprint, original, guid) = dns_state.expect("dns applied (apply ok)");
        Ok((
            Self {
                luid,
                owned_addresses,
                installed_routes,
                mtu_applied: mtu_state.as_ref().map(|(a, _)| a.clone()),
                mtu_original: mtu_state.as_ref().map(|(_, o)| o.clone()),
                dns_fingerprint: fingerprint,
                dns_original: original,
                dns_guid: guid,
                system_proxy: system_proxy_state,
                journal: journal.clone(),
            },
            facts,
        ))
    }

    /// **减负断开清理（D13）**：逆序 compare-and-restore + 移除地址，**保留 adapter**
    /// （网卡以无地址惰性存续，D12）。
    ///
    /// 顺序 = 逆序：DNS → 路由（逆序精确 remove）→ MTU → 地址（owned 行 compare-delete）。
    /// 第三方变更 typed skip（不覆盖）。
    ///
    /// # Errors
    ///
    /// 任一 leaf restore 硬失败 → typed `String`（绝不静默）；调用方应据错误决定兜底
    /// （adapter 保留，退出清理仍会 close）。
    pub fn clear(&mut self) -> Result<(), String> {
        // DNS 逆序。
        DnsApplier::restore(&self.dns_guid, &self.dns_fingerprint, &self.dns_original)
            .map_err(|e| format!("dns-restore:{e:?}"))?;
        // 系统代理族逆序（设计 §5.3：DNS 先撤、system_proxy 在路由/bypass 之前撤）。
        // 两态 compare-and-restore（§2 表）：指纹等 → 精确还原 / 不等 → typed skip
        // （不覆盖）。还原成功或 typed skip 后清账（compact 自己的严格后缀）；
        // 硬失败保留记录并照旧传播 Err。
        if let Some(applied) = self.system_proxy.take() {
            let started = Instant::now();
            let proxy_log = self.journal.log();
            log_system_proxy_stage(
                &proxy_log,
                self.luid,
                proxy_diagnostic("restore", "started", started, None),
            );
            match restore_system_proxy_step(
                &applied.step.originating_sid,
                &applied.step,
                &applied.step.written_fingerprint,
            ) {
                Ok(outcome) => {
                    let result = match outcome {
                        SystemProxyRestoreOutcome::TypedSkip => "third_party_change",
                        SystemProxyRestoreOutcome::Restored { .. } => "restored",
                    };
                    log_system_proxy_stage(
                        &proxy_log,
                        self.luid,
                        proxy_diagnostic("restore", result, started, None),
                    );
                    // §4.4：清账 = 前缀 compact（keep = apply 前记录数，只丢自己的
                    // 严格后缀）。compact 失败只记日志——记录遗留，下次启动回放的
                    // AlreadyClean 路径自然清除（自愈）。
                    self.journal.clear_tail(applied.journal_len_before);
                }
                Err(e) => {
                    log_system_proxy_stage(
                        &proxy_log,
                        self.luid,
                        proxy_diagnostic("restore", "failed", started, Some(&e)),
                    );
                    // restore 未完成时仍保留完整 state（含 PAC endpoint 与账本基准），
                    // 让持有本 RouteOwner 的调用方稍后可以重试；不能因 take 丢 owner。
                    self.system_proxy = Some(applied);
                    return Err(format!("system-proxy-restore:{e:?}"));
                }
            }
            // §5.6 零扰动顺序：先还原注册表并广播，端点随后 drop 关闭（Drop 即关
            // listener；浏览器已缓存包装脚本，短期重取失败回退直连，属降级不断网）。
            if let Some(endpoint) = applied.pac_endpoint {
                let release_started = Instant::now();
                drop(endpoint);
                log_system_proxy_stage(
                    &proxy_log,
                    self.luid,
                    proxy_diagnostic("endpoint", "released", release_started, None),
                );
            }
        }
        // 路由逆序（最后安装的先删）。
        retry_owned_routes_with(&mut self.installed_routes, remove_owned_route)?;
        // MTU 逆序（V4 族）。
        if let (Some(applied), Some(original)) = (&self.mtu_applied, &self.mtu_original) {
            let ctrl = MtuController::new(applied.luid, applied.family);
            ctrl.compare_and_restore(applied, original)
                .map_err(|e| format!("mtu-restore:{e:?}"))?;
        }
        // 地址：compare-delete owned 行。
        let ctrl = IpAddressController::new(self.luid);
        let current = ctrl.capture().map_err(|e| format!("addr-capture:{e:?}"))?;
        for row in restore_owned_addresses(&self.owned_addresses, &current) {
            ctrl.delete(&row)
                .map_err(|e| format!("addr-delete:{e:?}"))?;
        }
        self.owned_addresses.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 单元测试：纯逻辑（路由目标解析）。真实特权路径由集成/业务门禁覆盖。
// ---------------------------------------------------------------------------
