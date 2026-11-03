//! 预检域分类纯函数：把各域探测到的原始行映射为「发现 + 处置策略」。
//!
//! 全部为无副作用纯函数（注入单测）；探测（读）在 [`super`] 的域探针里，
//! 清理（写）在清理执行器里——**本模块只做判定，绝不触碰系统**。
//!
//! 处置策略（计划 §三 / 2026-09-27 任务书）：
//! - **`Cleanup`**：自有残留，本轮预检删除（NRPT 自有前缀规则 / `NetMgmt` 死
//!   接口路由 / 指向自有本地端口的系统代理）。
//! - **`ReportOnly`**：只上报不动手（外来 NRPT 冲突、活接口 bypass 形状路由、
//!   非自有回环代理、接口 DNS=校园解析器、journal 遗留）。语义细化（2026-10-08
//!   计划阶段 1）：外来 NRPT **未授权不动**——包括「等效接纳」路径：外来规则与
//!   本次连接配置完全等效时仅标记 external-provided、跳过自有添加，连接照常；
//!   绝不删除、绝不期待它在 teardown 时消失（防死锁约束：清理只认 `EXV-NRPT:`
//!   自有前缀，规则不在=成功，全部幂等）。
//!
//!   语义再细化（2026-10-09 用户裁定 + D8 阶段 2）：外来 NRPT 的处置升级为
//!   **静默精准覆盖**——连接门路径对本域发现仅报告放行；T3 装配段以会话实际
//!   `campus_dns_servers` 权威判定后，对非等效外来覆盖执行「journal 前置 → 删除 →
//!   验证 → 重建即停」（[`takeover_targets`] 是判定纯函数，执行器在
//!   `nrpt_takeover.rs`）。最小必要两条推论由判定结构保证：等效形式绝不覆盖
//!   （走 external-provided 接纳）；清单之外与自有前缀（`EXV-NRPT:`）覆盖的
//!   形式绝不进入接管目标（自有孤儿归认领清理域）。
//! - **`Manual`**：发现归自有但现无安全删除路径（孤儿 Wintun 网卡——wintun 0.14.1
//!   只有创建者 close 才移除 adapter，预检进程不是创建者）。

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use exv_vpn_win32_resource::proxy_tun::AdapterRecord;
use exv_vpn_win32_resource::routes::{BYPASS_ROUTE_METRIC, BYPASS_ROUTE_PREFIX};
use exv_vpn_win32_resource::system_proxy::{ProxyEndpoint, SystemProxySnapshot};

use super::probe_child::{PsDnsIface, PsNrptPolicyEntry, PsNrptRule, PsRouteRow};
use super::{Finding, FindingPolicy};

/// NRPT owner 注释前缀（与 `nrpt_worker.ps1` 写入的 `EXV-NRPT:<guid>` 一致）。
pub(crate) const NRPT_OWNER_PREFIX: &str = "EXV-NRPT:";

/// 已知校园接口 DNS 解析器集合（任务书给定：{202.120.80.2, 202.120.81.2}）。
pub(crate) const CAMPUS_DNS_SERVERS: [&str; 2] = ["202.120.80.2", "202.120.81.2"];

/// 回环主机名集合（系统代理端点的本机形态判定）。
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// loopback 伪接口索引（`GetAdaptersAddresses` 不枚举它，但始终存在）。
pub(crate) const LOOPBACK_IFINDEX: u32 = 1;

// ---------------------------------------------------------------------------
// NRPT 域
// ---------------------------------------------------------------------------

/// 校园域 → NRPT 命名空间形式集（每个域展开为不带点 + 带点两种形式，小写去重，
/// 保序）。
///
/// **单一实现**（防两表漂移的锚点）：`nrpt_worker.ps1` 的形式展开（domain +
/// `'.' + domain`）、本模块的等效判定、`classify_nrpt` 的校园域匹配全部以此
/// 规则为准——Rust 侧判定/消费的命名空间一律先经本函数规范化。
#[must_use]
pub(crate) fn expand_campus_forms(campus_domains: &[String]) -> Vec<String> {
    let mut forms = Vec::with_capacity(campus_domains.len() * 2);
    for domain in campus_domains {
        let canonical = domain.trim().to_ascii_lowercase();
        if canonical.is_empty() {
            continue;
        }
        let dotted = if canonical.starts_with('.') {
            canonical.clone()
        } else {
            format!(".{canonical}")
        };
        for form in [canonical, dotted] {
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
    }
    forms
}

/// 服务器集合规范化：逐个按 `IpAddr` 解析取规范形态（IPv6 压缩小写等），
/// 序无关集合相等。任一不可解析 → `None`（保守：无法判定等效）。
/// 空集合同样返回 `None`——**空服务器集永不等效**（指纹规则之一）。
#[must_use]
pub(crate) fn canonical_server_set(servers: &[String]) -> Option<HashSet<String>> {
    if servers.is_empty() {
        return None;
    }
    let mut set = HashSet::with_capacity(servers.len());
    for server in servers {
        let parsed: IpAddr = server.trim().parse().ok()?;
        set.insert(parsed.to_string());
    }
    Some(set)
}

/// 单个命名空间形式的外来覆盖判定（三态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForeignFormVerdict {
    /// 两表都无覆盖——EXV 正常添加自有规则（原路径）。
    Absent,
    /// 外来规则等效提供（服务器集合序无关相等 + DNSSEC/IPsec/DirectAccess 全关）
    /// ——跳过自有添加、标记 external-provided、teardown 不动。
    Equivalent,
    /// 有覆盖但不等效（含自有孤儿前缀覆盖、服务器不一致、任一附加设置启用）
    /// ——原冲突失败路径，语义不变。
    Conflict,
}

/// 判定单个命名空间形式的外来覆盖（纯函数；输入须为两表完整投影）。
///
/// 指纹规则（与任务书硬约束逐条对应）：
/// - 命名空间按带点/不带点精确形式匹配（大小写不敏感）；
/// - 服务器集合序无关相等（经 [`canonical_server_set`] 规范化）；
/// - DNSSEC/IPsec/DirectAccess 任一启用即不等效（有效策略表无 IPsec 投影属性，
///   按可观测标志判定并如实注释——不可观测项不虚构）；
/// - 空服务器集（或含不可解析项）永不等效；
/// - **自有前缀（`EXV-NRPT:`）覆盖恒为 Conflict**：自家孤儿归认领清理，不是
///   「外来等效」——即使内容一致也先走认领删除再由 worker 重加（生命周期清晰）。
#[must_use]
pub(crate) fn foreign_form_verdict(
    form: &str,
    desired_servers: &[String],
    local_rules: &[PsNrptRule],
    effective: &[PsNrptPolicyEntry],
) -> ForeignFormVerdict {
    let desired = canonical_server_set(desired_servers);
    let mut any_coverage = false;
    // 本地规则表：own 前缀覆盖 → Conflict；外来覆盖按指纹判定。
    for rule in local_rules {
        let covers = rule
            .namespaces
            .iter()
            .any(|namespace| namespace.trim().eq_ignore_ascii_case(form));
        if !covers {
            continue;
        }
        if rule.comment.starts_with(NRPT_OWNER_PREFIX) {
            return ForeignFormVerdict::Conflict;
        }
        any_coverage = true;
        if let Some(desired) = &desired
            && canonical_server_set(&rule.servers).as_ref() == Some(desired)
            && !rule.dnssec
            && !rule.ipsec
            && !rule.direct_access
        {
            return ForeignFormVerdict::Equivalent;
        }
    }
    // 有效策略表：仅当本地表无等效时补充判定（本地等效已返回；本地非等效覆盖
    // 继续）。有效表条目无 owner 信息——EXV 自有规则必持久化在本地表，因此
    // 「本地表无该形式 + 有效表有」即为外来（含仅有效表形态，2026-10-08 事故）。
    for entry in effective {
        let covers = entry
            .namespaces
            .iter()
            .any(|namespace| namespace.trim().eq_ignore_ascii_case(form));
        if !covers {
            continue;
        }
        any_coverage = true;
        if let Some(desired) = &desired
            && canonical_server_set(&entry.servers).as_ref() == Some(desired)
            && !entry.dnssec
            && !entry.direct_access
        {
            return ForeignFormVerdict::Equivalent;
        }
    }
    if any_coverage {
        ForeignFormVerdict::Conflict
    } else {
        ForeignFormVerdict::Absent
    }
}

/// T3 装配前的等效接纳决策（对全部校园形式逐个判定）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NrptEquivalenceDecision {
    /// 等效接纳的形式清单（传给 worker 的 `accept_forms`；teardown 不动它们）。
    pub(crate) accept_forms: Vec<String>,
    /// 不等效冲突形式（维持原冲突失败路径——仅诊断用）。
    pub(crate) conflict_forms: Vec<String>,
    /// 两表皆无覆盖的形式（worker 正常添加）。
    pub(crate) absent_forms: Vec<String>,
    /// 等效接纳指纹摘要（SHA-256 取前 16 字节的 hex；无接纳时为空串）。
    pub(crate) digest_hex: String,
}

impl NrptEquivalenceDecision {
    /// 全部校园形式都被等效接纳（worker 完全无需启动——防死锁约束：等效接纳
    /// 路径不启动 NRPT worker，杜绝新的「等待 worker/PS 进程退出」依赖）。
    #[must_use]
    pub(crate) fn accepts_all(&self, campus_domains: &[String]) -> bool {
        let forms = expand_campus_forms(campus_domains);
        !forms.is_empty()
            && forms
                .iter()
                .all(|form| self.accept_forms.iter().any(|accept| accept == form))
    }
}

/// 对全部校园形式做等效判定（T3 装配的权威判定；会话下发的
/// `campus_dns_servers` 在此才可知——连接门/善后路径不可用时走 classify 的
/// 常量启发式，本函数才是权威）。
#[must_use]
pub(crate) fn nrpt_equivalence_decision(
    campus_domains: &[String],
    desired_servers: &[String],
    local_rules: &[PsNrptRule],
    effective: &[PsNrptPolicyEntry],
) -> NrptEquivalenceDecision {
    let mut decision = NrptEquivalenceDecision::default();
    for form in expand_campus_forms(campus_domains) {
        match foreign_form_verdict(&form, desired_servers, local_rules, effective) {
            ForeignFormVerdict::Equivalent => decision.accept_forms.push(form),
            ForeignFormVerdict::Conflict => decision.conflict_forms.push(form),
            ForeignFormVerdict::Absent => decision.absent_forms.push(form),
        }
    }
    if !decision.accept_forms.is_empty() {
        decision.digest_hex = nrpt_acceptance_digest(&decision.accept_forms, desired_servers);
    }
    decision
}

/// 等效接纳指纹摘要：SHA-256(join("\u{1f}", [sorted(lower(forms)),
/// "servers=" + join(",", sorted(canonical(servers)))])) 前 16 字节的 hex。
///
/// 与 2026-10-08 计划 §5.3 的授权指纹同构（该指纹含 store/形状位，属阶段 2；
/// 阶段 1 等效接纳的形状恒为「全关」，此处摘要即形式+服务器集合）。用途：
/// `nrpt.equivalent_accepted` 打点与 runbook 对线；非安全哈希。
#[must_use]
pub(crate) fn nrpt_acceptance_digest(forms: &[String], servers: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut form_parts: Vec<String> = forms
        .iter()
        .map(|form| form.trim().to_ascii_lowercase())
        .collect();
    form_parts.sort();
    form_parts.dedup();
    let mut server_parts: Vec<String> = servers
        .iter()
        .map(|server| {
            server
                .trim()
                .parse::<IpAddr>()
                .map(|parsed| parsed.to_string())
                .unwrap_or_else(|_| server.trim().to_ascii_lowercase())
        })
        .collect();
    server_parts.sort();
    server_parts.dedup();
    let mut material = form_parts;
    material.push(format!("servers={}", server_parts.join(",")));
    let mut hasher = Sha256::new();
    hasher.update(material.join("\u{1f}").as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 接管目标的存储分支初判（D8 阶段 2；注册表分支为阶段 0 落实测布局后的扩展点，
/// 此处不设变体——启用时随实测布局新增）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NrptTakeoverStore {
    /// 本地规则表命中（句柄 = 规则 Name，经 `nrpt_delete` op 外科手术式按名删除）。
    RuleTable,
    /// 仅有效策略表覆盖（本地表无该形式的外来覆盖；无公开寻址手段 → 经
    /// `restart_dnscache` op 重启 Dnscache 清空内存态，护栏见 `nrpt_takeover.rs`）。
    EffectiveOnly,
}

impl NrptTakeoverStore {
    /// journal/日志用的稳定标签。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RuleTable => "rule_table",
            Self::EffectiveOnly => "effective_only",
        }
    }
}

/// 单个命名空间形式的静默精准接管目标（判定纯函数产物；执行归 `nrpt_takeover.rs`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NrptTakeoverTarget {
    /// 命名空间形式（带点/不带点之一，来自 [`expand_campus_forms`]）。
    pub(crate) form: String,
    /// 存储初判。
    pub(crate) store: NrptTakeoverStore,
    /// 本地表命中时的外来规则投影（`store == RuleTable` 时 `Some`）。
    pub(crate) rule: Option<PsNrptRule>,
    /// 有效表命中时的条目投影（`store == EffectiveOnly` 时 `Some`）。
    pub(crate) entry: Option<PsNrptPolicyEntry>,
}

/// 静默精准接管目标判定（D8 阶段 2；2026-10-09 用户裁定「fail-closed 改静默覆盖，
/// 程序侧保证精准覆盖 + 最小必要覆盖」）。
///
/// 对每个校园形式做 [`foreign_form_verdict`] 同源判定，只解析 **Conflict** 形式的
/// 覆盖来源。最小必要的判定结构保证（全部可直接单测钉住）：
/// - **等效不覆盖**（裁定推论 a）：Equivalent 形式不进目标（走 external-provided
///   接纳）；因此「会话服务器等效 + 常量不等效」的规则在 T3（会话权威）判定下
///   目标必空——这是最小必要的回归钉。
/// - **清单外绝不碰**（裁定推论 b）：仅 [`expand_campus_forms`]（带点+不带点）
///   之内的形式参与判定，其他命名空间不进入任何分支。
/// - **自有前缀排除**（复审遗漏 1 必改）：`EXV-NRPT:` 前缀覆盖的形式整体跳过
///   ——自有孤儿归认领清理域，绝不能被 journal 记为 `nrpt_foreign_taken_over`。
///   若该形式同时存在外来覆盖，认领清理删除自有规则后由 worker conflict 分支
///   重判接管（同一连接内共享「单指纹至多覆盖一次」硬闸）。
/// - **本地表优先**：外来规则命中本地表 → `RuleTable`（按名删除的最小动作）；
///   本地表无外来覆盖而有效表有 → `EffectiveOnly`（Dnscache 重启分支）。
///
/// 输入须为两表完整投影（与 [`nrpt_equivalence_decision`] 同数据源）。
#[must_use]
pub(crate) fn takeover_targets(
    campus_domains: &[String],
    session_servers: &[String],
    local_rules: &[PsNrptRule],
    effective: &[PsNrptPolicyEntry],
) -> Vec<NrptTakeoverTarget> {
    let covers_form = |namespaces: &[String], form: &str| {
        namespaces
            .iter()
            .any(|namespace| namespace.trim().eq_ignore_ascii_case(form))
    };
    let mut targets = Vec::new();
    for form in expand_campus_forms(campus_domains) {
        if foreign_form_verdict(&form, session_servers, local_rules, effective)
            != ForeignFormVerdict::Conflict
        {
            // Absent=正常添加；Equivalent=等效接纳（等效不覆盖）。
            continue;
        }
        // 自有前缀覆盖 → 认领清理域，绝不接管（复审遗漏 1）。
        if local_rules.iter().any(|rule| {
            rule.comment.starts_with(NRPT_OWNER_PREFIX) && covers_form(&rule.namespaces, &form)
        }) {
            continue;
        }
        // 解析外来覆盖来源：本地表外来规则（可按名删除）优先；否则仅有效表覆盖。
        if let Some(rule) = local_rules.iter().find(|rule| {
            !rule.comment.starts_with(NRPT_OWNER_PREFIX) && covers_form(&rule.namespaces, &form)
        }) {
            targets.push(NrptTakeoverTarget {
                form: form.clone(),
                store: NrptTakeoverStore::RuleTable,
                rule: Some(rule.clone()),
                entry: None,
            });
        } else if let Some(entry) = effective
            .iter()
            .find(|entry| covers_form(&entry.namespaces, &form))
        {
            targets.push(NrptTakeoverTarget {
                form: form.clone(),
                store: NrptTakeoverStore::EffectiveOnly,
                rule: None,
                entry: Some(entry.clone()),
            });
        }
        // 两表皆无外来覆盖而 verdict=Conflict 不可达（Conflict 必有覆盖来源；
        // 纯自有前缀覆盖已在上方 continue）。
    }
    targets
}

/// NRPT 域分类结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NrptClassified {
    /// 自有孤儿（注释带 `EXV-NRPT:` 前缀）——清理对象。
    ///
    /// 孤儿语义：预检的前提是**无存活 engine 会话**，因此无论 GUID 是否匹配
    /// 上一会话记录，带前缀的规则都是 EXV 残留（匹配=最近一次会话；不匹配=更早
    /// 会话/其他批次），一律进入清理集。
    pub(crate) own: Vec<PsNrptRule>,
    /// 外来冲突（无自有前缀且命中校园域）——仅上报，绝不删除
    /// （他牌 VPN 的规则未授权不可动；补救三选归 UI，计划 §三）。
    pub(crate) foreign_conflict: Vec<PsNrptRule>,
    /// 有效策略表上的非等效外来覆盖（含仅有效表形态——本地表为空而有效表有，
    /// 2026-10-08 事故即此）。仅上报；渲染沿用 `nrpt_foreign_conflict` 稳定 kind
    /// （同一阻断语义：有外来规则挡住了校园 DNS 装配）。
    pub(crate) foreign_effective: Vec<PsNrptPolicyEntry>,
    /// 等效外来提供的形式（external-provided 候选）——ReportOnly，不阻断、
    /// 不清理；T3 装配以会话服务器做权威重判（连接门此处仅按传入服务器集合作
    /// 启发式判定）。
    pub(crate) external_provided: Vec<String>,
}

/// 分类 NRPT 规则（本地表 + 有效策略表）：
/// - 自有前缀 → `own`；
/// - 命中校园域（**带点+不带点两种形式，大小写不敏感**——2026-10-08 计划缺口
///   修复：此前仅按配置原文匹配，带点形式的外来规则完全不可见）：
///   - 与 `campus_servers` 完全等效（指纹规则同 [`foreign_form_verdict`]）→
///     `external_provided`（ReportOnly，不阻断——T3 重判）；
///   - 其余 → `foreign_conflict` / `foreign_effective`；
/// - 其余（外来但不涉校园域）不是预检发现。
///
/// `campus_servers` 是等效判定的服务器集合：连接门/善后路径传已知校园解析器
/// 常量（启发式——会话服务器在 T3 才可知，误判方向安全：门放行的等效形式若
/// 与会话服务器不等效，T3 权威判定会走原冲突失败路径）；T3 认领清理传会话
/// 实际下发的服务器。
///
/// hint 的 `nrpt_owner_guids` 刻意不参与判定（孤儿语义，见 [`NrptClassified::own`]）。
#[must_use]
pub(crate) fn classify_nrpt(
    rules: &[PsNrptRule],
    effective: &[PsNrptPolicyEntry],
    campus_domains: &[String],
    campus_servers: &[String],
) -> NrptClassified {
    let forms = expand_campus_forms(campus_domains);
    let mut classified = NrptClassified::default();
    let note_equivalent = |form: &str, classified: &mut NrptClassified| {
        if !classified.external_provided.contains(&form.to_owned()) {
            classified.external_provided.push(form.to_owned());
        }
    };
    // 本地规则表：一条规则可覆盖多个校园形式，逐形式判定——任一覆盖形式非
    // 等效（含被自有孤儿/他规则挡住）即整条进冲突集（保守：冲突规则即使部分
    // 形式等效也不据此放行）。
    for rule in rules {
        if rule.comment.starts_with(NRPT_OWNER_PREFIX) {
            classified.own.push(rule.clone());
            continue;
        }
        let mut equivalent_forms = Vec::new();
        let mut conflicted = false;
        let mut covers_any = false;
        for form in &forms {
            let covers = rule
                .namespaces
                .iter()
                .any(|namespace| namespace.trim().eq_ignore_ascii_case(form));
            if covers {
                covers_any = true;
                match foreign_form_verdict(form, campus_servers, rules, effective) {
                    ForeignFormVerdict::Equivalent => equivalent_forms.push(form.clone()),
                    ForeignFormVerdict::Conflict | ForeignFormVerdict::Absent => {
                        conflicted = true;
                    }
                }
            }
        }
        if covers_any && (conflicted || equivalent_forms.is_empty()) {
            classified.foreign_conflict.push(rule.clone());
        } else {
            for form in equivalent_forms {
                note_equivalent(&form, &mut classified);
            }
        }
    }
    // 有效策略表：同规则逐形式判定；非等效覆盖进 foreign_effective。
    for entry in effective {
        let mut equivalent_forms = Vec::new();
        let mut conflicted = false;
        let mut covers_any = false;
        for form in &forms {
            let covers = entry
                .namespaces
                .iter()
                .any(|namespace| namespace.trim().eq_ignore_ascii_case(form));
            if covers {
                covers_any = true;
                match foreign_form_verdict(form, campus_servers, rules, effective) {
                    ForeignFormVerdict::Equivalent => equivalent_forms.push(form.clone()),
                    ForeignFormVerdict::Conflict | ForeignFormVerdict::Absent => {
                        conflicted = true;
                    }
                }
            }
        }
        if covers_any && (conflicted || equivalent_forms.is_empty()) {
            classified.foreign_effective.push(entry.clone());
        } else {
            for form in equivalent_forms {
                note_equivalent(&form, &mut classified);
            }
        }
    }
    classified
}

/// 认领清理视角的外来在场报告（缺口 G 修复：`foreign_present` 此前只读本地表，
/// 仅有效表存在的外来规则永远上报为空——事故日志已证实失真）。
///
/// 逐校园形式报告外来覆盖来源标识：`{form}@local:{规则名}`（本地表外来）、
/// `{form}@effective`（仅有效表）、`{form}@equivalent`（等效提供）。自有前缀
/// 规则不在报告内（它们是认领删除对象，已在 `removed` 记账）。
#[must_use]
pub(crate) fn foreign_presence_report(
    campus_domains: &[String],
    campus_servers: &[String],
    local_rules: &[PsNrptRule],
    effective: &[PsNrptPolicyEntry],
) -> Vec<String> {
    let mut report = Vec::new();
    for form in expand_campus_forms(campus_domains) {
        for rule in local_rules {
            if rule.comment.starts_with(NRPT_OWNER_PREFIX) {
                continue;
            }
            let covers = rule
                .namespaces
                .iter()
                .any(|namespace| namespace.trim().eq_ignore_ascii_case(&form));
            if covers {
                report.push(format!("{form}@local:{}", rule.name));
            }
        }
        let effective_covers = effective.iter().any(|entry| {
            entry
                .namespaces
                .iter()
                .any(|namespace| namespace.trim().eq_ignore_ascii_case(&form))
        });
        if effective_covers {
            report.push(format!("{form}@effective"));
        }
        if foreign_form_verdict(&form, campus_servers, local_rules, effective)
            == ForeignFormVerdict::Equivalent
        {
            report.push(format!("{form}@equivalent"));
        }
    }
    report
}

/// NRPT 域发现渲染（`own` → `Cleanup`；`foreign_conflict`/`foreign_effective` →
/// `ReportOnly`；`external_provided` → `ReportOnly`）。
///
/// detail 自 D8 阶段 2 起携带 servers/store（2026-10-09 事故观测缺口 §1.3 的修复
/// 材料：连接门预检结构化日志据此落「发现构成」——此前 blocked 事件只有
/// `Nrpt×4` 计数，事故构成无法事后复原）。
pub(crate) fn nrpt_findings(classified: &NrptClassified) -> Vec<Finding> {
    let mut findings: Vec<Finding> = classified
        .own
        .iter()
        .map(|rule| Finding {
            kind: "nrpt_own_orphan",
            detail: format!(
                "NRPT 规则 {name}（namespace: {namespaces}，servers: {servers}，store: rule_table）owner={comment}",
                name = rule.name,
                namespaces = rule.namespaces.join(","),
                servers = rule.servers.join(","),
                comment = rule.comment
            ),
            policy: FindingPolicy::Cleanup,
        })
        .collect();
    findings.extend(classified.foreign_conflict.iter().map(|rule| Finding {
        kind: "nrpt_foreign_conflict",
        detail: format!(
            "外来 NRPT 规则 {name}（namespace: {namespaces}，servers: {servers}，store: rule_table）覆盖校园域，未改动（连接门放行，T3 将以会话服务器权威判定）",
            name = rule.name,
            namespaces = rule.namespaces.join(","),
            servers = rule.servers.join(",")
        ),
        policy: FindingPolicy::ReportOnly,
    }));
    findings.extend(classified.foreign_effective.iter().map(|entry| Finding {
        // 沿用既有稳定 kind：语义与本地表外来一致（消费方按
        // reason id 路由），detail 说明数据源为有效策略表。
        detail: format!(
            "有效策略表存在外来 NRPT 覆盖（namespace: {namespaces}，servers: {servers}，store: effective_only，本地规则表无对应规则），未改动（连接门放行，T3 将以会话服务器权威判定）",
            namespaces = entry.namespaces.join(","),
            servers = entry.servers.join(",")
        ),
        kind: "nrpt_foreign_conflict",
        policy: FindingPolicy::ReportOnly,
    }));
    findings.extend(classified.external_provided.iter().map(|form| Finding {
        kind: "nrpt_external_provided",
        detail: format!(
            "外来 NRPT 规则等效提供校园域形式 {form}（external-provided）；本次连接将跳过自有添加、不阻断、不删除"
        ),
        policy: FindingPolicy::ReportOnly,
    }));
    findings
}

// ---------------------------------------------------------------------------
// 网卡域
// ---------------------------------------------------------------------------

/// 网卡域发现：预检运行时（前提：无存活会话）仍存在的自有网卡。
///
/// 命中条件：FriendlyName 等于 engine 的 `ExvEngine` 常量名，或列在 hint 的
/// `adapter_names`（含遗留 `本地连接` 形态——**仅当** hint 记录了它，避免误伤
/// 用户同名物理网卡）。处置为 Manual：wintun 0.14.1 只有创建者 close 才移除
/// adapter（`wintun_adapter.rs` 冻结事实），预检进程不是创建者，无安全删除路径。
#[must_use]
pub(crate) fn classify_adapter_orphans(
    records: &[AdapterRecord],
    hint_adapter_names: &[String],
) -> Vec<AdapterRecord> {
    records
        .iter()
        .filter(|record| {
            let name = record.name.as_str();
            name.eq_ignore_ascii_case(crate::service::ENGINE_ADAPTER_NAME)
                || hint_adapter_names
                    .iter()
                    .any(|hint| name.eq_ignore_ascii_case(hint))
        })
        .cloned()
        .collect()
}

/// 网卡域分类结果（W1 四象限判定后的完整域数据）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AdapterDomainData {
    /// 命中自有名（ExvEngine / hint 记录名）的系统网卡记录（清理记账用）。
    pub(crate) matched: Vec<AdapterRecord>,
    /// 按归属事实渲染后的发现（A 类持有复用=非发现；B/C=ReportOnly；D=Manual）。
    pub(crate) findings: Vec<Finding>,
}

/// 单条命中记录的处置判定（W1 四象限的 A/C/D 三类；B 由域级合成）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdapterDisposition {
    /// A：本引擎 NicOwner 持有该网卡——跨连接持久设计内状态，非发现，连接复用。
    OwnedReuse,
    /// C：创建者不在的真孤儿——不阻断；装配阶段领用/重建（W3 实证前保持只报）。
    OrphanUnowned,
    /// D：自有名被非 EXV/wintun 网卡占用——真需人工处置，阻断。
    NameConflict,
}

/// EXV/wintun 网卡描述特征（自有及遗留形态）；其余描述视为外来占用（D 类）。
fn looks_like_exv_adapter(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    lower.contains("exv vpn tunnel") || lower.contains("wintun")
}

/// 按归属事实判定单条命中记录的处置（W1）。
#[must_use]
pub(crate) fn adapter_disposition(
    record: &AdapterRecord,
    held_adapter_name: Option<&str>,
) -> AdapterDisposition {
    if held_adapter_name.is_some_and(|held| held.eq_ignore_ascii_case(&record.name)) {
        AdapterDisposition::OwnedReuse
    } else if looks_like_exv_adapter(&record.description) {
        AdapterDisposition::OrphanUnowned
    } else {
        AdapterDisposition::NameConflict
    }
}

/// 网卡域完整分类（W1）：过滤自有名命中 → 按归属逐条定性 → 合成 B 类发现。
///
/// - A（本进程 NicOwner 持有且系统可枚举）：**非发现**——持久网卡设计内状态，
///   预检不得据此拒绝连接（用户底线：不能在实际可连接时拒绝连接）。
/// - B（本进程持有句柄但系统枚举不到该网卡）：ReportOnly——装配阶段由
///   `NicOwner::ensure` 重建，不阻断。
/// - C（未持有 + 描述为 EXV/wintun 形态的真孤儿）：ReportOnly——不阻断；
///   装配阶段领用/重建（W3 实证后扩展为主动处置），文案保留"重启服务可清"。
/// - D（未持有 + 描述非 EXV/wintun：自有名被外来网卡占用）：Manual——唯一
///   保留人工处置的网卡情形，阻断且文案准确。
#[must_use]
pub(crate) fn adapter_domain_data(
    records: &[AdapterRecord],
    hint_adapter_names: &[String],
    held_adapter_name: Option<&str>,
) -> AdapterDomainData {
    let matched = classify_adapter_orphans(records, hint_adapter_names);
    let mut findings = Vec::new();
    for record in &matched {
        match adapter_disposition(record, held_adapter_name) {
            AdapterDisposition::OwnedReuse => {}
            AdapterDisposition::OrphanUnowned => findings.push(Finding {
                kind: "adapter_orphan_unowned",
                detail: format!(
                    "网卡 {name}（ifindex={index}，{desc}）为遗留 EXV 网卡（创建者进程不在）；本次连接不据此阻断，装配阶段将领用或重建；如持续残留可重启 EXV 服务",
                    name = record.name,
                    index = record.if_index,
                    desc = record.description
                ),
                policy: FindingPolicy::ReportOnly,
            }),
            AdapterDisposition::NameConflict => findings.push(Finding {
                kind: "adapter_name_conflict",
                detail: format!(
                    "网卡名 {name}（ifindex={index}，{desc}）被非 EXV 网卡占用，需人工处置",
                    name = record.name,
                    index = record.if_index,
                    desc = record.description
                ),
                policy: FindingPolicy::Manual,
            }),
        }
    }
    if let Some(held) = held_adapter_name
        && !records
            .iter()
            .any(|record| record.name.eq_ignore_ascii_case(held))
    {
        findings.push(Finding {
            kind: "adapter_owned_missing",
            detail: format!("本引擎持有网卡 {held} 句柄但系统未枚举到该网卡；装配阶段将重建"),
            policy: FindingPolicy::ReportOnly,
        });
    }
    AdapterDomainData { matched, findings }
}

// ---------------------------------------------------------------------------
// 路由域
// ---------------------------------------------------------------------------

/// 路由域分类结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoutesClassified {
    /// 死接口上的 `NetMgmt` 路由（engine/手工 API 安装形态的残留）——清理对象。
    pub(crate) dangling: Vec<PsRouteRow>,
    /// 死接口上的非 `NetMgmt` 路由——仅上报（非自有形态，保守不动）。
    pub(crate) dangling_foreign: Vec<PsRouteRow>,
    /// 活接口上的 bypass 形状（`/32` + metric 1 + `NetMgmt` + 有下一跳）——仅上报
    /// （hint 不含网关 IP，无法断言归属；可能是上一会话的 bypass，也可能是他牌）。
    pub(crate) bypass_shape: Vec<PsRouteRow>,
}

/// 分类路由：死接口（ifindex 不在存活集且非 loopback）为 dangling；活接口上
/// 的 bypass 形状仅上报。目的地址不可解析为 IPv4 的行不参与任何判定。
#[must_use]
pub(crate) fn classify_routes(
    rows: &[PsRouteRow],
    live_ifindexes: &HashSet<u32>,
) -> RoutesClassified {
    let mut classified = RoutesClassified::default();
    for row in rows {
        if row.dest.parse::<Ipv4Addr>().is_err() {
            continue;
        }
        let alive = row.ifindex == LOOPBACK_IFINDEX || live_ifindexes.contains(&row.ifindex);
        let netmgmt = row.protocol.eq_ignore_ascii_case("NetMgmt");
        if !alive {
            if netmgmt {
                classified.dangling.push(row.clone());
            } else {
                classified.dangling_foreign.push(row.clone());
            }
        } else if netmgmt
            && row.prefix == BYPASS_ROUTE_PREFIX
            && row.metric == BYPASS_ROUTE_METRIC
            && !row.next_hop.is_empty()
            && row.next_hop != "0.0.0.0"
        {
            classified.bypass_shape.push(row.clone());
        }
    }
    classified
}

/// 路由域发现渲染。
pub(crate) fn route_findings(classified: &RoutesClassified) -> Vec<Finding> {
    let mut findings: Vec<Finding> = classified
        .dangling
        .iter()
        .map(|row| Finding {
            kind: "route_dangling",
            detail: format!(
                "路由 {dest}/{prefix} 指向不存在的接口 ifindex={index}（{proto}），删除",
                dest = row.dest,
                prefix = row.prefix,
                index = row.ifindex,
                proto = row.protocol
            ),
            policy: FindingPolicy::Cleanup,
        })
        .collect();
    findings.extend(classified.dangling_foreign.iter().map(|row| Finding {
        kind: "route_dangling_foreign",
        detail: format!(
            "路由 {dest}/{prefix} 指向不存在的接口 ifindex={index}（{proto}），非自有形态仅上报",
            dest = row.dest,
            prefix = row.prefix,
            index = row.ifindex,
            proto = row.protocol
        ),
        policy: FindingPolicy::ReportOnly,
    }));
    findings.extend(classified.bypass_shape.iter().map(|row| Finding {
        kind: "route_bypass_shape",
        detail: format!(
            "路由 {dest}/{prefix} metric={metric} next-hop={hop} 形似网关 bypass，仅上报",
            dest = row.dest,
            prefix = row.prefix,
            metric = row.metric,
            hop = row.next_hop
        ),
        policy: FindingPolicy::ReportOnly,
    }));
    findings
}

// ---------------------------------------------------------------------------
// 接口 DNS 域
// ---------------------------------------------------------------------------

/// 接口 DNS 域发现：DNS 服务器集合恰为已知校园解析器集合的接口。
///
/// 清理策略 D-pending：本轮仅上报。
#[must_use]
pub(crate) fn classify_iface_dns(ifaces: &[PsDnsIface]) -> Vec<PsDnsIface> {
    let campus: HashSet<&str> = CAMPUS_DNS_SERVERS.iter().copied().collect();
    ifaces
        .iter()
        .filter(|iface| {
            let servers: HashSet<&str> = iface.servers.iter().map(String::as_str).collect();
            servers == campus
        })
        .cloned()
        .collect()
}

/// 接口 DNS 域发现渲染（全部 `ReportOnly`）。
pub(crate) fn iface_dns_findings(hits: &[PsDnsIface]) -> Vec<Finding> {
    hits.iter()
        .map(|iface| Finding {
            kind: "iface_dns_campus",
            detail: format!(
                "接口 {alias}（ifindex={index}）DNS 为校园解析器集合，本轮仅上报",
                alias = iface.alias,
                index = iface.ifindex
            ),
            policy: FindingPolicy::ReportOnly,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 系统代理域
// ---------------------------------------------------------------------------

/// 系统代理域分类结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyClassified {
    /// 指向 hint 端口的回环端点（自有残留）——清理对象（禁用 `ProxyEnable`）。
    pub(crate) ours: Vec<ProxyEndpoint>,
    /// 指向其它回环端点的端点（他牌本机代理）——仅上报。
    pub(crate) foreign_loopback: Vec<ProxyEndpoint>,
}

/// 分类系统代理快照：只看**已启用**代理里的回环端点；非回环端点（用户自己的
/// 局域网代理等）不是预检发现。端点解析复用 `system_proxy::snapshot_from_raw`
/// 的规范化结果（host 已去 IPv6 括号、socks5/4 已归一）。
#[must_use]
pub(crate) fn classify_proxy(snapshot: &SystemProxySnapshot, our_ports: &[u16]) -> ProxyClassified {
    let mut ours = Vec::new();
    let mut foreign_loopback = Vec::new();
    for endpoint in &snapshot.endpoints {
        if !LOOPBACK_HOSTS.contains(&endpoint.host.as_str()) {
            continue;
        }
        if our_ports.contains(&endpoint.port) {
            ours.push(endpoint.clone());
        } else {
            foreign_loopback.push(endpoint.clone());
        }
    }
    ProxyClassified {
        ours,
        foreign_loopback,
    }
}

/// 系统代理域发现渲染。
pub(crate) fn proxy_findings(
    classified: &ProxyClassified,
    proxy_server_text: Option<&str>,
) -> Vec<Finding> {
    let server_text = proxy_server_text.unwrap_or("");
    let mut findings: Vec<Finding> = classified
        .ours
        .iter()
        .map(|endpoint| Finding {
            kind: "proxy_loopback_ours",
            detail: format!(
                "系统代理指向自有本地端口 {host}:{port}（ProxyServer={text:?}），禁用",
                host = endpoint.host,
                port = endpoint.port,
                text = server_text
            ),
            policy: FindingPolicy::Cleanup,
        })
        .collect();
    findings.extend(classified.foreign_loopback.iter().map(|endpoint| Finding {
        kind: "proxy_loopback_foreign",
        detail: format!(
            "系统代理指向外来回环端口 {host}:{port}（ProxyServer={text:?}），未改动",
            host = endpoint.host,
            port = endpoint.port,
            text = server_text
        ),
        policy: FindingPolicy::ReportOnly,
    }));
    findings
}

// ---------------------------------------------------------------------------
// 单元测试（纯函数注入；不触碰真实系统）。
// ---------------------------------------------------------------------------
