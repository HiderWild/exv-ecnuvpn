
//! NRPT 外来规则静默精准接管执行器（D8 阶段 2；2026-10-09 用户裁定）。
//!
//! 裁定原文：「fail-closed 改静默覆盖，程序逻辑保证精准覆盖 + 最小必要覆盖」
//! ——本执行器是处置半边；判定半边在 [`crate::preflight::takeover_targets`]
//! （纯函数，同源同输入）。**journal 前置可恢复是静默的底气**（裁定）：
//! 删除前把原规则完整形状落 journal（FlushFileBuffers 持久），journal 写失败
//! 即中止全部覆盖（fail-closed 保留在异常路径上）。
//!
//! 执行序（对每个接管目标；预算 [`TAKEOVER_BUDGET`] 硬顶）：
//!
//! ```text
//! 定位（list 双表投影 → takeover_targets）
//!   → journal 前置（status=planned；含完整形状 + restore_hint + 厂商在场注记）
//!   → 同名漂移复核（list 重读；内容漂移 → 以新内容补记 planned/supersedes）
//!   → 删除（rule_table=按名删除；effective_only=Restart-Service Dnscache）
//!   → 按原动作结果记账（applied；完成后继续实际 NRPT Add）
//! ```
//!
//! # 防死锁约束对照（2026-09-26 孤儿事故复盘四约束）
//!
//! - 全程复用预检常驻探针子进程的**既有**有界生命周期（spawn ≤2.5s、普通 op ≤2s、
//!   退出 ≤0.5s）；`restart_dnscache` 是唯一例外——专用 8s 超时
//!   （[`RESTART_OP_TIMEOUT`]）；失败或超时直接返回，保留原错误；
//! - 不启动 NRPT worker 做接管（接管发生在 worker 启动之前、`install_live` 之前
//!   ——此刻自有规则尚未入表、隧道未提交）；
//! - teardown 不恢复被接管的外来规则（恢复=制造 EXV 不拥有的规则；journal +
//!   `scripts/repair-foreign-nrpt-rule.ps1 -Restore` 保留人工路径）；
//! - 失败收口有界（[`TakeoverFailure`] → 连接按 `platform/nrpt_takeover_failed` /
//!   `platform/nrpt_foreign_rebuild` 失败终态，词表见 D9 的 copy-tables）。
//!
//! # 预算与断开竞态（复审遗漏 7）
//!
//! 接管硬顶 10s > 断开路径对在途组装的 5s 有界等待：断开已回报完成后，接管/
//! Dnscache 重启仍可能在途完成（1–3 秒解析闪断发生在「已断开」回报之后）——
//! 后果有界且自愈（下次连接重判定），风险已在 D8 计划 §六 声明；调用方在接管
//! 前的取消令牌段边界检查尽早退出。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_resource::journal::JournalRecord;
use exv_vpn_win32_resource::journal_path::JournalPath;
use exv_vpn_win32_resource::journal_store::{RecoverOutcome, WinJournalStore};
use sha2::{Digest, Sha256};

use crate::log_sink::{LogLevel, LogSink};
use crate::preflight::probe_child::{
    ProbeRequest, PsNrptPolicyEntry, PsNrptRule, RESTART_OP_TIMEOUT,
};
use crate::preflight::{
    ps_op, ps_op_with_timeout, shutdown_probe_child, spawn_probe_child, takeover_targets,
    NrptTakeoverStore, NrptTakeoverTarget, SharedProbeChild,
};
use crate::vendor_presence::{snapshot as vendor_snapshot, VendorPresence};

/// 接管预算覆盖定位、前置 journal 和原动作；Restart 使用既有专用完成应答期限。
pub(crate) const TAKEOVER_BUDGET: Duration = Duration::from_secs(10);

/// 接管执行参数（生产 [`Default`]；测试注入缩小的预算/超时钉有界行为）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TakeoverParams {
    /// 全程硬顶。
    pub(crate) budget: Duration,
    /// `restart_dnscache` 单 op 应答上界（复审遗漏 3 必改：8s 专用，不用 2s）。
    pub(crate) restart_op_timeout: Duration,
}

impl Default for TakeoverParams {
    fn default() -> Self {
        Self {
            budget: TAKEOVER_BUDGET,
            restart_op_timeout: RESTART_OP_TIMEOUT,
        }
    }
}

/// 接管失败形态（连接按 `platform/nrpt_takeover_failed` 收口；异常路径保留
/// fail-closed——只是不再请示用户）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TakeoverFailure {
    /// journal 前置失败（已中止全部覆盖：零删除）。
    JournalFailed(String),
    /// 删除 op 失败（可能部分目标已删除——journal 逐条存证）。
    DeleteFailed(String),
    /// 原动作前定位或精确目标选择失败。
    VerifyFailed(String),
    /// 预算耗尽。**删除可能已生效、journal 已存证——这是接受态而非需回滚的
    /// 中间态**（复审判定 11）：连接失败收口，被删规则可由 `-Restore` 恢复。
    BudgetExceeded(String),
}

impl TakeoverFailure {
    /// 失败摘要（本地日志/journal 用；不落 wire）。
    pub(crate) fn detail(&self) -> &str {
        match self {
            Self::JournalFailed(_) => "journal_failed",
            Self::DeleteFailed(_) => "delete_failed",
            Self::VerifyFailed(_) => "verify_failed",
            Self::BudgetExceeded(_) => "budget_exceeded",
        }
    }

    /// 原始错误文本。
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::JournalFailed(m)
            | Self::DeleteFailed(m)
            | Self::VerifyFailed(m)
            | Self::BudgetExceeded(m) => m,
        }
    }
}

/// 接管结局（消费方：`tunnel_runtime` T3 接线）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TakeoverOutcome {
    /// 原删除或 Restart 返回成功的形式。
    pub(crate) taken_over_forms: Vec<String>,
    /// 定位与删除之间自行消失的形式（瞬态形态——视为成功不覆盖）。
    pub(crate) vanished_before_delete: Vec<String>,
    /// 实际触及的存储分支标签。
    pub(crate) store_reached: Vec<&'static str>,
    /// 本连接内已执行过动作的同指纹目标再次进入前置选择。
    pub(crate) rebuilt_forms: Vec<String>,
    /// 已发出删除 op 的目标数（「单指纹单次覆盖」硬闸的覆盖计数）。
    pub(crate) delete_issued_count: usize,
    /// 失败（None = 全部目标处理完毕）。
    pub(crate) failed: Option<TakeoverFailure>,
}

impl TakeoverOutcome {
    /// 接管已实际介入（删除已发出或检出重建环）——T3 据此决定 conflict 失败
    /// 是否挂 `platform/nrpt_foreign_rebuild`（未介入的 conflict 维持泛化语义）。
    pub(crate) fn engaged(&self) -> bool {
        self.delete_issued_count > 0 || !self.rebuilt_forms.is_empty()
    }
}

// ---------------------------------------------------------------------------
// journal payload（§4.6；status 字段 = 复审遗漏 4 的完成标记）
// ---------------------------------------------------------------------------

/// 规则完整形状（`ipsec` 对 effective_only 目标**不可观测**——有效策略表无
/// IPsec 投影属性，如实置 null，绝不默认 false 谎报形状；复审判定 1 补充）。
#[derive(serde::Serialize)]
struct FullShape {
    dnssec: bool,
    ipsec: Option<bool>,
    direct_access: bool,
    /// ipsec 是否可观测（rule_table=true / effective_only=false）。
    ipsec_observable: bool,
    /// 编码声明（探针投影为 UTF-8 无映射损失形态）。
    encoding: &'static str,
}

/// `nrpt_foreign_taken_over` 前置记录（status=planned；superseded 由
/// 后续记录的 supersedes 链接表达）。
#[derive(serde::Serialize)]
struct TakeoverJournalPayload {
    kind: &'static str,
    /// 前置标记 `planned`；原动作完成另记 applied，旧恢复脚本仍消费原形状记录。
    status: &'static str,
    fingerprint_hex: String,
    /// 同名漂移复核后以新内容取代的旧指纹（无漂移为 null）。
    supersedes: Option<String>,
    namespace_forms: Vec<String>,
    servers: Vec<String>,
    store: &'static str,
    /// 句柄（rule_table=规则 Name；effective_only=空——无公开寻址手段）。
    store_handle: String,
    full_shape: FullShape,
    /// `Add-DnsClientNrptRule` 参数集原文（含附加设置开关；人工恢复依据）。
    restore_hint: String,
    operation_id: String,
    vendor_presence: BTreeMap<String, String>,
    taken_at_ms: u64,
    engine_pid: u32,
}

/// 原动作完成记录（`nrpt_foreign_taken_over_applied`），只表述动作返回成功。
#[derive(serde::Serialize)]
struct TakeoverOutcomePayload {
    kind: &'static str,
    fingerprint_hex: String,
    forms: Vec<String>,
    operation_id: String,
    taken_at_ms: u64,
    engine_pid: u32,
}

// ---------------------------------------------------------------------------
// 入口（生产：真探针 + 机器级 journal）
// ---------------------------------------------------------------------------

/// 静默精准接管（生产入口：自建常驻探针子进程 + 机器级 journal 目录）。
///
/// `covered_fingerprints`：**调用方持有**的「本连接内已覆盖指纹」账本（初始
/// 分支与 worker conflict 分支共享——单次连接内同一指纹至多覆盖一次，重建即停）。
/// 本函数会向其追加已发出删除的指纹。
pub(crate) async fn takeover_foreign_nrpt(
    campus_domains: &[String],
    session_servers: &[String],
    covered_fingerprints: &mut Vec<String>,
    log: &LogSink,
    operation_id: &str,
) -> TakeoverOutcome {
    let ps = spawn_probe_child().await;
    let journal_dir: PathBuf = JournalPath::machine_default().as_path().to_path_buf();
    let outcome = takeover_foreign_nrpt_on(
        &ps,
        &journal_dir,
        campus_domains,
        session_servers,
        covered_fingerprints,
        log,
        operation_id,
        TakeoverParams::default(),
    )
    .await;
    shutdown_probe_child(&ps).await;
    outcome
}

/// 执行主体（探针句柄/journal 目录/参数由调用方提供——测试注入假脚本子进程与
/// 临时 journal 目录；`nrpt_worker` lifecycle 同款接缝）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn takeover_foreign_nrpt_on(
    ps: &SharedProbeChild,
    journal_dir: &Path,
    campus_domains: &[String],
    session_servers: &[String],
    covered_fingerprints: &mut Vec<String>,
    log: &LogSink,
    operation_id: &str,
    params: TakeoverParams,
) -> TakeoverOutcome {
    let mut outcome = TakeoverOutcome::default();
    let deadline = Instant::now() + params.budget;

    // ---- ① 定位：list 双表投影 → takeover_targets（会话服务器权威判定）。----
    let (rules1, effective1) = match locate(ps).await {
        Ok(tables) => tables,
        Err(message) => {
            outcome.failed = Some(TakeoverFailure::VerifyFailed(format!("定位失败：{message}")));
            emit_failed(log, &outcome, campus_domains);
            return outcome;
        }
    };
    let mut planned: Vec<NrptTakeoverTarget> =
        takeover_targets(campus_domains, session_servers, &rules1, &effective1);
    // 单指纹单次覆盖硬闸：本连接已覆盖过的指纹 → 重建即停（不再覆盖）。
    for target in planned
        .iter()
        .filter(|target| covered_fingerprints.contains(&target_fingerprint(target)))
    {
        outcome.rebuilt_forms.push(target.form.clone());
    }
    planned.retain(|target| !covered_fingerprints.contains(&target_fingerprint(target)));
    if planned.is_empty() {
        // 无目标（等效已被接纳/自有孤儿归认领清理/全部为已覆盖指纹）——无动作。
        emit_noop(log, &outcome, campus_domains);
        return outcome;
    }

    // ---- ② 厂商在场注记（仅日志/journal，不进判定分支；§3.3）。----
    let vendor = vendor_snapshot();

    // ---- ③ journal 前置（status=planned；任何失败即中止全部覆盖）。----
    let mut journal = match open_journal_for_takeover(journal_dir) {
        Ok(journal) => journal,
        Err(message) => {
            outcome.failed = Some(TakeoverFailure::JournalFailed(message));
            emit_failed(log, &outcome, campus_domains);
            return outcome;
        }
    };
    emit_started(log, &planned, &vendor, session_servers);
    for target in &planned {
        let fingerprint = target_fingerprint(target);
        if let Err(message) = journal_planned(
            &mut journal,
            journal_dir,
            target,
            fingerprint,
            None,
            operation_id,
            &vendor,
        ) {
            outcome.failed = Some(TakeoverFailure::JournalFailed(message));
            emit_failed(log, &outcome, campus_domains);
            return outcome;
        }
    }

    // ---- ④ 同名漂移复核（复审遗漏 6 必改：删除前重读一次，定位与删除之间
    //      同名规则被替换时以**新内容**补记 journal 再删——账实一致）。----
    if deadline_exceeded(&deadline) {
        return budget_exceeded(log, outcome, campus_domains);
    }
    let (rules2, effective2) = match locate(ps).await {
        Ok(tables) => tables,
        Err(message) => {
            outcome.failed =
                Some(TakeoverFailure::VerifyFailed(format!("漂移复核失败：{message}")));
            emit_failed(log, &outcome, campus_domains);
            return outcome;
        }
    };
    let fresh = takeover_targets(campus_domains, session_servers, &rules2, &effective2);
    let mut vanished = Vec::new();
    let mut effective_only_present = false;
    let mut journaled: Vec<NrptTakeoverTarget> = Vec::new();
    for target in &planned {
        let Some(fresh_target) = fresh.iter().find(|fresh| fresh.form == target.form) else {
            // 探测与删除之间自行消失（瞬态形态）——视为成功不覆盖（§5.2）。
            vanished.push(target.form.clone());
            continue;
        };
        let planned_fingerprint = target_fingerprint(target);
        let fresh_fingerprint = target_fingerprint(fresh_target);
        if fresh_fingerprint != planned_fingerprint {
            // 内容漂移：以新内容补记 planned/supersedes，删除按新内容执行
            //（复审遗漏 6：journal 记录内容与实际删除对象保持一致）。
            if let Err(message) = journal_planned(
                &mut journal,
                journal_dir,
                fresh_target,
                fresh_fingerprint,
                Some(planned_fingerprint),
                operation_id,
                &vendor,
            ) {
                outcome.failed = Some(TakeoverFailure::JournalFailed(message));
                emit_failed(log, &outcome, campus_domains);
                return outcome;
            }
        }
        if fresh_target.store == NrptTakeoverStore::EffectiveOnly {
            effective_only_present = true;
        }
        journaled.push(fresh_target.clone());
    }
    outcome.vanished_before_delete = vanished;
    if journaled.is_empty() {
        emit_noop(log, &outcome, campus_domains);
        return outcome;
    }

    // ---- ⑤ 删除（effective_only → restart_dnscache；rule_table → 按名删除）。----
    if deadline_exceeded(&deadline) {
        return budget_exceeded(log, outcome, campus_domains);
    }
    if effective_only_present {
        match ps_op_with_timeout(
            ps,
            &ProbeRequest::RestartDnscache,
            Some(params.restart_op_timeout),
        ).await {
            Ok(_) => {
                outcome.store_reached.push("effective_only");
                for target in journaled.iter().filter(|target| target.store == NrptTakeoverStore::EffectiveOnly) {
                    if let Err(message) = record_completed_target(
                        &mut outcome, covered_fingerprints, target,
                        &mut journal, journal_dir, operation_id,
                    ) {
                        outcome.failed = Some(TakeoverFailure::JournalFailed(message));
                        emit_failed(log, &outcome, campus_domains);
                        return outcome;
                    }
                }
            }
            Err(message) => {
                outcome.failed = Some(TakeoverFailure::DeleteFailed(format!("restart_dnscache 失败：{message}")));
                emit_failed(log, &outcome, campus_domains);
                return outcome;
            }
        }
    }
    for target in &journaled {
        if target.store != NrptTakeoverStore::RuleTable {
            continue;
        }
        let Some(rule) = &target.rule else { continue };
        match ps_op(ps, &ProbeRequest::NrptDelete { name: &rule.name }).await {
            Ok(_) => {
                if !outcome.store_reached.contains(&"rule_table") {
                    outcome.store_reached.push("rule_table");
                }
                if let Err(message) = record_completed_target(&mut outcome, covered_fingerprints, target, &mut journal, journal_dir, operation_id) {
                    outcome.failed = Some(TakeoverFailure::JournalFailed(message));
                    emit_failed(log, &outcome, campus_domains);
                    return outcome;
                }
            }
            Err(message) => {
                outcome.failed = Some(TakeoverFailure::DeleteFailed(format!(
                    "nrpt_delete {name} 失败：{message}",
                    name = rule.name
                )));
                emit_failed(log, &outcome, campus_domains);
                return outcome;
            }
        }
    }
    // 原 Delete 成功后清解析缓存；Clear 本身的失败继续返回。
    if journaled.iter().any(|target| target.store == NrptTakeoverStore::RuleTable) {
        if let Err(message) = ps_op(ps, &ProbeRequest::Flushdns).await {
            outcome.failed = Some(TakeoverFailure::DeleteFailed(format!("flushdns 失败：{message}")));
            emit_failed(log, &outcome, campus_domains);
            return outcome;
        }
    }
    // ---- ⑦ 留痕终态事件。----
    let vendor_fields = vendor.annotation_fields();
    let mut fields: Vec<(&str, String)> = vec![
        (
            "forms",
            outcome
                .taken_over_forms
                .iter()
                .chain(outcome.vanished_before_delete.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("stores", outcome.store_reached.join(",")),
        ("servers", session_servers.join(",")),
        ("rebuilt_forms", outcome.rebuilt_forms.join(",")),
    ];
    for (key, value) in &vendor_fields {
        fields.push((key, value.clone()));
    }
    let fields_ref: Vec<(&str, &str)> = fields
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    if !outcome.taken_over_forms.is_empty() || !outcome.vanished_before_delete.is_empty() {
        log.emit(
            LogLevel::Info,
            "engine",
            "tunnel.nrpt.foreign.taken_over",
            "外来 NRPT 规则已静默精准接管（journal 已存证可恢复；清单外零接触）",
            &fields_ref,
        );
    }
    if !outcome.rebuilt_forms.is_empty() {
        log.emit(
            LogLevel::Warn,
            "engine",
            "tunnel.nrpt.foreign.rebuilt",
            "外来 NRPT 规则删除后重现（重建环）：已停止覆盖（单指纹单次覆盖硬闸），journal 已落 rebuild 记录",
            &fields_ref,
        );
    }
    outcome
}

/// list 双表投影。
async fn locate(
    ps: &SharedProbeChild,
) -> Result<(Vec<PsNrptRule>, Vec<PsNrptPolicyEntry>), String> {
    let response = ps_op(ps, &ProbeRequest::List).await?;
    Ok((response.rules, response.effective))
}

/// 每个已完成的原动作立即登记，后续动作失败时保留已经承担的清理与恢复事实。
fn record_completed_target(
    outcome: &mut TakeoverOutcome,
    covered: &mut Vec<String>,
    target: &NrptTakeoverTarget,
    journal: &mut WinJournalStore,
    journal_dir: &Path,
    operation_id: &str,
) -> Result<(), String> {
    outcome.delete_issued_count += 1;
    outcome.taken_over_forms.push(target.form.clone());
    covered.push(target_fingerprint(target));
    append_outcome_journal(journal, journal_dir, &TakeoverOutcomePayload {
        kind: "nrpt_foreign_taken_over_applied",
        fingerprint_hex: target_fingerprint(target),
        forms: vec![target.form.clone()],
        operation_id: operation_id.to_owned(),
        taken_at_ms: unix_now_ms(),
        engine_pid: std::process::id(),
    })
}

fn deadline_exceeded(deadline: &Instant) -> bool {
    Instant::now() >= *deadline
}

fn budget_exceeded(
    log: &LogSink,
    mut outcome: TakeoverOutcome,
    campus_domains: &[String],
) -> TakeoverOutcome {
    outcome.failed = Some(TakeoverFailure::BudgetExceeded(format!(
        "接管预算 {}ms 耗尽（删除可能已生效、journal 已存证——接受态，可由 -Restore 恢复）",
        TAKEOVER_BUDGET.as_millis()
    )));
    emit_failed(log, &outcome, campus_domains);
    outcome
}

// ---------------------------------------------------------------------------
// journal 追加（链头推进 + FlushFileBuffers 持久；镜像 system_proxy_journal）
// ---------------------------------------------------------------------------

fn open_journal_for_takeover(dir: &Path) -> Result<WinJournalStore, String> {
    WinJournalStore::open(&JournalPath::from_dir(dir.to_path_buf()))
        .map_err(|e| format!("接管 journal 打开失败（{dir:?}）：{e:?}"))
}

/// 追加一条 `nrpt_foreign_taken_over`（status=planned）记录——journal 前置与
/// 漂移补记共用（完整形状 + restore_hint + 厂商在场注记 + 完成标记字段）。
fn journal_planned(
    store: &mut WinJournalStore,
    dir: &Path,
    target: &NrptTakeoverTarget,
    fingerprint: String,
    supersedes: Option<String>,
    operation_id: &str,
    vendor: &VendorPresence,
) -> Result<(), String> {
    append_takeover_journal(
        store,
        dir,
        &TakeoverJournalPayload {
            kind: "nrpt_foreign_taken_over",
            status: "planned",
            supersedes,
            fingerprint_hex: fingerprint,
            namespace_forms: covered_namespaces_of(target),
            servers: servers_of(target),
            store: target.store.as_str(),
            store_handle: target
                .rule
                .as_ref()
                .map(|rule| rule.name.clone())
                .unwrap_or_default(),
            full_shape: full_shape_of(target),
            restore_hint: restore_hint_of(target),
            operation_id: operation_id.to_owned(),
            vendor_presence: vendor_annotation(vendor),
            taken_at_ms: unix_now_ms(),
            engine_pid: std::process::id(),
        },
    )
}

/// 追加一条接管记录（recover 链头 → append_synced；Corrupt → 失败——绝不在
/// 损坏文件后追加，也不就地隔离（恢复脚本会看到损坏态，由人工处置）。
fn append_takeover_journal(
    store: &mut WinJournalStore,
    dir: &Path,
    payload: &TakeoverJournalPayload,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(payload)
        .map_err(|e| format!("接管 journal payload 编码失败：{e}"))?;
    append_bytes(store, dir, bytes)
}

fn append_outcome_journal(
    store: &mut WinJournalStore,
    dir: &Path,
    payload: &TakeoverOutcomePayload,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(payload)
        .map_err(|e| format!("接管 journal payload 编码失败：{e}"))?;
    append_bytes(store, dir, bytes)
}

fn append_bytes(store: &mut WinJournalStore, dir: &Path, bytes: Vec<u8>) -> Result<(), String> {
    let (seq, prev) = match store.recover() {
        Ok(RecoverOutcome::Clean(records)) | Ok(RecoverOutcome::TornTail { records }) => {
            match records.last() {
                Some(last) => (last.sequence + 1, last.digest),
                None => (0, [0u8; 32]),
            }
        }
        Ok(RecoverOutcome::Corrupt { offset }) => {
            return Err(format!(
                "接管 journal 已损坏（offset {offset}，目录 {dir:?}）：拒绝追加"
            ));
        }
        Err(e) => return Err(format!("接管 journal 读取失败：{e:?}")),
    };
    store
        .append_synced(&JournalRecord::new(seq, prev, bytes))
        .map(|_| ())
        .map_err(|e| format!("接管 journal 追加失败：{e:?}"))
}

// ---------------------------------------------------------------------------
// 目标材料（指纹/形状/恢复提示）
// ---------------------------------------------------------------------------

/// 目标指纹（SHA-256 前 16 字节 hex；材料 = store+形式+服务器集合+形状+句柄）。
/// 与 `nrpt_acceptance_digest` 同构（非安全哈希）：单指纹单次覆盖硬闸与 journal
/// 对线的键。
fn target_fingerprint(target: &NrptTakeoverTarget) -> String {
    let mut servers: Vec<String> = servers_of(target)
        .iter()
        .map(|server| server.trim().to_ascii_lowercase())
        .collect();
    servers.sort();
    servers.dedup();
    let flags = match (&target.rule, &target.entry) {
        (Some(rule), _) => format!(
            "dnssec={},ipsec={},da={}",
            rule.dnssec, rule.ipsec, rule.direct_access
        ),
        (_, Some(entry)) => format!(
            "dnssec={},ipsec=unobservable,da={}",
            entry.dnssec, entry.direct_access
        ),
        _ => "shape=none".to_owned(),
    };
    let handle = target
        .rule
        .as_ref()
        .map(|rule| rule.name.as_str())
        .unwrap_or("");
    let material = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        target.store.as_str(),
        target.form,
        servers.join(","),
        flags,
        handle
    );
    let digest = Sha256::digest(material.as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 目标覆盖的命名空间形式（journal 记录用：本地表规则取其全部命名空间，
/// 有效表条目取条目命名空间）。
fn covered_namespaces_of(target: &NrptTakeoverTarget) -> Vec<String> {
    if let Some(rule) = &target.rule {
        return rule.namespaces.clone();
    }
    target
        .entry
        .as_ref()
        .map(|entry| entry.namespaces.clone())
        .unwrap_or_default()
}

fn servers_of(target: &NrptTakeoverTarget) -> Vec<String> {
    if let Some(rule) = &target.rule {
        return rule.servers.clone();
    }
    target
        .entry
        .as_ref()
        .map(|entry| entry.servers.clone())
        .unwrap_or_default()
}

fn full_shape_of(target: &NrptTakeoverTarget) -> FullShape {
    match (&target.rule, &target.entry) {
        (Some(rule), _) => FullShape {
            dnssec: rule.dnssec,
            ipsec: Some(rule.ipsec),
            direct_access: rule.direct_access,
            ipsec_observable: true,
            encoding: "Utf8WithoutMapping",
        },
        (_, Some(entry)) => FullShape {
            dnssec: entry.dnssec,
            // 有效策略表无 IPsec 投影属性：不可观测项如实置 null，绝不默认
            // false 谎报形状（复审判定 1 补充）。
            ipsec: None,
            direct_access: entry.direct_access,
            ipsec_observable: false,
            encoding: "Utf8WithoutMapping",
        },
        _ => FullShape {
            dnssec: false,
            ipsec: None,
            direct_access: false,
            ipsec_observable: false,
            encoding: "Utf8WithoutMapping",
        },
    }
}

/// `Add-DnsClientNrptRule` 参数集原文（人工恢复依据；`-Restore` 脚本从 journal
/// payload 重建而非直接执行本字符串——字符串仅供人读）。
fn restore_hint_of(target: &NrptTakeoverTarget) -> String {
    fn quote(text: &str) -> String {
        format!("'{}'", text.replace('\'', "''"))
    }
    let namespaces: Vec<String> = covered_namespaces_of(target)
        .iter()
        .map(|namespace| quote(namespace))
        .collect();
    let servers: Vec<String> = servers_of(target)
        .iter()
        .map(|server| quote(server))
        .collect();
    let mut hint = format!(
        "Add-DnsClientNrptRule -Namespace {} -NameServers {}",
        namespaces.join(","),
        servers.join(",")
    );
    if let Some(rule) = &target.rule {
        if !rule.comment.is_empty() {
            hint.push_str(&format!(" -Comment {}", quote(&rule.comment)));
        }
        if rule.dnssec {
            hint.push_str(" -DnsSecValidationRequired");
        }
        if rule.ipsec {
            hint.push_str(" -DnsSecQueryIPsecRequired");
        }
        if rule.direct_access {
            hint.push_str(" -DirectAccessEnabled");
        }
    } else if let Some(entry) = &target.entry {
        if entry.dnssec {
            hint.push_str(" -DnsSecValidationRequired");
        }
        if entry.direct_access {
            hint.push_str(" -DirectAccessEnabled");
        }
        hint.push_str("（注：有效策略表条目无 IPsec 投影与注释属性，恢复不含未知项）");
    }
    hint
}

fn vendor_annotation(vendor: &VendorPresence) -> BTreeMap<String, String> {
    vendor
        .annotation_fields()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// 事件（结构化留痕；自由文本不落 wire）
// ---------------------------------------------------------------------------

fn emit_started(
    log: &LogSink,
    targets: &[NrptTakeoverTarget],
    vendor: &VendorPresence,
    session_servers: &[String],
) {
    let forms: Vec<String> = targets.iter().map(|t| t.form.clone()).collect();
    let stores: Vec<&str> = targets.iter().map(|t| t.store.as_str()).collect();
    let mut fields: Vec<(&str, String)> = vec![
        ("forms", forms.join(",")),
        ("stores", stores.join(",")),
        ("servers", session_servers.join(",")),
    ];
    for (key, value) in &vendor.annotation_fields() {
        fields.push((key, value.clone()));
    }
    let fields_ref: Vec<(&str, &str)> = fields
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    log.emit(
        LogLevel::Info,
        "engine",
        "tunnel.nrpt.foreign.takeover_started",
        "外来 NRPT 规则与本次连接冲突：静默精准接管开始（journal 前置 → 原动作完成；仅精确匹配本次校园域、清单外零接触）",
        &fields_ref,
    );
}

fn emit_noop(log: &LogSink, outcome: &TakeoverOutcome, campus_domains: &[String]) {
    if outcome.rebuilt_forms.is_empty() {
        log.emit(
            LogLevel::Debug,
            "engine",
            "tunnel.nrpt.foreign.takeover_noop",
            "接管目标为空（等效已被接纳/自有孤儿归认领清理/清单外零接触）——零动作",
            &[("campus_domains", &campus_domains.join(","))],
        );
    } else {
        log.emit(
            LogLevel::Warn,
            "engine",
            "tunnel.nrpt.foreign.rebuilt",
            "外来 NRPT 规则指纹在本连接内已覆盖过（单指纹单次覆盖硬闸）：停止覆盖",
            &[("rebuilt_forms", &outcome.rebuilt_forms.join(","))],
        );
    }
}

fn emit_failed(log: &LogSink, outcome: &TakeoverOutcome, campus_domains: &[String]) {
    let Some(failure) = &outcome.failed else {
        return;
    };
    log.emit(
        LogLevel::Error,
        "engine",
        "tunnel.nrpt.foreign.failed",
        "外来 NRPT 规则静默接管失败（fail-closed 保留在异常路径：连接将失败收口；已删除部分可由 journal -Restore 恢复）",
        &[
            ("failure", failure.detail()),
            ("message", failure.message()),
            ("campus_domains", &campus_domains.join(",")),
            ("stores", &outcome.store_reached.join(",")),
        ],
    );
}

// ---------------------------------------------------------------------------
// 集成测试（真子进程/真管道 + 假脚本 + 文件态共享；`nrpt_worker` lifecycle 同款
// 接缝——不触碰真实 NRPT/Dnscache/journal 机器目录）。
// ---------------------------------------------------------------------------
