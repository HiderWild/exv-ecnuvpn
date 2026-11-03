//! 预检域分类纯函数：把各域探测到的原始行映射为「发现 + 处置策略」。
//!
//! 全部为无副作用纯函数（注入单测）；探测（读）在 [`super`] 的域探针里，
//! 清理（写）在清理执行器里——**本模块只做判定，绝不触碰系统**。
//!
//! 处置策略（计划 §三 / 2026-09-27 任务书）：
//! - **`Cleanup`**：自有残留，本轮预检删除（NRPT 自有前缀规则 / `NetMgmt` 死
//!   接口路由 / 指向自有本地端口的系统代理）。
//! - **`ReportOnly`**：只上报不动手（外来 NRPT 冲突、活接口 bypass 形状路由、
//!   非自有回环代理、接口 DNS=校园解析器、journal 遗留）。
//! - **`Manual`**：发现归自有但现无安全删除路径（孤儿 Wintun 网卡——wintun 0.14.1
//!   只有创建者 close 才移除 adapter，预检进程不是创建者）。

use std::collections::HashSet;
use std::net::Ipv4Addr;

use exv_vpn_win32_resource::proxy_tun::AdapterRecord;
use exv_vpn_win32_resource::routes::{BYPASS_ROUTE_METRIC, BYPASS_ROUTE_PREFIX};
use exv_vpn_win32_resource::system_proxy::{ProxyEndpoint, SystemProxySnapshot};

use super::probe_child::{PsDnsIface, PsNrptRule, PsRouteRow};
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

/// NRPT 域分类结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NrptClassified {
    /// 自有孤儿（注释带 `EXV-NRPT:` 前缀）——清理对象。
    ///
    /// 孤儿语义：预检的前提是**无存活 engine 会话**，因此无论 GUID 是否匹配
    /// 上一会话记录，带前缀的规则都是 EXV 残留（匹配=最近一次会话；不匹配=更早
    /// 会话/其他批次），一律进入清理集。
    pub(crate) own: Vec<PsNrptRule>,
    /// 外来冲突（无自有前缀且命名空间命中校园域）——仅上报，绝不删除
    /// （他牌 VPN 的规则不可动；补救三选归 UI，计划 §三）。
    pub(crate) foreign_conflict: Vec<PsNrptRule>,
}

/// 分类 NRPT 规则：自有前缀 → `own`；无前缀且命中校园命名空间（大小写不
/// 敏感）→ `foreign_conflict`；其余（外来但不涉校园域）不是预检发现。
///
/// hint 的 `nrpt_owner_guids` 刻意不参与判定（孤儿语义，见 [`NrptClassified::own`]）。
#[must_use]
pub(crate) fn classify_nrpt(rules: &[PsNrptRule], campus_namespaces: &[String]) -> NrptClassified {
    let mut own = Vec::new();
    let mut foreign_conflict = Vec::new();
    for rule in rules {
        if rule.comment.starts_with(NRPT_OWNER_PREFIX) {
            own.push(rule.clone());
        } else if rule.namespaces.iter().any(|namespace| {
            campus_namespaces
                .iter()
                .any(|campus| namespace.eq_ignore_ascii_case(campus))
        }) {
            foreign_conflict.push(rule.clone());
        }
    }
    NrptClassified {
        own,
        foreign_conflict,
    }
}

/// NRPT 域发现渲染（`own` → `Cleanup`；`foreign_conflict` → `ReportOnly`）。
pub(crate) fn nrpt_findings(classified: &NrptClassified) -> Vec<Finding> {
    let mut findings: Vec<Finding> = classified
        .own
        .iter()
        .map(|rule| Finding {
            kind: "nrpt_own_orphan",
            detail: format!(
                "NRPT 规则 {name}（namespace: {namespaces}）owner={comment}",
                name = rule.name,
                namespaces = rule.namespaces.join(","),
                comment = rule.comment
            ),
            policy: FindingPolicy::Cleanup,
        })
        .collect();
    findings.extend(classified.foreign_conflict.iter().map(|rule| Finding {
        kind: "nrpt_foreign_conflict",
        detail: format!(
            "外来 NRPT 规则 {name}（namespace: {namespaces}）覆盖校园域，未改动",
            name = rule.name,
            namespaces = rule.namespaces.join(",")
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

/// 网卡域发现渲染（全部 Manual）。
pub(crate) fn adapter_findings(orphans: &[AdapterRecord]) -> Vec<Finding> {
    orphans
        .iter()
        .map(|record| Finding {
            kind: "adapter_orphan",
            detail: format!(
                "网卡 {name}（ifindex={index}，{desc}）为 EXV 残留，需人工删除",
                name = record.name,
                index = record.if_index,
                desc = record.description
            ),
            policy: FindingPolicy::Manual,
        })
        .collect()
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
pub(crate) fn classify_proxy(
    snapshot: &SystemProxySnapshot,
    our_ports: &[u16],
) -> ProxyClassified {
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
