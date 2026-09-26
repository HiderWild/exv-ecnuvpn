
//! 系统代理 family 的执行层（TK1b；设计 §5.3 + 2026-09-05 账本计划 §4.2/§4.3）。
//!
//! [`crate::system_proxy_family`] 提供纯决策（`decide`/`compute_restore`/
//! `decide_replay`），本模块把决策接到副作用边界上：
//!
//! - **plan/commit 拆分（§4.2 冻结执行序「J50-WAL：无持久意图则无效果」）**：
//!   [`plan_system_proxy_apply`] 只产出计划（计算写入态与指纹；PAC 端点先可服务），
//!   **注册表零写入**——engine 接线在 plan 与 commit 之间插入 journal 落盘
//!   （记账失败 → 降级「未豁免」，跳过 commit，连接继续 best-effort）；
//!   [`commit_system_proxy_apply`] 消费计划执行注册表写入 + 广播。
//! - [`apply_system_proxy_with_pac`]：plan+commit 薄包装（无账本；既有调用方/
//!   测试过渡），行为与拆分前等价。
//! - [`replay_system_proxy_step`]：崩溃回放真机入口——三态裁决
//!   （`decide_replay`）+ 精确还原（存在 → 字节级写回；原不存在 → 删除）+ 广播；
//!   `AlreadyClean`/`SkipForfeit` 零写入；还原硬失败返回
//!   [`SystemProxyReplayOutcome::HardFailure`]（调用方保留记录待下次重试）。
//! - [`restore_from_current`] / [`restore_system_proxy_step`]：teardown 的两态
//!   compare-and-restore（第三方改动 typed skip，绝不强写）。
//!
//! Win32 注册表写回/删除的直调层走 [`crate::system_proxy_override`] 既有实现；
//! journal 落盘由 engine 接线以 `SystemProxyFamilyStep::to_payload`（v0x03 自带
//! 写入指纹）经 `journal_store::append_synced` 完成——本模块只产出步骤记录、指纹
//! 与计划。

use crate::native_error::{NativeError, NativeErrorKind};
use crate::system_proxy::{RawInternetSettings, RawValue, capture_for_user, snapshot_from_raw};
use crate::system_proxy_family::{
    ReplayDecision, RestoreDecision, RestoreOp, SystemProxyFamilyStep, ValueName, compute_restore,
    decide, decide_replay,
};
use std::time::Instant;

/// 五值的注册表指纹（compare-and-restore 的比较输入）：按固定顺序拼接各值的
/// tag + 内容字节。任何第三方对五值任一的改动都会改变指纹。
#[must_use]
pub fn fingerprint_prestate(raw: &RawInternetSettings) -> Vec<u8> {
    let mut buf = Vec::new();
    for value in [
        &raw.proxy_enable,
        &raw.proxy_server,
        &raw.proxy_override,
        &raw.auto_config_url,
        &raw.auto_detect,
    ] {
        match value {
            RawValue::Absent => buf.push(0),
            RawValue::Dword(v) => {
                buf.push(1);
                buf.extend_from_slice(&v.to_le_bytes());
            }
            RawValue::Other { r#type, data } => {
                buf.push(2);
                buf.extend_from_slice(&r#type.to_le_bytes());
                buf.extend_from_slice(data);
            }
            // REG_SZ 的指纹取其 UTF-16 单元字节（与 capture 的去尾 NUL 语义一致，
            // 同一文本恒得同一指纹）。
            RawValue::Sz(s) => {
                buf.push(3);
                let units: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
                buf.extend_from_slice(&units);
            }
        }
    }
    buf
}

/// Manual 写入态的确定性重算：`written = prestate{ProxyOverride=Sz(merged)}`。
#[must_use]
fn prestate_with_override(prestate: &RawInternetSettings, merged: &str) -> RawInternetSettings {
    let mut written = prestate.clone();
    written.proxy_override = RawValue::Sz(merged.to_owned());
    written
}

/// apply 阶段系统代理 family 步骤的执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemProxyApplyOutcome {
    /// Disabled：零动作零账本（拍板结论 4）。
    SkippedNoProxy,
    /// Automatic / WPAD：typed skip（v1 不接线 PAC）。
    PacDetectedSkip,
    /// Manual/Mixed：已合并写入 + 广播。携带 journal 落盘所需的步骤记录与
    /// 「写入后」指纹（还原时的 compare 基准）。
    Applied {
        /// apply 成功前由调用方落盘的步骤记录。
        step: SystemProxyFamilyStep,
        /// 写入后立即重读五值得到的指纹（= 我们写入状态的基准指纹）。
        written_fingerprint: Vec<u8>,
    },
}

/// 执行 apply 阶段的系统代理 family 步骤（真机入口；含 PAC 开环包装）。
///
/// admission-first：`capture_for_user` 或解析失败直接返回 `Err`——此时零效果，
/// 调用方的整计划 admission 语义（W22-I）据此放弃整个 plan。
///
/// # Errors
///
/// 注册表不可读、快照自洽性破坏、或写入/广播失败时传播类型化 [`NativeError`]。
pub fn apply_system_proxy_step(
    originating_sid: &str,
    desired_entries: &[String],
) -> Result<SystemProxyApplyOutcome, NativeError> {
    let prestate = capture_for_user(originating_sid)?;
    apply_from_prestate(originating_sid, desired_entries, prestate)
}

/// apply 的**带 PAC 开环包装**入口（设计 §7-3 二次修订；plan+commit 薄包装，无账本）：
/// Automatic（有 `AutoConfigURL`）时，一次性下载原文 → 文本包装 → 起 loopback 端点 →
/// 把 `AutoConfigURL` 改指端点 → 广播。任一步失败 **fail-closed** 回退为
/// 「仅提示」（[`SystemProxyApplyResult::Pac`] 且 `endpoint: None`），绝不静默
/// 半改写注册表。开环语义：注入后不跟踪，代理软件改写即失效、重连刷新。
///
/// 本入口是 [`plan_system_proxy_apply`] + [`commit_system_proxy_apply`] 的直连薄
/// 包装（无 journal 落盘点）——engine 记账接线必须在两步之间插入 `append_synced`
/// （§4.2：无持久意图则无效果），不得使用本包装。
///
/// 零扰动纪律（设计 §5.6）：端点先可服务 → 再改注册表 → 再广播。
pub fn apply_system_proxy_with_pac(
    originating_sid: &str,
    desired_entries: &[String],
) -> Result<SystemProxyApplyResult, NativeError> {
    let plan = plan_system_proxy_apply(originating_sid, desired_entries)?;
    if !plan.registry_effect {
        // 零效果：Disabled（SkipNoOp 形态）或 PAC fail-closed 仅提示（endpoint None）。
        if plan.step.pac_detected {
            return Ok(SystemProxyApplyResult::Pac {
                written_fingerprint: plan.step.written_fingerprint.clone(),
                step: plan.step,
                endpoint: None,
            });
        }
        return Ok(SystemProxyApplyResult::SkippedNoProxy);
    }
    let step = plan.step.clone();
    let written_fingerprint = plan.step.written_fingerprint.clone();
    let endpoint_arc = plan.endpoint.clone();
    commit_system_proxy_apply(originating_sid, plan)?;
    // commit 已消费计划（其 Arc 克隆随之释放）：此处 unwrap 回独占所有权供调用方
    // teardown 持有（Drop 即关），与拆分前「调用方持有端点」行为等价。
    let endpoint = endpoint_arc.and_then(|e| std::sync::Arc::try_unwrap(e).ok());
    Ok(match endpoint {
        Some(endpoint) => SystemProxyApplyResult::Pac {
            step,
            written_fingerprint,
            endpoint: Some(endpoint),
        },
        None => SystemProxyApplyResult::Applied {
            step,
            written_fingerprint,
        },
    })
}

/// apply 计划（§4.7 冻结）：plan 阶段的唯一产物——注册表**零写入**，效果与端点
/// 状态由 [`commit_system_proxy_apply`] 落地。`registry_effect == false` ⇒ 不记账
/// 不 commit（零效果形态：Disabled / 纯 WPAD 提示 / merged==原值 / PAC fail-closed）。
///
/// 端点生命周期（零扰动 §5.6）：`plan` 起端点并先保证可服务；`commit` 只写注册表
/// 不动端点；端点所有权归调用方（teardown 还原注册表后再 drop 关闭）。因此端点以
/// `Arc` 共享——`commit` 按值消费计划后调用方仍可持引用做还原后关闭（实现级偏差，
/// 语义与 §4.7 字段清单一致：字段集不变，仅端点所有权可共享）。
pub struct SystemProxyApplyPlan {
    /// 步骤记录（v0x03，自带写入后指纹）：记账 payload 即 `step.to_payload()`。
    pub step: SystemProxyFamilyStep,
    /// Manual/Mixed 的合并结果（commit 写入 `ProxyOverride` 的值；PAC/零效果为 None）。
    pub merged: Option<String>,
    /// PAC loopback 端点（plan 阶段已 spawn 且可服务；Manual/零效果为 None）。
    pub endpoint: Option<std::sync::Arc<crate::system_proxy_pac::PacEndpoint>>,
    /// false ⇒ 不记账不 commit（零动作零账本）。
    pub registry_effect: bool,
}

/// apply 计划入口（真机）：capture → decide → 产出 [`SystemProxyApplyPlan`]。
///
/// admission-first：`capture_for_user` 或解析失败直接返回 `Err`——此时零效果，
/// 调用方的整计划 admission 语义（W22-I）据此放弃整个 plan。
///
/// # Errors
///
/// 注册表不可读或快照自洽性破坏时传播类型化 [`NativeError`]。
pub fn plan_system_proxy_apply(
    originating_sid: &str,
    desired_entries: &[String],
) -> Result<SystemProxyApplyPlan, NativeError> {
    plan_system_proxy_apply_observed(originating_sid, desired_entries, &|_| {})
}

/// 系统代理阶段观测；仅固定分类、数量和错误码，不包含注册表原值、PAC URL 或脚本。
#[derive(Debug, Clone)]
pub struct SystemProxyDiagnostic {
    pub stage: &'static str,
    pub outcome: &'static str,
    pub elapsed_ms: u64,
    pub error: Option<(NativeErrorKind, u32)>,
    pub reason: Option<&'static str>,
}

/// 从已知 PAC 错误提取固定分类，绝不把原始 URL、脚本或系统错误正文送入日志。
#[must_use]
pub fn safe_system_proxy_error_reason(error: &NativeError) -> Option<&'static str> {
    let message = error.message.as_str();
    if let Some(reason) = message.strip_prefix("pac_script_unwrappable: ") {
        return Some(match reason {
            "empty script" => "empty_script",
            "script already carries the EXV wrapper alias" => "already_wrapped",
            "no `function FindProxyForURL` declaration found" => "missing_entrypoint",
            "bracket imbalance in assembled script" => "unbalanced_brackets",
            _ if reason.split_once(' ').is_some_and(|(count, rest)| {
                count.parse::<usize>().is_ok_and(|count| count > 1)
                    && rest == "`function FindProxyForURL` declarations found, exactly 1 required"
            }) =>
            {
                "multiple_entrypoints"
            }
            _ => "unclassified_pac_error",
        });
    }
    for (prefix, reason) in [
        ("pac_fetch_https_unsupported:", "https_unsupported"),
        ("pac_fetch_unsupported_url:", "unsupported_url"),
        ("pac_fetch_connect_failed:", "connect_failed"),
        ("pac_fetch_request_write_failed:", "request_write_failed"),
        ("pac_fetch_timeout:", "timeout"),
        ("pac_fetch_read_failed:", "read_failed"),
        ("pac_fetch_too_large:", "response_too_large"),
        ("pac_fetch_malformed_response:", "malformed_response"),
        ("pac_fetch_invalid_utf8:", "invalid_utf8"),
        ("pac_fetch_http_status_", "http_status"),
        ("pac-endpoint-spawn:", "endpoint_spawn_failed"),
    ] {
        if message.starts_with(prefix) {
            return Some(reason);
        }
    }
    if message.starts_with("pac_") || message.starts_with("pac-") {
        Some("unclassified_pac_error")
    } else {
        None
    }
}

fn observe_proxy_stage(
    observe: &dyn Fn(SystemProxyDiagnostic),
    stage: &'static str,
    outcome: &'static str,
    started: Instant,
    error: Option<&NativeError>,
) {
    observe(SystemProxyDiagnostic {
        stage,
        outcome,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error: error.map(|e| (e.kind, e.code)),
        reason: error.and_then(safe_system_proxy_error_reason),
    });
}

/// 生产观测入口。观测不改变代理计划、注册表效果和 best-effort 降级语义。
///
/// # Errors
/// 与 `plan_system_proxy_apply` 一致。
pub fn plan_system_proxy_apply_observed(
    originating_sid: &str,
    desired_entries: &[String],
    observe: &dyn Fn(SystemProxyDiagnostic),
) -> Result<SystemProxyApplyPlan, NativeError> {
    let started = Instant::now();
    let captured = capture_for_user(originating_sid);
    observe_proxy_stage(
        observe,
        "capture",
        if captured.is_ok() { "ok" } else { "failed" },
        started,
        captured.as_ref().err(),
    );
    plan_system_proxy_apply_from_prestate_observed(
        originating_sid,
        desired_entries,
        captured?,
        observe,
    )
}

/// [`plan_system_proxy_apply`] 的可注入内核：prestate 由调用方给定（单测不触真机）。
///
/// plan 阶段零注册表效果：Manual 只计算 `written = prestate{ProxyOverride=Sz(merged)}`
/// 与其指纹；PAC 在端点可服务后计算 `written = prestate{AutoConfigURL=Sz(served_url)}`
/// 与其指纹。注册表写入一律推迟到 [`commit_system_proxy_apply`]。
///
/// # Errors
///
/// 快照自洽性破坏或合并条目非法（含分号/空/超长）传播类型化错误。
pub fn plan_system_proxy_apply_from_prestate(
    originating_sid: &str,
    desired_entries: &[String],
    prestate: RawInternetSettings,
) -> Result<SystemProxyApplyPlan, NativeError> {
    plan_system_proxy_apply_from_prestate_observed(
        originating_sid,
        desired_entries,
        prestate,
        &|_| {},
    )
}

fn plan_system_proxy_apply_from_prestate_observed(
    originating_sid: &str,
    desired_entries: &[String],
    prestate: RawInternetSettings,
    observe: &dyn Fn(SystemProxyDiagnostic),
) -> Result<SystemProxyApplyPlan, NativeError> {
    let started = Instant::now();
    let snapshot = snapshot_from_raw(&prestate)?;
    match decide(&snapshot, desired_entries)? {
        crate::system_proxy_family::FamilyAction::SkipNoOp => {
            observe_proxy_stage(observe, "plan", "disabled", started, None);
            // Disabled：零动作零账本（拍板结论 4）。
            Ok(zero_effect_plan(
                originating_sid,
                desired_entries,
                prestate,
                false,
            ))
        }
        crate::system_proxy_family::FamilyAction::MergeAndWrite { .. } => {
            let merged = decide_merged(&snapshot, desired_entries)?;
            // 零效果合并：期望条目已在豁免中 → 写入是冗余，零动作零账本（§4.2）。
            let original = snapshot.bypass_entries.join(";");
            if merged == original {
                observe_proxy_stage(observe, "plan", "already_exempted", started, None);
                return Ok(zero_effect_plan(
                    originating_sid,
                    desired_entries,
                    prestate,
                    false,
                ));
            }
            observe_proxy_stage(observe, "plan", "merge_bypass", started, None);
            let written_fingerprint =
                fingerprint_prestate(&prestate_with_override(&prestate, &merged));
            Ok(SystemProxyApplyPlan {
                step: SystemProxyFamilyStep {
                    prestate,
                    desired_entries: desired_entries.to_vec(),
                    originating_sid: originating_sid.to_owned(),
                    pac_detected: false,
                    written_fingerprint,
                },
                merged: Some(merged),
                endpoint: None,
                registry_effect: true,
            })
        }
        crate::system_proxy_family::FamilyAction::PacDetectedSkip => {
            // 取 AutoConfigURL 原文（缺失/非文本 → 无包装对象，fail-closed 仅提示）。
            let url = match &prestate.auto_config_url {
                RawValue::Sz(u) if !u.is_empty() => u.clone(),
                _ => {
                    observe_proxy_stage(observe, "plan", "wpad_without_pac_url", started, None);
                    return Ok(zero_effect_plan(
                        originating_sid,
                        desired_entries,
                        prestate,
                        true,
                    ));
                }
            };
            // plan 阶段完成「fetch → wrap → spawn（端点先可服务）」；注册表零写入
            // ——`AutoConfigURL` 改指推迟到 commit（§4.2 零扰动序）。任一步失败
            // fail-closed 回退仅提示，不报错不阻断隧道。
            let endpoint = match try_pac_spawn(&url, desired_entries, observe) {
                Ok(endpoint) => endpoint,
                Err(e) => {
                    observe_proxy_stage(observe, "plan", "pac_degraded", started, Some(&e));
                    return Ok(zero_effect_plan(
                        originating_sid,
                        desired_entries,
                        prestate,
                        true,
                    ));
                }
            };
            let served_url = format!("http://127.0.0.1:{}/proxy.pac", endpoint.port());
            let mut plan = plan_pac_from_prestate_with_url(
                originating_sid,
                desired_entries,
                prestate,
                &served_url,
            );
            plan.endpoint = Some(std::sync::Arc::new(endpoint));
            observe_proxy_stage(observe, "plan", "pac_ready", started, None);
            Ok(plan)
        }
    }
}

/// PAC plan 的可注入内核（单测不触网络/注册表）：端点已就绪、`served_url` 由调用方
/// 给定（真机路径经 `try_pac_spawn`）。产出 `registry_effect = true` 的计划：
/// `written = prestate{AutoConfigURL=Sz(served_url)}`，写入后指纹随计划落账。
#[must_use]
pub fn plan_pac_from_prestate_with_url(
    originating_sid: &str,
    desired_entries: &[String],
    prestate: RawInternetSettings,
    served_url: &str,
) -> SystemProxyApplyPlan {
    let mut written = prestate.clone();
    written.auto_config_url = RawValue::Sz(served_url.to_owned());
    let written_fingerprint = fingerprint_prestate(&written);
    SystemProxyApplyPlan {
        step: SystemProxyFamilyStep {
            prestate,
            desired_entries: desired_entries.to_vec(),
            originating_sid: originating_sid.to_owned(),
            pac_detected: true,
            written_fingerprint,
        },
        merged: None,
        endpoint: None,
        registry_effect: true,
    }
}

/// 零效果计划（fail-closed / 零动作零账本）：`registry_effect = false`，无合并值、
/// 无端点；`written_fp = prestate 指纹`（写入未生效语义）。`pac_detected` 标记
/// PAC/WPAD 形态（供薄包装区分 `Pac{endpoint:None}` 与 `SkippedNoProxy`）。
#[must_use]
fn zero_effect_plan(
    originating_sid: &str,
    desired_entries: &[String],
    prestate: RawInternetSettings,
    pac_detected: bool,
) -> SystemProxyApplyPlan {
    SystemProxyApplyPlan {
        step: SystemProxyFamilyStep {
            written_fingerprint: fingerprint_prestate(&prestate),
            prestate,
            desired_entries: desired_entries.to_vec(),
            originating_sid: originating_sid.to_owned(),
            pac_detected,
        },
        merged: None,
        endpoint: None,
        registry_effect: false,
    }
}

/// commit 阶段（§4.2 效果点）：消费计划执行注册表写入 + 广播。
///
/// Manual/Mixed → `write_override_for_user`（内含 WinINET 广播）；Automatic/PAC →
/// `AutoConfigURL` 改指 plan 端点 + 显式广播（零扰动 §5.6：端点已在 plan 阶段可
/// 服务，此处只做注册表写入与刷新）。`registry_effect == false` 的计划是防御性
/// no-op（调用方本就不应记账或 commit）。
///
/// # Errors
///
/// 注册表写入/广播失败传播类型化 [`NativeError`]（调用方按降级处理；账本记录
/// 保留，下次回放按 `AlreadyClean`/`SkipForfeit` 自然收敛）。
pub fn commit_system_proxy_apply(sid: &str, plan: SystemProxyApplyPlan) -> Result<(), NativeError> {
    if !plan.registry_effect {
        return Ok(());
    }
    if let Some(merged) = &plan.merged {
        crate::system_proxy_override::write_override_for_user(sid, merged)?;
        return Ok(());
    }
    let endpoint = plan.endpoint.as_ref().ok_or_else(|| {
        NativeError::from_win32(87, "system_proxy_family_exec: PAC commit 缺端点")
    })?;
    let served_url = format!("http://127.0.0.1:{}/proxy.pac", endpoint.port());
    crate::system_proxy_override::write_back_value_for_user(
        sid,
        ValueName::AutoConfigUrl,
        &RawValue::Sz(served_url),
    )?;
    crate::system_proxy_override::broadcast_wininet_refresh();
    Ok(())
}

/// 一次性 PAC 开环包装的 plan 段：下载 → 包装 → 起端点（**注册表零写入**——写入
/// 在 [`commit_system_proxy_apply`]）。
///
/// 任何一步失败返回 `Err`——调用方已按 fail-closed 回退（零效果计划，端点未 spawn）。
fn try_pac_spawn(
    pac_url: &str,
    desired_entries: &[String],
    observe: &dyn Fn(SystemProxyDiagnostic),
) -> Result<crate::system_proxy_pac::PacEndpoint, NativeError> {
    use std::sync::Arc;
    let started = Instant::now();
    let original = crate::system_proxy_pac::fetch_pac_script(pac_url);
    observe_proxy_stage(
        observe,
        "pac_fetch",
        if original.is_ok() { "ok" } else { "failed" },
        started,
        original.as_ref().err(),
    );
    let started = Instant::now();
    let wrapped = crate::system_proxy_pac::wrap_pac_script(&original?, desired_entries);
    observe_proxy_stage(
        observe,
        "pac_wrap",
        if wrapped.is_ok() { "ok" } else { "failed" },
        started,
        wrapped.as_ref().err(),
    );
    let started = Instant::now();
    let endpoint = crate::system_proxy_pac::PacEndpoint::spawn(Arc::new(wrapped?))
        .map_err(|e| NativeError::from_win32(87, &format!("pac-endpoint-spawn:{e}")));
    observe_proxy_stage(
        observe,
        "pac_endpoint",
        if endpoint.is_ok() { "ready" } else { "failed" },
        started,
        endpoint.as_ref().err(),
    );
    endpoint
}

/// apply 结果（含 PAC 端点持有，供 teardown 关闭；不复用 [`SystemProxyApplyOutcome`]
/// 的 Clone/Eq 派生——端点持有线程句柄不可克隆）。
#[derive(Debug)]
pub enum SystemProxyApplyResult {
    /// Disabled：零动作零账本（拍板结论 4）。
    SkippedNoProxy,
    /// Manual/Mixed：已合并写入 + 广播。
    Applied {
        /// journal 落盘所需的步骤记录。
        step: SystemProxyFamilyStep,
        /// 写入后指纹（还原 compare 基准）。
        written_fingerprint: Vec<u8>,
    },
    /// Automatic：PAC 开环包装（成功 `endpoint: Some`；失败回退提示 `None`）。
    Pac {
        /// journal 落盘所需的步骤记录。
        step: SystemProxyFamilyStep,
        /// 改指后指纹（还原 compare 基准）。
        written_fingerprint: Vec<u8>,
        /// loopback 包装端点（Drop 即关；teardown 还原注册表后再 drop）。
        endpoint: Option<crate::system_proxy_pac::PacEndpoint>,
    },
}

/// [`apply_system_proxy_step`] 的可注入内核：prestate 由调用方给定（单测不触真机）。
pub fn apply_from_prestate(
    originating_sid: &str,
    desired_entries: &[String],
    prestate: RawInternetSettings,
) -> Result<SystemProxyApplyOutcome, NativeError> {
    let snapshot = snapshot_from_raw(&prestate)?;
    match decide(&snapshot, desired_entries)? {
        crate::system_proxy_family::FamilyAction::SkipNoOp => {
            Ok(SystemProxyApplyOutcome::SkippedNoProxy)
        }
        crate::system_proxy_family::FamilyAction::PacDetectedSkip => {
            Ok(SystemProxyApplyOutcome::PacDetectedSkip)
        }
        crate::system_proxy_family::FamilyAction::MergeAndWrite { .. } => {
            // decide 已算出 merged；此处经真机 leaf 写入 + 广播。广播 skip
            // （wininet 缺失，Ok(false)）仍是成功写入，设置重启后生效，不阻断。
            let merged = decide_merged(&snapshot, desired_entries)?;
            let _broadcasted =
                crate::system_proxy_override::write_override_for_user(originating_sid, &merged)?;
            let written = capture_for_user(originating_sid)?;
            let written_fingerprint = fingerprint_prestate(&written);
            Ok(SystemProxyApplyOutcome::Applied {
                // v0x03：步骤记录自带写入后指纹（与本 outcome 的指纹同源同值）。
                step: SystemProxyFamilyStep {
                    prestate,
                    desired_entries: desired_entries.to_vec(),
                    originating_sid: originating_sid.to_owned(),
                    pac_detected: false,
                    written_fingerprint: written_fingerprint.clone(),
                },
                written_fingerprint,
            })
        }
    }
}

/// 取出合并结果（`decide` 的 MergeAndWrite 变体重放；避免 FamilyAction 携带
/// String 导致的 match 借用纠缠——纯读函数，成本可忽略）。
fn decide_merged(
    snapshot: &crate::system_proxy::SystemProxySnapshot,
    desired: &[String],
) -> Result<String, NativeError> {
    match decide(snapshot, desired)? {
        crate::system_proxy_family::FamilyAction::MergeAndWrite { merged } => Ok(merged),
        _ => Err(NativeError {
            kind: NativeErrorKind::Protocol,
            code: 87,
            message: "system_proxy_family_exec: 决策重放变体不符".to_owned(),
        }),
    }
}

/// restore 阶段的结果（teardown/recovery 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemProxyRestoreOutcome {
    /// 指纹相等：还原操作序列就绪/已执行（写回 N 个 + 删除 M 个）。
    Restored {
        /// 字节级写回的原存在值个数。
        write_backs: usize,
        /// 删除的原本不存在值个数。
        deletes: usize,
    },
    /// 指纹不等：第三方中途改动，typed skip（调用方记 restore_failures）。
    TypedSkip,
}

/// 执行 restore 阶段系统代理 family 步骤的比较内核（可注入，单测不触真机）。
///
/// `current` 为 teardown 时重读的五值；`written_fingerprint` 为 apply 后记录的
/// 基准指纹。相等 → [`compute_restore`] 的精确还原操作序列；不等 → typed skip。
#[must_use]
pub fn plan_restore(
    prestate: &RawInternetSettings,
    current: &RawInternetSettings,
    written_fingerprint: &[u8],
) -> SystemProxyRestoreOutcome {
    let current_fp = fingerprint_prestate(current);
    match compute_restore(prestate, &current_fp, written_fingerprint) {
        RestoreDecision::TypedSkip => SystemProxyRestoreOutcome::TypedSkip,
        RestoreDecision::Restore { ops } => SystemProxyRestoreOutcome::Restored {
            write_backs: ops
                .iter()
                .filter(|op| matches!(op, RestoreOp::WriteBack { .. }))
                .count(),
            deletes: ops
                .iter()
                .filter(|op| matches!(op, RestoreOp::DeleteValue { .. }))
                .count(),
        },
    }
}

/// teardown 便捷入口：从 journal 步骤记录 + 当前真机状态还原（真机入口）。
///
/// compare-and-restore：当前值指纹 == 写入后指纹才执行精确还原（存在 → 字节级
/// 写回；原不存在 → 删除该值）；第三方改动是 [`SystemProxyRestoreOutcome::TypedSkip`]，
/// 绝不强写（设计 §5.3 / 拍板结论 3）。
///
/// # Errors
///
/// 注册表操作失败传播；第三方改动不是错误（typed skip）。
pub fn restore_system_proxy_step(
    sid: &str,
    step: &SystemProxyFamilyStep,
    written_fingerprint: &[u8],
) -> Result<SystemProxyRestoreOutcome, NativeError> {
    let current = capture_for_user(sid)?;
    let current_fp = fingerprint_prestate(&current);
    match compute_restore(&step.prestate, &current_fp, written_fingerprint) {
        RestoreDecision::TypedSkip => Ok(SystemProxyRestoreOutcome::TypedSkip),
        RestoreDecision::Restore { .. } => {
            let (write_backs, deletes) =
                execute_restore_ops_with(sid, &step.prestate, &write_back_real, &delete_real)?;
            crate::system_proxy_override::broadcast_wininet_refresh();
            Ok(SystemProxyRestoreOutcome::Restored {
                write_backs,
                deletes,
            })
        }
    }
}

/// 真机写回 leaf（注入 seam 的实现）。
fn write_back_real(sid: &str, value_name: ValueName, value: &RawValue) -> Result<(), NativeError> {
    crate::system_proxy_override::write_back_value_for_user(sid, value_name, value)
}

/// 真机删除 leaf（注入 seam 的实现）。
fn delete_real(sid: &str, value_name: ValueName) -> Result<(), NativeError> {
    crate::system_proxy_override::delete_value_for_user(sid, value_name)
}

/// 五值精确还原操作执行（Restore 的真机效果段；Manual 与 replay 共用）：prestate
/// 存在的值字节级写回、原本不存在的值删除，返回 `(写回数, 删除数)`。任一 leaf 失败
/// 即刻传播（硬失败——调用方保留账本记录待重试）。
fn execute_restore_ops_with<WF, DF>(
    sid: &str,
    prestate: &RawInternetSettings,
    write_back: &WF,
    delete: &DF,
) -> Result<(usize, usize), NativeError>
where
    WF: Fn(&str, ValueName, &RawValue) -> Result<(), NativeError>,
    DF: Fn(&str, ValueName) -> Result<(), NativeError>,
{
    let fields = [
        (ValueName::ProxyEnable, &prestate.proxy_enable),
        (ValueName::ProxyServer, &prestate.proxy_server),
        (ValueName::ProxyOverride, &prestate.proxy_override),
        (ValueName::AutoConfigUrl, &prestate.auto_config_url),
        (ValueName::AutoDetect, &prestate.auto_detect),
    ];
    let mut write_backs = 0usize;
    let mut deletes = 0usize;
    for (value_name, value) in fields {
        match value {
            RawValue::Absent => {
                delete(sid, value_name)?;
                deletes += 1;
            }
            _ => {
                write_back(sid, value_name, value)?;
                write_backs += 1;
            }
        }
    }
    Ok((write_backs, deletes))
}

/// 崩溃回放三态结果（§4.3；teardown 复用 [`SystemProxyRestoreOutcome`] 的两态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemProxyReplayOutcome {
    /// 注册表仍处写入态 → 已精确还原（写回 N + 删除 M）并广播。
    Restored {
        /// 字节级写回的原存在值个数。
        write_backs: usize,
        /// 删除的原本不存在值个数。
        deletes: usize,
    },
    /// 注册表已等于 prestate（含退化全等）→ 零写入，幂等清账即可。
    AlreadyClean,
    /// 用户手工改动等指纹不符 → 弃权：零写入，记 restore_failures 日志后清账。
    SkipForfeit,
    /// capture 或还原写入硬失败 → **记录不得被调用方清除**（保留待下次重试）。
    HardFailure,
}

/// 崩溃回放真机入口（§4.3 冻结三态）：capture 当前五值 → `decide_replay` 裁决 →
/// `Restore` 则精确还原 + 广播；`AlreadyClean`/`SkipForfeit` 零写入。
///
/// 硬失败（capture 失败或任一还原 leaf 失败）→
/// [`SystemProxyReplayOutcome::HardFailure`]——调用方**保留**账本记录，下次重放；
/// 三态中任何非硬失败结果调用方都可清账（compact 严格后缀）。
#[must_use]
pub fn replay_system_proxy_step(
    sid: &str,
    step: &SystemProxyFamilyStep,
) -> SystemProxyReplayOutcome {
    replay_system_proxy_step_with(sid, step, capture_for_user, write_back_real, delete_real)
}

/// [`replay_system_proxy_step`] 的可注入内核（单测不触真机；§5.1 测试 7-8 的 seam）。
#[must_use]
pub fn replay_system_proxy_step_with<CF, WF, DF>(
    sid: &str,
    step: &SystemProxyFamilyStep,
    capture: CF,
    write_back: WF,
    delete: DF,
) -> SystemProxyReplayOutcome
where
    CF: Fn(&str) -> Result<RawInternetSettings, NativeError>,
    WF: Fn(&str, ValueName, &RawValue) -> Result<(), NativeError>,
    DF: Fn(&str, ValueName) -> Result<(), NativeError>,
{
    let current = match capture(sid) {
        Ok(current) => current,
        Err(_) => return SystemProxyReplayOutcome::HardFailure,
    };
    let current_fp = fingerprint_prestate(&current);
    let prestate_fp = fingerprint_prestate(&step.prestate);
    match decide_replay(&prestate_fp, &step.written_fingerprint, &current_fp) {
        ReplayDecision::AlreadyClean => SystemProxyReplayOutcome::AlreadyClean,
        ReplayDecision::SkipForfeit => SystemProxyReplayOutcome::SkipForfeit,
        ReplayDecision::Restore => {
            match execute_restore_ops_with(sid, &step.prestate, &write_back, &delete) {
                Ok((write_backs, deletes)) => {
                    crate::system_proxy_override::broadcast_wininet_refresh();
                    SystemProxyReplayOutcome::Restored {
                        write_backs,
                        deletes,
                    }
                }
                Err(_) => SystemProxyReplayOutcome::HardFailure,
            }
        }
    }
}
