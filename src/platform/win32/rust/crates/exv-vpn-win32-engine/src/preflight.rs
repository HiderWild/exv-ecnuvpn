
//! 连接前预检（2026-09-27 计划 §三，T2+T3：预检探针框架 + 清理执行器）。
//!
//! 触发与接线归 T4/T7（本模块只导出 API，不从连接路径调用）。六域并行只读探测
//! → 按域策略清理 → 复验，全程只读探测在前、清理是唯一写路径。
//!
//! # 预算
//!
//! 单域探测预算 [`PROBE_TIMEOUT`]（2s，并行 → 探测阶段墙钟 ≈ 最慢域）；清理阶段
//! 每个操作经常驻 PS 子进程毫秒级完成（单操作防御上界 2s）；复验再跑一轮探测。
//! 典型路径（PS 冷启动 ~1s + 各 op 毫秒级）总耗时 ≪ 3s（计划 §三目标）；病态
//! 上界（冷启动贴满 + 逐 op 贴满超时）可超出目标——此时各域以 Timeout/Error
//! 状态如实上报，绝不无限等待。
//!
//! # 底质与域清单（计划 §三 v6）
//!
//! - **NRPT**：经 [`probe_child`]（常驻 PS 子进程，`nrpt_worker` 同款模式）列举；
//!   自有前缀残留删除，外来校园域冲突仅上报（补救三选归 UI）。
//! - **网卡**：`GetAdaptersAddresses` 枚举（复用 resource `proxy_tun`）；发现
//!   仅 Manual（wintun 0.14.1 无非创建者删除路径）。
//! - **路由**：`Get-NetRoute`（PS 子进程）× 存活 ifindex 集（网卡枚举）判死接口；
//!   死接口 `NetMgmt` 行删除，活接口 bypass 形状仅上报。
//! - **接口 DNS**：`Get-DnsClientServerAddress`（PS 子进程）；命中校园解析器集合
//!   仅上报（清理策略 D-pending）。
//! - **系统代理**：`WinINET` 注册表（复用 resource `system_proxy` 读路径 +
//!   `system_proxy_override` 写回/广播）；指向自有回环端口才禁用。
//! - **DNS 缓存**：非独立域；任一清理动作生效后经 PS 子进程
//!   `Clear-DnsClientCache`。
//! - **journal 遗留**：目录非空仅上报（恢复归 W14 既有行为）。
//!
//! # 前提与红线
//!
//! 预检的前提是**无存活 engine 会话**（trace 存在/首连/升级/oneshot provision
//! 触发时成立）；因此 NRPT 自有前缀规则一律按孤儿处理。测试与生产分离
//! （计划 §七）：本模块只包含功能本身，唯一测试接缝是 [`probe_child`] 的
//! 脚本参数（`nrpt_worker` `lifecycle_tests` 同款）。

mod classify;
mod probe_child;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_win32_resource::journal_path::JournalPath;
use exv_vpn_win32_resource::native_error::NativeError;
use exv_vpn_win32_resource::proxy_tun::{enumerate_adapter_records, AdapterRecord};
use exv_vpn_win32_resource::system_proxy_family::ValueName;
use exv_vpn_win32_resource::system_proxy_override::{
    broadcast_wininet_refresh, write_back_value_for_user,
};
use exv_vpn_win32_resource::system_proxy::{
    capture_for_user, snapshot_from_raw, RawInternetSettings, RawValue,
};
use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_DWORD, REG_SZ, REG_VALUE_TYPE,
};

use crate::connection_trace::TraceRecord;
use classify::{
    adapter_findings, classify_adapter_orphans, classify_iface_dns, classify_nrpt,
    classify_proxy, classify_routes, iface_dns_findings, nrpt_findings, proxy_findings,
    route_findings, NrptClassified, ProxyClassified, RoutesClassified,
};
use probe_child::{ProbeChild, ProbeRequest, ProbeResponse, PsDnsIface};

/// 单域探测预算（并行；探测阶段墙钟 ≈ 最慢域）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 预检总预算目标（计划 §三：≤3s；构成见模块级「预算」说明）。
pub const PREFLIGHT_BUDGET_TARGET: Duration = Duration::from_secs(3);

/// PS 子进程启动的观察上界（冷启动就绪 2s + 余量）。
const SPAWN_BOUND: Duration = Duration::from_millis(2_500);

/// `WinINET` `Internet Settings` 子键路径（resource 同款字面量）。
const INTERNET_SETTINGS_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

// ---------------------------------------------------------------------------
// 公开报告类型
// ---------------------------------------------------------------------------

/// 预检域（报告按此稳定排序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainKind {
    /// NRPT 规则。
    Nrpt,
    /// Wintun 网卡。
    Adapter,
    /// IPv4 路由。
    Routes,
    /// 接口 DNS。
    IfaceDns,
    /// 系统代理（WinINET）。
    Proxy,
    /// journal 遗留。
    Journal,
}

/// 单域状态。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainStatus {
    /// 无发现。
    Clean,
    /// 有发现（细看各 Finding 的 policy：清理/仅上报/人工）。
    Findings,
    /// 探测超时（受 [`PROBE_TIMEOUT`] 约束后放弃）。
    Timeout,
    /// 探测失败（携带原因）。
    Error(String),
}

/// 单条发现：匹配到的残留项 + 处置策略（policy 决定清理执行器是否动手）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    /// 稳定标识（如 `nrpt_own_orphan`；消费方按此路由文案/处置）。
    pub kind: &'static str,
    /// 人读细节（规则名/网卡名/目的前缀/代理串等匹配材料）。
    pub detail: String,
    /// 处置策略。
    pub policy: FindingPolicy,
}

/// 发现的处置策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingPolicy {
    /// 自有残留，本轮清理。
    Cleanup,
    /// 仅上报，不动手。
    ReportOnly,
    /// 自有残留但无安全删除路径，需人工。
    Manual,
}

/// 单域报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DomainReport {
    /// 域。
    pub domain: DomainKind,
    /// 清理前状态。
    pub status: DomainStatus,
    /// 清理前发现（含仅上报项）。
    pub findings: Vec<Finding>,
    /// 清理后复验状态（仅对「清理前有发现」的域重跑只读探测；其余 None）。
    pub post_cleanup_status: Option<DomainStatus>,
}

/// 单条清理动作的执行记账。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CleanupAction {
    /// 所属域。
    pub domain: DomainKind,
    /// 对应的发现 kind。
    pub finding_kind: &'static str,
    /// 动作目标（规则名 / 路由前缀@`ifindex` / 网卡名 / `ProxyServer` 串）。
    pub target: String,
    /// 结局。
    pub outcome: CleanupOutcome,
}

/// 清理动作结局。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    /// 已执行且复验前视为生效。
    Done,
    /// 执行失败（携带原因）。
    Failed(String),
    /// 无自动删除路径，转人工。
    Manual,
}

/// 预检报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PreflightReport {
    /// 预检开始时刻（Unix 毫秒）。
    pub started_at_ms: u64,
    /// 总耗时。
    pub elapsed: Duration,
    /// 各域报告（固定域序）。
    pub domains: Vec<DomainReport>,
    /// 清理动作记账（含 Manual 项）。
    pub cleanup: Vec<CleanupAction>,
    /// 是否执行了 DNS 缓存清刷（任一清理动作生效后为 true）。
    pub dns_cache_flushed: bool,
}

// ---------------------------------------------------------------------------
// 输入与入口
// ---------------------------------------------------------------------------

/// 预检输入。
#[derive(Debug, Clone, Copy, Default)]
pub struct PreflightInputs<'a> {
    /// 上一次连接的 trace 记录（触发预检时的遗留痕迹；None = 无 hint 探测）。
    pub record_hint: Option<&'a TraceRecord>,
    /// 发起用户的 SID（服务形态必传；None 时系统代理域退回当前进程 HKCU——
    /// 仅 oneshot 提权用户进程形态下正确，见 [`read_proxy_raw`]）。
    pub user_sid: Option<&'a str>,
    /// 校园命名空间清单（外来 NRPT 冲突的匹配集；T4 接线时来自配置）。
    pub campus_namespaces: &'a [String],
}

/// 运行一次预检（无 `user_sid` / 无校园命名空间集的便捷入口）。
///
/// 服务形态（SYSTEM 进程，HKCU 不是用户配置单元）必须改用
/// [`run_preflight_for`] 并传入 `user_sid`，否则系统代理域探测的不是发起用户。
pub async fn run_preflight(record_hint: Option<&TraceRecord>) -> PreflightReport {
    run_preflight_for(&PreflightInputs {
        record_hint,
        user_sid: None,
        campus_namespaces: &[],
    })
    .await
}

/// 运行一次预检：并行只读探测 → 按域清理 → 复验 → 关闭 PS 子进程。
pub async fn run_preflight_for(inputs: &PreflightInputs<'_>) -> PreflightReport {
    let started_at_ms = unix_now_ms();
    let start = Instant::now();

    // ① 常驻 PS 子进程（各 PS 域共享一个；启动失败 → PS 域记 Error，其余域照常）。
    let ps = spawn_probe_child().await;

    // ② 并行只读探测（每域受 PROBE_TIMEOUT 约束；超时 → Timeout，跳过）。
    let probed = run_probe_set(build_probe_set(&ps, inputs), PROBE_TIMEOUT).await;
    let mut domain_reports: Vec<DomainReport> =
        probed.iter().cloned().map(to_domain_report).collect();

    // ③ 清理阶段（唯一写路径）+ DNS 缓存清刷。
    let (cleanup_actions, changed) = cleanup_phase(&ps, inputs.user_sid, &probed).await;
    let dns_cache_flushed = if changed {
        ps_op(&ps, &ProbeRequest::Flushdns).await.is_ok()
    } else {
        false
    };

    // ④ 复验：只对「清理前有发现」的域重跑只读探测。
    reverify_phase(&ps, inputs, &mut domain_reports).await;

    // ⑤ 关闭 PS 子进程（bye + stdin EOF，有界等待）。
    shutdown_probe_child(&ps).await;

    PreflightReport {
        started_at_ms,
        elapsed: start.elapsed(),
        domains: domain_reports,
        cleanup: cleanup_actions,
        dns_cache_flushed,
    }
}

/// 从预检报告提取**阻断连接**的未决发现（T4 连接门消费；计划 §三）。
///
/// 阻断集合（稳定 reason id，UI 文案后置阶段按 id 路由）：
/// - `nrpt_foreign_conflict`：外来校园域 NRPT 规则冲突（他牌规则不可动，补救
///   三选归 UI/T13）；
/// - `adapter_manual`：需人工处置的发现（wintun 0.14.1 网卡无非创建者删除路径）；
/// - `cleanup_failed`：清理动作执行失败；
/// - `cleanup_unresolved`：已清理域复验仍有发现（清理未收敛）。
///
/// 非阻断：ReportOnly 的 journal 遗留 / 接口 DNS 命中 / 活接口 bypass 形状等
/// （仅上报）；探测 Timeout/Error 域不计阻断（病态环境如实上报，不据此拦连接）。
/// 返回值去重保序（空 = 无阻断，连接放行）。
#[must_use]
pub fn blocking_reasons(report: &PreflightReport) -> Vec<&'static str> {
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut push = |reason: &'static str| {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    };
    for domain in &report.domains {
        if domain
            .findings
            .iter()
            .any(|finding| finding.kind == "nrpt_foreign_conflict")
        {
            push("nrpt_foreign_conflict");
        }
        if domain
            .findings
            .iter()
            .any(|finding| finding.policy == FindingPolicy::Manual)
        {
            push("adapter_manual");
        }
        let had_cleanup = domain
            .findings
            .iter()
            .any(|finding| finding.policy == FindingPolicy::Cleanup);
        if had_cleanup && domain.post_cleanup_status == Some(DomainStatus::Findings) {
            push("cleanup_unresolved");
        }
    }
    for action in &report.cleanup {
        if matches!(action.outcome, CleanupOutcome::Failed(_)) {
            push("cleanup_failed");
        }
    }
    reasons
}

/// 清理阶段（唯一写路径）：对 policy=Cleanup 的发现逐条执行并记账；返回
/// (动作清单, 是否有动作生效)——生效才需要清 DNS 缓存。
async fn cleanup_phase(
    ps: &SharedProbeChild,
    user_sid: Option<&str>,
    probed: &[DomainOutcome],
) -> (Vec<CleanupAction>, bool) {
    let mut actions = Vec::new();
    let mut changed = false;
    for outcome in probed {
        match (&outcome.domain, &outcome.result) {
            (DomainKind::Nrpt, Ok(DomainData::Nrpt(classified))) => {
                for rule in &classified.own {
                    let result = ps_op(ps, &ProbeRequest::NrptDelete { name: &rule.name })
                        .await
                        .map(|_| ());
                    actions.push(record_cleanup(
                        DomainKind::Nrpt,
                        "nrpt_own_orphan",
                        &rule.name,
                        result,
                        &mut changed,
                    ));
                }
            }
            (DomainKind::Adapter, Ok(DomainData::Adapters(orphans))) => {
                // wintun 0.14.1 只有创建者 close 才移除 adapter（冻结事实）：
                // 预检进程不是创建者 → 无安全删除路径，全部转人工。
                for orphan in orphans {
                    actions.push(CleanupAction {
                        domain: DomainKind::Adapter,
                        finding_kind: "adapter_orphan",
                        target: orphan.name.clone(),
                        outcome: CleanupOutcome::Manual,
                    });
                }
            }
            (DomainKind::Routes, Ok(DomainData::Routes(classified))) => {
                for row in &classified.dangling {
                    let result = ps_op(
                        ps,
                        &ProbeRequest::RouteDelete {
                            dest: &row.dest,
                            prefix: row.prefix,
                            ifindex: row.ifindex,
                            next_hop: &row.next_hop,
                        },
                    )
                    .await
                    .map(|_| ());
                    let target = format!(
                        "{dest}/{prefix}@if{index}",
                        dest = row.dest,
                        prefix = row.prefix,
                        index = row.ifindex
                    );
                    actions.push(record_cleanup(
                        DomainKind::Routes,
                        "route_dangling",
                        &target,
                        result,
                        &mut changed,
                    ));
                }
            }
            (
                DomainKind::Proxy,
                Ok(DomainData::Proxy(data)),
            ) if !data.classified.ours.is_empty() => {
                let result = disable_system_proxy(user_sid).await;
                actions.push(record_cleanup(
                    DomainKind::Proxy,
                    "proxy_loopback_ours",
                    &data.server_text.clone().unwrap_or_default(),
                    result,
                    &mut changed,
                ));
            }
            _ => {}
        }
    }
    (actions, changed)
}

/// 复验阶段：对「清理前有发现」的域重跑只读探测，回填 `post_cleanup_status`。
async fn reverify_phase(
    ps: &SharedProbeChild,
    inputs: &PreflightInputs<'_>,
    domain_reports: &mut [DomainReport],
) {
    let findings_domains: Vec<DomainKind> = domain_reports
        .iter()
        .filter(|report| report.status == DomainStatus::Findings)
        .map(|report| report.domain)
        .collect();
    if findings_domains.is_empty() {
        return;
    }
    let reverified = run_probe_set(
        build_probe_set(ps, inputs)
            .into_iter()
            .filter(|(kind, _)| findings_domains.contains(kind))
            .collect(),
        PROBE_TIMEOUT,
    )
    .await;
    for report in domain_reports {
        if let Some(post) = reverified.iter().find(|outcome| outcome.domain == report.domain) {
            report.post_cleanup_status = Some(outcome_status(&post.result));
        }
    }
}

/// 把一次清理 op 的结果记账为 [`CleanupAction`]（成功时置位 `changed`）。
fn record_cleanup(
    domain: DomainKind,
    finding_kind: &'static str,
    target: &str,
    result: Result<(), String>,
    changed: &mut bool,
) -> CleanupAction {
    let outcome = match result {
        Ok(()) => {
            *changed = true;
            CleanupOutcome::Done
        }
        Err(message) => CleanupOutcome::Failed(message),
    };
    CleanupAction {
        domain,
        finding_kind,
        target: target.to_owned(),
        outcome,
    }
}

// ---------------------------------------------------------------------------
// 探测框架（并行 + 单域预算）
// ---------------------------------------------------------------------------

/// 单域探测的结构化产物（清理/复验直接消费分类结果，不再反向解析 detail）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DomainData {
    /// NRPT 域分类。
    Nrpt(NrptClassified),
    /// 网卡域命中的自有残留（空 = 干净）。
    Adapters(Vec<AdapterRecord>),
    /// 路由域分类。
    Routes(RoutesClassified),
    /// 接口 DNS 域命中（空 = 干净）。
    IfaceDns(Vec<PsDnsIface>),
    /// 系统代理域分类 + 原始 `ProxyServer` 串。
    Proxy(ProxyDomainData),
    /// journal 域发现（直接成形）。
    Journal(Vec<Finding>),
}

/// 系统代理域探测产物。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProxyDomainData {
    /// 端点分类。
    pub(crate) classified: ProxyClassified,
    /// 原始 `ProxyServer` 注册表串（渲染细节用）。
    pub(crate) server_text: Option<String>,
}

impl DomainData {
    /// 渲染为报告发现。
    pub(crate) fn render(&self) -> Vec<Finding> {
        match self {
            Self::Nrpt(classified) => nrpt_findings(classified),
            Self::Adapters(orphans) => adapter_findings(orphans),
            Self::Routes(classified) => route_findings(classified),
            Self::IfaceDns(hits) => iface_dns_findings(hits),
            Self::Proxy(data) => proxy_findings(&data.classified, data.server_text.as_deref()),
            Self::Journal(findings) => findings.clone(),
        }
    }
}

/// 单域探测结局（Ok = 探测完成；Err = 超时/错误状态）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DomainOutcome {
    /// 域。
    pub(crate) domain: DomainKind,
    /// 探测结果。
    pub(crate) result: Result<DomainData, DomainStatus>,
}

/// 探针 future（Send + 'static：探针自持数据，域间无共享借用）。
type ProbeFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<DomainData, String>> + Send>>;

/// 单域预算包装：超时 → [`DomainStatus::Timeout`]；探测错误 → `Error(message)`。
async fn bounded_probe(probe: ProbeFuture, bound: Duration) -> Result<DomainData, DomainStatus> {
    match tokio::time::timeout(bound, probe).await {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(message)) => Err(DomainStatus::Error(message)),
        Err(_deadline_elapsed) => Err(DomainStatus::Timeout),
    }
}

/// 并行运行全部域探针（`tokio::spawn` 每域一个任务；结果按提交序回收）。
///
/// 任务 panic 不拖垮整轮预检：按 `Error` 记账该域。
async fn run_probe_set(
    probes: Vec<(DomainKind, ProbeFuture)>,
    bound: Duration,
) -> Vec<DomainOutcome> {
    let handles: Vec<(
        DomainKind,
        tokio::task::JoinHandle<Result<DomainData, DomainStatus>>,
    )> = probes
        .into_iter()
        .map(|(domain, probe)| (domain, tokio::spawn(bounded_probe(probe, bound))))
        .collect();
    let mut outcomes = Vec::with_capacity(handles.len());
    for (domain, handle) in handles {
        let result = match handle.await {
            Ok(result) => result,
            Err(join_error) => Err(DomainStatus::Error(format!("探测任务异常退出：{join_error}"))),
        };
        outcomes.push(DomainOutcome { domain, result });
    }
    outcomes
}

/// 探测结局 → 域报告。
fn to_domain_report(outcome: DomainOutcome) -> DomainReport {
    DomainReport {
        domain: outcome.domain,
        status: outcome_status(&outcome.result),
        findings: outcome.result.map(|data| data.render()).unwrap_or_default(),
        post_cleanup_status: None,
    }
}

/// 探测结局 → 状态（有发现 = Findings；干净 = Clean；Err 原样透传）。
fn outcome_status(result: &Result<DomainData, DomainStatus>) -> DomainStatus {
    match result {
        Ok(data) if data.render().is_empty() => DomainStatus::Clean,
        Ok(_) => DomainStatus::Findings,
        Err(status) => status.clone(),
    }
}

// ---------------------------------------------------------------------------
// 域探针构造（探测与复验共用）
// ---------------------------------------------------------------------------

/// 组装六域探针（探测与复验同一构造路径；复验方按域过滤）。
fn build_probe_set(
    ps: &SharedProbeChild,
    inputs: &PreflightInputs<'_>,
) -> Vec<(DomainKind, ProbeFuture)> {
    let hint_adapters: Vec<String> = inputs
        .record_hint
        .map(|hint| hint.adapter_names.clone())
        .unwrap_or_default();
    let hint_ports: Vec<u16> = inputs
        .record_hint
        .map(|hint| hint.proxy_ports.clone())
        .unwrap_or_default();
    vec![
        (
            DomainKind::Nrpt,
            nrpt_probe(ps.clone(), inputs.campus_namespaces.to_vec()),
        ),
        (DomainKind::Adapter, adapters_probe(hint_adapters)),
        (DomainKind::Routes, routes_probe(ps.clone())),
        (DomainKind::IfaceDns, iface_dns_probe(ps.clone())),
        (
            DomainKind::Proxy,
            proxy_probe(inputs.user_sid.map(str::to_owned), hint_ports),
        ),
        (DomainKind::Journal, journal_probe()),
    ]
}

/// NRPT 域探针：list op → 分类（自有前缀孤儿 / 外来校园域冲突）。
fn nrpt_probe(ps: SharedProbeChild, campus_namespaces: Vec<String>) -> ProbeFuture {
    Box::pin(async move {
        let response = ps_op(&ps, &ProbeRequest::List).await?;
        Ok(DomainData::Nrpt(classify_nrpt(&response.rules, &campus_namespaces)))
    })
}

/// 网卡域探针：`GetAdaptersAddresses` 枚举 → 自有残留（ExvEngine / hint 记录名）。
fn adapters_probe(hint_adapter_names: Vec<String>) -> ProbeFuture {
    Box::pin(async move {
        let records = tokio::task::spawn_blocking(enumerate_adapter_records)
            .await
            .map_err(|e| format!("网卡枚举任务异常退出：{e}"))?
            .map_err(|e| format!("网卡枚举失败：{}", e.message))?;
        Ok(DomainData::Adapters(classify_adapter_orphans(
            &records,
            &hint_adapter_names,
        )))
    })
}

/// 路由域探针：网卡枚举取存活 ifindex 集 + routes op → 死接口/bypass 形状分类。
fn routes_probe(ps: SharedProbeChild) -> ProbeFuture {
    Box::pin(async move {
        let records = tokio::task::spawn_blocking(enumerate_adapter_records)
            .await
            .map_err(|e| format!("网卡枚举任务异常退出：{e}"))?
            .map_err(|e| format!("网卡枚举失败：{}", e.message))?;
        let live: HashSet<u32> = records.iter().map(|record| record.if_index).collect();
        let response = ps_op(&ps, &ProbeRequest::Routes).await?;
        Ok(DomainData::Routes(classify_routes(&response.routes, &live)))
    })
}

/// 接口 DNS 域探针：dnservers op → 命中校园解析器集合的接口（仅上报）。
fn iface_dns_probe(ps: SharedProbeChild) -> ProbeFuture {
    Box::pin(async move {
        let response = ps_op(&ps, &ProbeRequest::Dnservers).await?;
        Ok(DomainData::IfaceDns(classify_iface_dns(&response.ifaces)))
    })
}

/// 系统代理域探针：读 `WinINET` 注册表 → 规范化快照 → 回环端点分类。
fn proxy_probe(user_sid: Option<String>, our_ports: Vec<u16>) -> ProbeFuture {
    Box::pin(async move {
        let (raw, server_text) = tokio::task::spawn_blocking(move || {
            read_proxy_raw(user_sid.as_deref())
        })
        .await
        .map_err(|e| format!("系统代理读取任务异常退出：{e}"))??;
        let snapshot = snapshot_from_raw(&raw)
            .map_err(|e| format!("系统代理快照自洽性校验失败：{}", e.message))?;
        Ok(DomainData::Proxy(ProxyDomainData {
            classified: classify_proxy(&snapshot, &our_ports),
            server_text,
        }))
    })
}

/// journal 域探针：机器级 journal 目录非空 → 仅上报（恢复归 W14 既有行为）。
fn journal_probe() -> ProbeFuture {
    Box::pin(async move {
        let findings = tokio::task::spawn_blocking(journal_leftover_findings)
            .await
            .map_err(|e| format!("journal 检查任务异常退出：{e}"))??;
        Ok(DomainData::Journal(findings))
    })
}

/// journal 目录遗留发现。
fn journal_leftover_findings() -> Result<Vec<Finding>, String> {
    let dir = JournalPath::machine_default().as_path().to_path_buf();
    match std::fs::read_dir(&dir) {
        Ok(mut entries) => {
            if entries.any(|entry| entry.is_ok()) {
                Ok(vec![Finding {
                    kind: "journal_leftover",
                    detail: format!(
                        "journal 目录 {} 存在遗留条目（恢复归 W14 既有行为）",
                        dir.display()
                    ),
                    policy: FindingPolicy::ReportOnly,
                }])
            } else {
                Ok(Vec::new())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("journal 目录读取失败：{e}")),
    }
}

// ---------------------------------------------------------------------------
// 系统代理读写（读复用 resource；HKCU 回退为本模块私有直调）
// ---------------------------------------------------------------------------

/// 读发起用户的 `Internet Settings` 原始值。
///
/// - `Some(sid)`：HKU\<sid>（复用 resource `capture_for_user`；服务形态正确路径）。
/// - `None`：当前进程 HKCU（oneshot 提权用户进程形态下即发起用户；SYSTEM 服务
///   形态下不是——服务形态必须显式传 sid）。
fn read_proxy_raw(user_sid: Option<&str>) -> Result<(RawInternetSettings, Option<String>), String> {
    let raw = match user_sid {
        Some(sid) => capture_for_user(sid)
            .map_err(|e| format!("读取 HKU Internet Settings 失败：{}", e.message))?,
        None => capture_hkcu_internet_settings()
            .map_err(|e| format!("读取 HKCU Internet Settings 失败：{}", e.message))?,
    };
    let server_text = raw.proxy_server.as_sz().map(str::to_owned);
    Ok((raw, server_text))
}

/// 禁用系统代理（ProxyEnable=0），仅对「指向自有回环端口」的发现调用。
///
/// sid 路径复用 resource `write_back_value_for_user`；无 sid 路径为 `HKCU` 直调
/// （同一注册表值的写回）。成功后广播 `WinINET` 刷新（既有通道）。
async fn disable_system_proxy(user_sid: Option<&str>) -> Result<(), String> {
    let result = match user_sid {
        Some(sid) => {
            let sid = sid.to_owned();
            tokio::task::spawn_blocking(move || {
                write_back_value_for_user(&sid, ValueName::ProxyEnable, &RawValue::Dword(0))
            })
            .await
            .map_err(|e| format!("系统代理禁用任务异常退出：{e}"))?
            .map_err(|e| format!("系统代理禁用写回失败：{}", e.message))
        }
        None => tokio::task::spawn_blocking(disable_hkcu_proxy)
            .await
            .map_err(|e| format!("系统代理禁用任务异常退出：{e}"))?
            .map_err(|e| format!("系统代理禁用写回失败：{e}")),
    };
    result?;
    // WinINET 刷新广播为尽力而为（无 wininet 的环境重启后由系统读取生效）。
    let _ = tokio::task::spawn_blocking(broadcast_wininet_refresh).await;
    Ok(())
}

/// 读当前进程 HKCU 的 `Internet Settings`（ProxyEnable/ProxyServer 两值；
/// 其余记 Absent——快照解析只消费这两个值）。镜像 resource `system_proxy.rs`
/// 的两遍 `RegQueryValueExW` 形态。
fn capture_hkcu_internet_settings() -> Result<RawInternetSettings, NativeError> {
    let subkey = HSTRING::from(INTERNET_SETTINGS_SUBKEY);
    let mut key = HKEY::default();
    // SAFETY: subkey 是合法 NUL 结尾宽字符串；key 为输出句柄，用毕关闭。
    let open = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, None, KEY_READ, &raw mut key)
    };
    if open.0 != 0 {
        return Err(NativeError::from_win32(
            open.0,
            "预检打开 HKCU Internet Settings 失败",
        ));
    }
    let result = (|| {
        Ok(RawInternetSettings {
            proxy_enable: hkcu_query_value(key, "ProxyEnable")?,
            proxy_server: hkcu_query_value(key, "ProxyServer")?,
            proxy_override: RawValue::Absent,
            auto_config_url: RawValue::Absent,
            auto_detect: RawValue::Absent,
        })
    })();
    // SAFETY: key 是本函数打开的句柄；无论成败都要关闭。
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// 读 HKCU 打开键上的单个注册表值（两遍查询：先取大小再取数据）。
fn hkcu_query_value(key: HKEY, name: &str) -> Result<RawValue, NativeError> {
    let name_w = HSTRING::from(name);
    let mut kind = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    // SAFETY: 第一遍只取所需字节数（buffer 传 None）；kind/len 由系统填充。
    let rc = unsafe {
        RegQueryValueExW(key, &name_w, None, Some(&raw mut kind), None, Some(&raw mut len))
    };
    if rc.0 == ERROR_FILE_NOT_FOUND.0 || rc.0 == ERROR_PATH_NOT_FOUND.0 {
        return Ok(RawValue::Absent);
    }
    if rc.0 != 0 {
        return Err(NativeError::from_win32(
            rc.0,
            "RegQueryValueExW 取值大小失败",
        ));
    }
    let mut buff = vec![0u8; len as usize];
    let mut kind2 = REG_VALUE_TYPE(0);
    let mut len2 = len;
    // SAFETY: buff 按 len 分配；len2 输入输出；kind2 为输出。
    let rc = unsafe {
        RegQueryValueExW(
            key,
            &name_w,
            None,
            Some(&raw mut kind2),
            Some(buff.as_mut_ptr()),
            Some(&raw mut len2),
        )
    };
    if rc.0 != 0 {
        return Err(NativeError::from_win32(rc.0, "RegQueryValueExW 读值失败"));
    }
    buff.truncate(len2 as usize);
    match kind2 {
        k if k == REG_SZ => {
            let utf16: Vec<u16> = buff
                .chunks_exact(2)
                .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
                .take_while(|ch| *ch != 0)
                .collect();
            Ok(RawValue::Sz(String::from_utf16(&utf16).unwrap_or_default()))
        }
        k if k == REG_DWORD => {
            if buff.len() < 4 {
                return Err(NativeError::from_win32(
                    1450,
                    "REG_DWORD 数据不足 4 字节",
                ));
            }
            let bytes = [buff[0], buff[1], buff[2], buff[3]];
            Ok(RawValue::Dword(u32::from_le_bytes(bytes)))
        }
        other => Ok(RawValue::Other {
            r#type: other.0,
            data: buff,
        }),
    }
}

/// HKCU 直写 `ProxyEnable=0`（与 resource `write_back_value_for_user` 同一值的
/// 无 sid 回退路径）。
fn disable_hkcu_proxy() -> Result<(), String> {
    let subkey = HSTRING::from(INTERNET_SETTINGS_SUBKEY);
    let value_name = HSTRING::from("ProxyEnable");
    let mut key = HKEY::default();
    // SAFETY: subkey 是合法 NUL 结尾宽字符串；key 为输出句柄，用毕关闭。
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            KEY_READ | KEY_WRITE | KEY_SET_VALUE,
            &raw mut key,
        )
    };
    if open.0 != 0 {
        return Err(format!("打开 HKCU Internet Settings 失败（win32 {code}）", code = open.0));
    }
    let data = 0u32.to_le_bytes();
    // SAFETY: key 来自上方成功的打开；value_name 是 NUL 结尾宽字符串；data 为
    // 4 字节 REG_DWORD 载荷。
    let set = unsafe { RegSetValueExW(key, &value_name, None, REG_DWORD, Some(&data)) };
    // SAFETY: key 是本函数打开的句柄；无论写成败都要关闭。
    let close = unsafe { RegCloseKey(key) };
    if set.0 != 0 {
        return Err(format!("写回 ProxyEnable 失败（win32 {code}）", code = set.0));
    }
    if close.0 != 0 {
        return Err(format!("关闭 HKCU Internet Settings 失败（win32 {code}）", code = close.0));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 常驻 PS 子进程共享句柄
// ---------------------------------------------------------------------------

/// 各 PS 域共享的子进程句柄（None = 启动失败/超时，PS 域记 Error）。
type SharedProbeChild = Option<Arc<tokio::sync::Mutex<Option<ProbeChild>>>>;

/// 启动常驻 PS 子进程（有界等待 ready；超时/失败返回 None，不阻塞整轮预检）。
async fn spawn_probe_child() -> SharedProbeChild {
    let joined =
        tokio::time::timeout(SPAWN_BOUND, tokio::task::spawn_blocking(ProbeChild::start)).await;
    match joined {
        Ok(Ok(Ok(child))) => Some(Arc::new(tokio::sync::Mutex::new(Some(child)))),
        Ok(Ok(Err(message))) => {
            tracing::warn!(%message, "预检 PowerShell 子进程启动失败，PS 相关域将报 Error");
            None
        }
        Ok(Err(join_error)) => {
            tracing::warn!(%join_error, "预检 PowerShell 子进程启动任务异常退出");
            None
        }
        Err(_) => {
            tracing::warn!("预检 PowerShell 子进程启动超时，PS 相关域将报 Error");
            None
        }
    }
}

/// 在共享子进程上执行一个 op（串行互斥；op 校验 ok 与应答回显配对）。
async fn ps_op(ps: &SharedProbeChild, request: &ProbeRequest<'_>) -> Result<ProbeResponse, String> {
    let Some(ps) = ps.as_ref() else {
        return Err("预检 PowerShell 子进程不可用（启动失败或超时）。".to_owned());
    };
    let expected_op = request.op_name();
    let line = serde_json::to_string(request)
        .map_err(|e| format!("预检 op 请求编码失败：{e}"))?;
    let ps = Arc::clone(ps);
    let response = tokio::task::spawn_blocking(move || {
        let mut guard = ps.blocking_lock();
        let Some(child) = guard.as_mut() else {
            return Err("预检 PowerShell 子进程已关闭。".to_owned());
        };
        child.request_line(&line)
    })
    .await
    .map_err(|e| format!("预检 op 任务异常退出：{e}"))??;
    let response: ProbeResponse = serde_json::from_str(&response)
        .map_err(|e| format!("预检 PowerShell 应答无法解析：{e}"))?;
    if response.op != expected_op {
        return Err(format!(
            "预检 PowerShell 应答 op 不配对：期望 {expected_op}，收到 {received}",
            received = response.op
        ));
    }
    if !response.ok {
        return Err(format!(
            "预检 op {expected_op} 失败：{reason}",
            reason = response.error.unwrap_or_else(|| "未知错误".to_owned())
        ));
    }
    Ok(response)
}

/// 关闭共享子进程（take 出来后 bye + EOF + 有界等待退出；幂等）。
async fn shutdown_probe_child(ps: &SharedProbeChild) {
    if let Some(ps) = ps.as_ref() {
        let ps = Arc::clone(ps);
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(mut child) = ps.blocking_lock().take() {
                child.shutdown();
            }
        })
        .await;
    }
}

// ---------------------------------------------------------------------------
// 兜底认领清理（计划 v6.1 §8.2 触发模型补洞；T2/T3 清理器的同款策略与代码）
// ---------------------------------------------------------------------------

/// 兜底认领清理的结局（受理门/connect 路由侧重试判定的输入）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NrptClaimClean {
    /// 已删除的自有前缀（`EXV-NRPT:`）规则名。
    pub(crate) removed: Vec<String>,
    /// 现存的外来校园域冲突规则名（未动——T2 裁定不变，只上报）。
    pub(crate) foreign_present: Vec<String>,
    /// 探测与删除是否全部成功（false = 清理未收敛）。
    pub(crate) all_ok: bool,
}

/// 兜底认领清理（计划 v6.1 §8.2 触发模型补洞）：连接路径撞自有 NRPT conflict 时
/// （预检三触发源全不命中的路径——上次善后未竟的残留规则），只针对 NRPT 域跑
/// 一轮「探测 → 分类 → 删除」。
///
/// **与预检清理阶段完全同源**（非新机制）：[`classify_nrpt`] 的 `own` 集 =
/// `EXV-NRPT:` 前缀规则（孤儿语义——任意 GUID 均为 EXV 残留）；删除走同一
/// [`ProbeRequest::NrptDelete`]；生效后清 DNS 缓存。**外来规则（无前缀的校园域
/// 规则）绝不动**（T2 裁定不变——他牌规则只上报，补救三选归 UI）。
pub(crate) async fn claim_clean_own_nrpt(campus_namespaces: &[String]) -> NrptClaimClean {
    let ps = spawn_probe_child().await;
    let outcome = claim_clean_own_nrpt_on(&ps, campus_namespaces).await;
    shutdown_probe_child(&ps).await;
    outcome
}

/// 认领清理主体（子进程句柄由调用方提供——测试注入假脚本子进程，生产经
/// [`claim_clean_own_nrpt`] 用真实 PS 子进程）。
async fn claim_clean_own_nrpt_on(
    ps: &SharedProbeChild,
    campus_namespaces: &[String],
) -> NrptClaimClean {
    if ps.is_none() {
        return NrptClaimClean::default();
    }
    let response = match ps_op(ps, &ProbeRequest::List).await {
        Ok(response) => response,
        Err(_) => return NrptClaimClean::default(),
    };
    let classified = classify_nrpt(&response.rules, campus_namespaces);
    let foreign_present = classified
        .foreign_conflict
        .iter()
        .map(|rule| rule.name.clone())
        .collect();
    let mut removed = Vec::new();
    let mut all_ok = true;
    for rule in &classified.own {
        match ps_op(ps, &ProbeRequest::NrptDelete { name: &rule.name }).await {
            Ok(_) => removed.push(rule.name.clone()),
            Err(_) => all_ok = false,
        }
    }
    if !removed.is_empty() {
        let _ = ps_op(ps, &ProbeRequest::Flushdns).await;
    }
    NrptClaimClean {
        removed,
        foreign_present,
        all_ok,
    }
}

/// Unix 毫秒时间戳（时钟回拨等异常退化为 0，仅用于报告标记）。
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// 单元测试（合成输入；不触碰真实 NRPT/路由/注册表/PS——真机行为归验收门禁）。
// PS 子进程生命周期用假脚本在 probe_child.rs 内测（nrpt_worker lifecycle 同款）。
// ---------------------------------------------------------------------------
