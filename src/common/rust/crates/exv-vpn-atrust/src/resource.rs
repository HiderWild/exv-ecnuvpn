//! 资源目录：把服务端 `clientResource` JSON 解析成**保留约束**的 [`ResourceCatalog`]。
//!
//! # 输入形态（对照上游 `client/atrust/parse.go@8de443f`）
//!
//! `Data.AppList.Data.AppInfo[].Apps[]` 每项含 `ID`/`NodeGroupID`/`AccessModel`/
//! `EnableTCPPrefL3`/`AddrPretend`/`AddressList[]`；地址项含 `Protocol`（`tcp`/`udp`/`all`）、
//! `Port`（单值或 `min-max`）、`Host`（IP / CIDR / `ipMin-ipMax` / 域名，域名可带 `*.` 前缀）、
//! `IP[]`（域名型条目附带的解析地址）。节点组来自
//! `Data.AppList.Data.Config.NodeGroupConf`，DNS 选项来自 `Data.SDPPolicy.Data.ClientOption`。
//!
//! # 本模块的取舍
//!
//! - **保留约束**：目录保留协议、端口区间、App/资源 ID、节点组、地址形态与域名，不压扁成裸
//!   IP 列表（计划 §3.1）。
//! - **不静默丢弃**：非 `L3VPN` 的 `AccessModel`、IPv6 条目、非法端口/端口区间、非法 CIDR、
//!   空 `Host`、未知 `Protocol`、非法域名通配形态、无法解析的节点地址都会生成
//!   [`SkippedEntry`]，带上字段路径、原文与 [`SkipReason`]。上游对这些只写 debug 日志。
//! - **未知字段**：JSON 里的未知字段被忽略（前向兼容）；未知的**取值**（访问模型、协议、
//!   节点类型）一律记录，不当作“没有这条”处理。
//! - **DNS 选项只是数据**：本模块不覆盖系统 DNS，宿主按设计 §7.3 只处理校园域名路径。
//! - **节点地址只是数据**：本模块不创建网卡、不写路由；宿主必须把
//!   [`ResourceCatalog::node_endpoint_ips`] 与 [`ResourceCatalog::node_endpoint_hosts`] 从
//!   校园路由/出口决策里排除以防回环（设计 §7.2）。**只有 IP 字面量能直接排除**：`{{sdpcHost}}`
//!   这类主机名端点必须先由宿主解析，解析结果由宿主自行排除——本 crate 不做解析，也不替宿主
//!   保证排除清单完整（上游 `parse.go` 的同一处也只移除 IP 字面量）。
//! - **查询是低频操作**：[`ResourceCatalog::dispatch`] 对每个目标线性遍历目录并克隆命中资源的
//!   字段，适合配置期/连接期查询；**按包查询必须由宿主先建索引**（上游为逐包路径建了区间树
//!   `internal/ipresource.Index`），本模块提供借用式入口 [`ResourceCatalog::dispatch_ref`] 与
//!   [`ResourceCatalog::entries`] 供该索引复用匹配谓词。
//! - **取值严格匹配**：`AccessModel` 必须是 `L3VPN`，`Protocol` 必须是 `tcp`/`udp`/`all`，
//!   与上游 `parse.go` 的精确比较一致（不 trim、不忽略大小写）；变体取值走 [`SkippedEntry`]
//!   而不是被猜成某个已知取值。仅节点 `Type` 例外：上游对它用 `strings.ToLower`，本模块同样
//!   接受 `WAN`/`Wan` 等形态。
//!
//! # 未匹配语义与命中顺序（设计 §7.1）
//!
//! [`ResourceCatalog::dispatch`] 区分四种结果，**没有任何一种**表示“服务端拒绝授权”：
//! 命中 L3 通道、需要 TCP 应用通道（TCP 且 `EnableTCPPrefL3 == false`）、地址在目录内但
//! 协议/端口未被覆盖、目标不属于任何资源。上游 `matchL3IPResource` 对 TCP 额外要求
//! `EnableTCPPrefL3`，因此“L3 未匹配”不能当作未授权。
//!
//! 两条通道的命中顺序**不同**，本模块照上游分开实现：
//!
//! | 通道 | 上游 | 取谁 | 本模块 |
//! |---|---|---|---|
//! | L3 | `matchL3IPResource` → `ipresource.Index.MatchWhere` | 按资源顺序**第一个**命中 | [`Dispatch::L3`] |
//! | TCP 应用通道 | `matchTCPIPResource` → `Index.MatchLastWhere` | 按资源顺序**最后一个**命中 | [`Dispatch::TcpApplicationChannel`] |
//!
//! 因此同一条 `ip/proto/port` 被多条资源覆盖时，两条通道可能选出不同的资源；统一取“首个命中”
//! 是错的。资源顺序 = JSON 里的 `AppInfo[].Apps[].AddressList[]` 遍历顺序。

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use serde::Deserialize;

use crate::error::ResourceError;

/// 资源正文的输入上限，超过即在解析前拒绝。
pub const RESOURCE_JSON_MAX_BYTES: usize = 8 * 1024 * 1024;

/// 节点地址未带端口时补的默认端口（上游 `:441`）。
pub const DEFAULT_NODE_PORT: u16 = 441;

/// 上游用网关地址替换的节点地址占位符。
pub const SDPC_HOST_PLACEHOLDER: &str = "{{sdpcHost}}";

/// 目录条目的协议约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceProtocol {
    /// `tcp`
    Tcp,
    /// `udp`
    Udp,
    /// `all`：同时覆盖 TCP 与 UDP（上游语义），ICMP 也只被 `all` 覆盖。
    All,
}

impl ResourceProtocol {
    /// 线上取值。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::All => "all",
        }
    }

    /// 解析线上取值；未知取值返回 `None`（调用点负责记录跳过原因）。
    ///
    /// **精确匹配**：只有 `tcp`/`udp`/`all` 三个字面量被接受，`TCP`、` tcp` 之类的变体一律
    /// 返回 `None`——与上游 `parse.go` 的 `address.Protocol == "tcp"` 比较一致，也符合本 crate
    /// 的严格解析定位（不猜服务端意图，变体走 [`SkippedEntry`]）。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// 该约束是否覆盖一个查询协议。
    #[must_use]
    pub fn covers(self, flow: FlowProtocol) -> bool {
        matches!(
            (self, flow),
            (Self::All, _) | (Self::Tcp, FlowProtocol::Tcp) | (Self::Udp, FlowProtocol::Udp)
        )
    }
}

impl fmt::Display for ResourceProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 查询用的流协议。`ICMP` 只出现在查询侧：目录里的取值只有 `tcp`/`udp`/`all`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowProtocol {
    /// TCP。
    Tcp,
    /// UDP。
    Udp,
    /// ICMP（无端口概念，上游对该协议跳过端口匹配）。
    Icmp,
}

/// 端口区间（闭区间）。单端口表示为 `min == max`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortRange {
    /// 下限（1 起）。
    pub min: u16,
    /// 上限。
    pub max: u16,
}

impl PortRange {
    /// 区间是否包含该端口。
    #[must_use]
    pub fn contains(self, port: u16) -> bool {
        (self.min..=self.max).contains(&port)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{}-{}", self.min, self.max)
        }
    }
}

/// 域名匹配形态。
///
/// 上游把 `*.example.edu` 归一成 `.example.edu`（含前导点）后按标签边界做后缀匹配，因此：
/// 通配形态**不**匹配裸域本身，只匹配至少多一级标签的主机；非通配形态按上游
/// `resolve/domain_index.go` 的语义同时匹配裸域与其子域。比较不区分大小写，并忽略末尾的点。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DomainPattern {
    /// 非通配：`example.edu` 匹配 `example.edu` 与 `x.example.edu`。
    Exact(String),
    /// `*.` 前缀：只匹配子域，不匹配裸域。
    Wildcard(String),
}

impl DomainPattern {
    /// 归一化后的域名（小写、去空格、去末尾点）。
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(domain) | Self::Wildcard(domain) => domain,
        }
    }

    /// 是否来自 `*.` 前缀。
    #[must_use]
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::Wildcard(_))
    }

    /// 主机名是否命中该形态。
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        let host = normalize_hostname(host);
        if host.is_empty() {
            return false;
        }
        let domain = self.as_str();
        if host == domain {
            return !self.is_wildcard();
        }
        // 标签边界：只有 `.domain` 结尾才算命中，`notexample.edu` 不算。
        host.len() > domain.len() + 1
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
    }
}

impl fmt::Display for DomainPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(domain) => f.write_str(domain),
            Self::Wildcard(domain) => write!(f, "*.{domain}"),
        }
    }
}

/// 资源地址形态。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceHost {
    /// 单个 IPv4 地址。
    Ip(Ipv4Addr),
    /// IPv4 网段（已按掩码归一，`last` 为广播地址）。
    Cidr {
        /// 网络地址。
        network: Ipv4Addr,
        /// 前缀长度。
        prefix_len: u8,
        /// 区间末地址。
        last: Ipv4Addr,
    },
    /// IPv4 地址区间（`ipMin-ipMax`）。
    Range {
        /// 区间首地址。
        first: Ipv4Addr,
        /// 区间末地址。
        last: Ipv4Addr,
    },
    /// 域名（含通配形态）。
    Domain(DomainPattern),
}

impl ResourceHost {
    /// 该地址形态是否覆盖给定 IPv4 地址。
    ///
    /// 域名形态本身不覆盖任何 IP；域名条目附带的解析地址在
    /// [`ResourceEntry::covers_ip`] 里一并参与匹配（上游把 `IP[]` 并入 IP 匹配集合）。
    #[must_use]
    pub fn covers_ip(&self, ip: Ipv4Addr) -> bool {
        match self {
            Self::Ip(address) => *address == ip,
            Self::Cidr {
                network,
                prefix_len,
                ..
            } => {
                let mask = prefix_mask(*prefix_len);
                (u32::from(ip) & mask) == (u32::from(*network) & mask)
            }
            Self::Range { first, last } => *first <= ip && ip <= *last,
            Self::Domain(_) => false,
        }
    }
}

impl fmt::Display for ResourceHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(address) => write!(f, "{address}"),
            Self::Cidr {
                network,
                prefix_len,
                ..
            } => write!(f, "{network}/{prefix_len}"),
            Self::Range { first, last } => write!(f, "{first}-{last}"),
            Self::Domain(pattern) => write!(f, "{pattern}"),
        }
    }
}

/// `AddrPretend` 的上游语义：显式 `false` 才表示关闭，缺省或任意非布尔值都按开启处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AddrPretend {
    /// 开启（缺省）。
    #[default]
    Enabled,
    /// 显式 `false`。
    Disabled,
}

impl AddrPretend {
    fn from_json(value: Option<&serde_json::Value>) -> Self {
        match value {
            Some(serde_json::Value::Bool(false)) => Self::Disabled,
            _ => Self::Enabled,
        }
    }

    /// 是否开启。
    #[must_use]
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// 一条资源约束（一个地址形态 + 协议 + 端口区间）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    /// 协议约束。
    pub protocol: ResourceProtocol,
    /// 端口区间。
    pub ports: PortRange,
    /// 地址形态。
    pub host: ResourceHost,
    /// 域名条目的 `IP[]`；上游把这批地址并入 IP 匹配集合，因此这里同样参与 IP 匹配。
    pub resolved_ips: Vec<Ipv4Addr>,
}

impl ResourceEntry {
    /// 目标 IP 是否落在该条目的地址（含域名条目附带的解析地址）内。
    #[must_use]
    pub fn covers_ip(&self, ip: Ipv4Addr) -> bool {
        self.host.covers_ip(ip) || self.resolved_ips.contains(&ip)
    }
}

/// 一个资源的约束集合（对应上游一个 `Apps[]` 项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceApp {
    /// `ID`。
    pub id: String,
    /// `NodeGroupID`；空串按“未指定”处理。
    pub node_group_id: Option<String>,
    /// `AccessModel`；进入目录的条目一定是 `L3VPN`。
    pub access_model: String,
    /// `EnableTCPPrefL3`：TCP 是否偏好走 L3（为 `false` 时需要 TCP 应用通道）。
    pub enable_tcp_pref_l3: bool,
    /// `AddrPretend`。
    pub addr_pretend: AddrPretend,
    /// 该资源的约束条目。可能为空：地址被逐条拒绝时由 [`SkippedEntry`] 解释原因。
    pub entries: Vec<ResourceEntry>,
}

/// 跳过原因。每一条被丢弃的取值都必须落到这里，不允许静默丢失。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// `AccessModel` 不是本实现支持的访问模型（上游只写 debug 日志）。
    UnsupportedAccessModel,
    /// `Protocol` 不是 `tcp`/`udp`/`all`。
    UnsupportedProtocol,
    /// 端口不是 1..=65535 的十进制整数。
    InvalidPort,
    /// 端口区间非法（形状不对、上下限非法或下限大于上限）。
    InvalidPortRange,
    /// CIDR 非法（地址或前缀长度不合法）。
    InvalidCidr,
    /// 地址区间非法（下限大于上限，或地址无法解析）。
    InvalidAddressRange,
    /// 域名条目的 `IP[]` 里有无法解析的地址。
    InvalidResolvedAddress,
    /// `Host` 为空。
    EmptyHost,
    /// 域名形态不被支持（例如 `*` 出现在非前缀位置）。
    UnsupportedDomainPattern,
    /// 地址形态不被支持（例如带端口的地址、含非法字符）。
    UnsupportedHostShape,
    /// IPv6 地址/网段：上游 IPv4-only，本实现同样不纳入，但显式记录。
    Ipv6Unsupported,
    /// 节点地址里的 `{{sdpcHost}}` 无法替换（未提供网关地址）。
    UnresolvedSdpcPlaceholder,
    /// 节点地址无法解析成 `host[:port]`。
    UnsupportedNodeAddress,
    /// 节点 `Type` 不是 `wan`/`lan`。
    UnsupportedNodeType,
}

impl SkipReason {
    /// 稳定机器码，供宿主 UI 与诊断报告映射。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::UnsupportedAccessModel => "unsupported-access-model",
            Self::UnsupportedProtocol => "unsupported-protocol",
            Self::InvalidPort => "invalid-port",
            Self::InvalidPortRange => "invalid-port-range",
            Self::InvalidCidr => "invalid-cidr",
            Self::InvalidAddressRange => "invalid-address-range",
            Self::InvalidResolvedAddress => "invalid-resolved-address",
            Self::EmptyHost => "empty-host",
            Self::UnsupportedDomainPattern => "unsupported-domain-pattern",
            Self::UnsupportedHostShape => "unsupported-host-shape",
            Self::Ipv6Unsupported => "ipv6-unsupported",
            Self::UnresolvedSdpcPlaceholder => "unresolved-sdpc-placeholder",
            Self::UnsupportedNodeAddress => "unsupported-node-address",
            Self::UnsupportedNodeType => "unsupported-node-type",
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// 一条被跳过的取值：字段路径 + 原文 + 原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    /// 所属 App 的 `ID`（若该记录与某个 App 相关）。
    pub app_id: Option<String>,
    /// 字段路径，例如 `AppInfo[0].Apps[3].AddressList[1].Port`。
    pub field: String,
    /// 原文（资源字段不是凭据材料，可按原文记录用于解释）。
    pub raw: String,
    /// 跳过原因。
    pub reason: SkipReason,
}

/// 节点类别（上游只有 `wan`/`lan`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeClass {
    /// `wan`
    Wan,
    /// `lan`
    Lan,
}

impl NodeClass {
    /// 线上取值。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wan => "wan",
            Self::Lan => "lan",
        }
    }
}

/// 一个已解析的节点地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAddress {
    /// 主机名或 IP 字面量（已完成 `{{sdpcHost}}` 替换）。
    pub host: String,
    /// 端口；未指定时为 [`DEFAULT_NODE_PORT`]。
    pub port: u16,
    /// 节点类别。
    pub class: NodeClass,
}

impl fmt::Display for NodeAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// 一个节点组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeGroup {
    /// 节点组 `ID`。
    pub id: String,
    /// 组内地址。
    pub addresses: Vec<NodeAddress>,
}

/// 解析选项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceParseOptions {
    /// 用于替换节点地址里 `{{sdpcHost}}` 的网关地址（上游用会话的 serverAddress）。
    pub sdpc_host: Option<String>,
}

impl ResourceParseOptions {
    /// 不带网关地址（占位符会被记录为跳过）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定网关地址用于占位符替换。
    #[must_use]
    pub fn with_sdpc_host(host: impl Into<String>) -> Self {
        Self {
            sdpc_host: Some(host.into()),
        }
    }
}

/// 一个匹配到的资源引用（路由/分派所需的最小字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    /// 资源 `ID`。
    pub app_id: String,
    /// 资源所属节点组。
    pub node_group_id: Option<String>,
    /// 命中的协议约束。
    pub protocol: ResourceProtocol,
    /// 命中的端口区间。
    pub ports: PortRange,
    /// 命中的地址形态。
    pub host: ResourceHost,
    /// 该资源是否允许 TCP 走 L3。
    pub enable_tcp_pref_l3: bool,
    /// 该资源的 `AddrPretend`。
    pub addr_pretend: AddrPretend,
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "app={} group={} {}:{}/{}",
            self.app_id,
            self.node_group_id.as_deref().unwrap_or("-"),
            self.host,
            self.ports,
            self.protocol
        )
    }
}

/// 目录内一条条目的借用句柄。
///
/// 只持有 [`ResourceCatalog`] 内部 App 与条目的引用，不复制任何字段。用途是给 T06/T07 的
/// **逐包路径**提供匹配原语：[`ResourceCatalog::dispatch`] 每次查询都会克隆命中资源的字段，
/// 适合低频/配置期调用；按包查询的宿主应先用 [`ResourceCatalog::entries`] 建自己的索引
/// （上游为同一目的建了区间树 `internal/ipresource.Index`），再用本句柄的
/// [`Self::covers_flow`] / [`Self::l3_eligible`] 判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceHandle<'a> {
    app: &'a ResourceApp,
    entry: &'a ResourceEntry,
}

impl<'a> ResourceHandle<'a> {
    fn new(app: &'a ResourceApp, entry: &'a ResourceEntry) -> Self {
        Self { app, entry }
    }

    /// 所属 App。
    #[must_use]
    pub fn app(self) -> &'a ResourceApp {
        self.app
    }

    /// 条目本身。
    #[must_use]
    pub fn entry(self) -> &'a ResourceEntry {
        self.entry
    }

    /// 资源 `ID`。
    #[must_use]
    pub fn app_id(self) -> &'a str {
        &self.app.id
    }

    /// 资源所属节点组。
    #[must_use]
    pub fn node_group_id(self) -> Option<&'a str> {
        self.app.node_group_id.as_deref()
    }

    /// 条目的协议约束。
    #[must_use]
    pub fn protocol(self) -> ResourceProtocol {
        self.entry.protocol
    }

    /// 条目的端口区间。
    #[must_use]
    pub fn ports(self) -> PortRange {
        self.entry.ports
    }

    /// 条目的地址形态。
    #[must_use]
    pub fn host(self) -> &'a ResourceHost {
        &self.entry.host
    }

    /// 该资源是否允许 TCP 走 L3。
    #[must_use]
    pub fn enable_tcp_pref_l3(self) -> bool {
        self.app.enable_tcp_pref_l3
    }

    /// 该资源的 `AddrPretend`。
    #[must_use]
    pub fn addr_pretend(self) -> AddrPretend {
        self.app.addr_pretend
    }

    /// 条目地址（含域名条目附带的解析地址）是否覆盖目标 IP。
    #[must_use]
    pub fn covers_ip(self, ip: Ipv4Addr) -> bool {
        self.entry.covers_ip(ip)
    }

    /// 条目是否在协议/端口上覆盖目标（**地址匹配由调用方的索引负责**）。
    ///
    /// 谓词与上游 `ipresource.match` 一致：协议相同或条目取值为 `all`；ICMP 不参与端口匹配。
    #[must_use]
    pub fn covers_flow(self, target: Target) -> bool {
        if !self.entry.protocol.covers(target.protocol) {
            return false;
        }
        match (target.protocol, target.port) {
            // ICMP 无端口：上游对该协议跳过端口匹配。
            (FlowProtocol::Icmp, _) => true,
            (_, Some(port)) => self.entry.ports.contains(port),
            (_, None) => false,
        }
    }

    /// 该条目在该协议上是否可走 L3（上游 `matchL3IPResource` 的 accept 谓词：TCP 还要求
    /// `EnableTCPPrefL3 == true`）。
    #[must_use]
    pub fn l3_eligible(self, protocol: FlowProtocol) -> bool {
        match protocol {
            FlowProtocol::Tcp => self.app.enable_tcp_pref_l3,
            FlowProtocol::Udp | FlowProtocol::Icmp => true,
        }
    }

    /// 转成拥有所有权的 [`ResourceRef`]（低频路径用；逐包路径应继续用借用句柄）。
    #[must_use]
    pub fn to_resource_ref(self) -> ResourceRef {
        ResourceRef {
            app_id: self.app.id.clone(),
            node_group_id: self.app.node_group_id.clone(),
            protocol: self.entry.protocol,
            ports: self.entry.ports,
            host: self.entry.host.clone(),
            enable_tcp_pref_l3: self.app.enable_tcp_pref_l3,
            addr_pretend: self.app.addr_pretend,
        }
    }
}

/// 目标流的查询形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// 目的 IPv4 地址。
    pub ip: Ipv4Addr,
    /// 目的协议。
    pub protocol: FlowProtocol,
    /// 目的端口；ICMP 无端口，忽略该字段。
    pub port: Option<u16>,
}

impl Target {
    /// TCP 目标。
    #[must_use]
    pub fn tcp(ip: Ipv4Addr, port: u16) -> Self {
        Self {
            ip,
            protocol: FlowProtocol::Tcp,
            port: Some(port),
        }
    }

    /// UDP 目标。
    #[must_use]
    pub fn udp(ip: Ipv4Addr, port: u16) -> Self {
        Self {
            ip,
            protocol: FlowProtocol::Udp,
            port: Some(port),
        }
    }

    /// ICMP 目标（无端口）。
    #[must_use]
    pub fn icmp(ip: Ipv4Addr) -> Self {
        Self {
            ip,
            protocol: FlowProtocol::Icmp,
            port: None,
        }
    }
}

/// 分派结论。
///
/// **四种结论都不是“服务端拒绝授权”**：目录只说明客户端可以尝试什么，服务端是否放行由真实
/// 数据面结果决定（设计 §7.1）。宿主不得把 [`Self::NotInCatalog`] 显示成“未授权”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// 命中 L3 通道：该条目允许这种协议/端口走 L3。
    L3(ResourceRef),
    /// TCP 流命中资源，但该资源的 `EnableTCPPrefL3 == false`：需要 TCP 应用通道（T08）。
    TcpApplicationChannel(ResourceRef),
    /// 目标地址在目录里，但没有条目覆盖该协议/端口组合。
    PortNotCovered {
        /// 覆盖了该地址的条目（协议/端口未命中）。
        address_hits: Vec<ResourceRef>,
    },
    /// 目标不属于任何资源：可能不在用户选择范围、可能资源发现不完整，**不是**未授权。
    NotInCatalog,
}

impl Dispatch {
    /// 稳定机器码，供宿主 UI 与诊断映射。
    #[must_use]
    pub fn reason(&self) -> DispatchReason {
        match self {
            Self::L3(_) => DispatchReason::L3,
            Self::TcpApplicationChannel(_) => DispatchReason::TcpApplicationChannel,
            Self::PortNotCovered { .. } => DispatchReason::PortNotCovered,
            Self::NotInCatalog => DispatchReason::NotInCatalog,
        }
    }

    /// 命中的资源（[`Self::L3`] 与 [`Self::TcpApplicationChannel`]）。
    #[must_use]
    pub fn resource(&self) -> Option<&ResourceRef> {
        match self {
            Self::L3(resource) | Self::TcpApplicationChannel(resource) => Some(resource),
            _ => None,
        }
    }
}

/// [`ResourceCatalog::dispatch_ref`] 的结论：与 [`Dispatch`] 同构，但携带借用句柄而非克隆字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchRef<'a> {
    /// 命中 L3 通道。
    L3(ResourceHandle<'a>),
    /// TCP 流命中资源，但该资源的 `EnableTCPPrefL3 == false`：需要 TCP 应用通道（T08）。
    TcpApplicationChannel(ResourceHandle<'a>),
    /// 目标地址在目录里，但没有条目覆盖该协议/端口组合。
    PortNotCovered {
        /// 覆盖了该地址的条目（协议/端口未命中）。
        address_hits: Vec<ResourceHandle<'a>>,
    },
    /// 目标不属于任何资源：**不是**未授权。
    NotInCatalog,
}

impl<'a> DispatchRef<'a> {
    /// 稳定机器码，与 [`Dispatch::reason`] 一致。
    #[must_use]
    pub fn reason(&self) -> DispatchReason {
        match self {
            Self::L3(_) => DispatchReason::L3,
            Self::TcpApplicationChannel(_) => DispatchReason::TcpApplicationChannel,
            Self::PortNotCovered { .. } => DispatchReason::PortNotCovered,
            Self::NotInCatalog => DispatchReason::NotInCatalog,
        }
    }

    /// 命中的资源句柄（[`Self::L3`] 与 [`Self::TcpApplicationChannel`]）。
    #[must_use]
    pub fn handle(&self) -> Option<ResourceHandle<'a>> {
        match self {
            Self::L3(handle) | Self::TcpApplicationChannel(handle) => Some(*handle),
            _ => None,
        }
    }

    /// 转成拥有所有权的 [`Dispatch`]。
    #[must_use]
    pub fn to_dispatch(&self) -> Dispatch {
        match self {
            Self::L3(handle) => Dispatch::L3(handle.to_resource_ref()),
            Self::TcpApplicationChannel(handle) => {
                Dispatch::TcpApplicationChannel(handle.to_resource_ref())
            }
            Self::PortNotCovered { address_hits } => Dispatch::PortNotCovered {
                address_hits: address_hits
                    .iter()
                    .map(|handle| handle.to_resource_ref())
                    .collect(),
            },
            Self::NotInCatalog => Dispatch::NotInCatalog,
        }
    }
}

/// [`Dispatch`] 的机器可读原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispatchReason {
    /// 走 L3 通道。
    L3,
    /// 需要 TCP 应用通道。
    TcpApplicationChannel,
    /// 地址在目录内但协议/端口未被覆盖。
    PortNotCovered,
    /// 不属于任何资源。
    NotInCatalog,
}

impl DispatchReason {
    /// 稳定机器码。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::L3 => "l3",
            Self::TcpApplicationChannel => "tcp-application-channel",
            Self::PortNotCovered => "port-not-covered",
            Self::NotInCatalog => "not-in-catalog",
        }
    }
}

impl fmt::Display for DispatchReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// 解析后的资源目录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceCatalog {
    apps: Vec<ResourceApp>,
    skipped: Vec<SkippedEntry>,
    node_groups: Vec<NodeGroup>,
    major_node_group: Option<String>,
    dns_servers: Vec<String>,
    node_endpoint_ips: Vec<Ipv4Addr>,
    node_endpoint_hosts: Vec<String>,
}

impl ResourceCatalog {
    /// 从服务端响应正文解析。
    ///
    /// `options.sdpc_host` 用于替换节点地址里的 `{{sdpcHost}}`。
    ///
    /// # Errors
    ///
    /// 正文超过 [`RESOURCE_JSON_MAX_BYTES`] 或不是合法 JSON 时返回 [`ResourceError`]。
    /// 单条资源不可用不会导致整体失败，而是进入 [`Self::skipped`]。
    pub fn from_json(body: &[u8], options: &ResourceParseOptions) -> Result<Self, ResourceError> {
        if body.len() > RESOURCE_JSON_MAX_BYTES {
            return Err(ResourceError::BodyTooLarge {
                bytes: body.len(),
                limit: RESOURCE_JSON_MAX_BYTES,
            });
        }
        let raw: RawBody = serde_json::from_slice(body).map_err(|error| ResourceError::Json {
            line: error.line(),
            column: error.column(),
        })?;
        Ok(Self::from_raw(&raw, options))
    }

    /// 从 UTF-8 正文解析，见 [`Self::from_json`]。
    ///
    /// # Errors
    ///
    /// 同 [`Self::from_json`]。
    pub fn from_json_str(body: &str, options: &ResourceParseOptions) -> Result<Self, ResourceError> {
        Self::from_json(body.as_bytes(), options)
    }

    /// 进入目录的资源。
    #[must_use]
    pub fn apps(&self) -> &[ResourceApp] {
        &self.apps
    }

    /// 被跳过的取值，按解析顺序。
    #[must_use]
    pub fn skipped(&self) -> &[SkippedEntry] {
        &self.skipped
    }

    /// 节点组。
    #[must_use]
    pub fn node_groups(&self) -> &[NodeGroup] {
        &self.node_groups
    }

    /// 主节点组 `ID`。
    #[must_use]
    pub fn major_node_group(&self) -> Option<&str> {
        self.major_node_group.as_deref()
    }

    /// 指定节点组的地址（未知组返回 `None`）。
    #[must_use]
    pub fn node_group_addresses(&self, node_group_id: &str) -> Option<&[NodeAddress]> {
        self.node_groups
            .iter()
            .find(|group| group.id == node_group_id)
            .map(|group| group.addresses.as_slice())
    }

    /// 服务端给出的 DNS 服务器（`DNSOption` 为空时取 `DNSOptionV2`，上游语义）。
    ///
    /// 宿主**不得**据此覆盖系统 DNS；只用于校园域名的按需解析路径（设计 §7.3）。
    #[must_use]
    pub fn dns_servers(&self) -> &[String] {
        &self.dns_servers
    }

    /// 数据节点端点里**以 IP 字面量给出**的 IPv4 地址。
    ///
    /// 宿主必须把这批地址从校园路由与出口决策里排除，避免节点流量再次进入自己的隧道
    /// （上游用同样的“从路由集合里移除节点 IP”策略；设计 §7.2 要求复用宿主既有归属机制）。
    ///
    /// **这份列表不是节点地址全集**：它只覆盖 IP 字面量端点。主机名端点——包括
    /// `{{sdpcHost}}` 替换后得到的网关域名、以及服务端直接下发的主机名——都在
    /// [`Self::node_endpoint_hosts`] 里，宿主必须在**解析之后**按实际解析结果自行排除。
    /// 只排除本函数返回的地址会让节点流量经域名端点回环到自己的隧道（上游 `parse.go` 的
    /// 同一处也只移除 IP 字面量，本 crate 不重复它的漏洞，但也不替宿主做解析）。
    #[must_use]
    pub fn node_endpoint_ips(&self) -> &[Ipv4Addr] {
        &self.node_endpoint_ips
    }

    /// 数据节点端点里**不是 IP 字面量**的主机名（去重，按首次出现顺序；比较不区分大小写）。
    ///
    /// 典型来源是 `{{sdpcHost}}` 替换后的网关域名与 `{{sdpcHost}}:port` 形态。宿主必须自行解析
    /// 这些主机名（经注入的 [`crate::transport::Underlay`]），把解析结果一并从校园路由与出口
    /// 决策里排除；本 crate 不解析、不缓存解析结果（见 [`Self::node_endpoint_ips`]）。
    #[must_use]
    pub fn node_endpoint_hosts(&self) -> &[String] {
        &self.node_endpoint_hosts
    }

    /// 条目总数。
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.apps.iter().map(|app| app.entries.len()).sum()
    }

    /// 目录里是否没有任何条目。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entry_count() == 0
    }

    /// 明确记录过的跳过数量。
    #[must_use]
    pub fn skipped_count(&self) -> usize {
        self.skipped.len()
    }

    /// 按目录顺序遍历全部条目（借用句柄，不克隆字段）。
    ///
    /// 供 T06/T07 建立按包查询所需的索引；顺序 = `AppInfo[].Apps[].AddressList[]`。
    pub fn entries(&self) -> impl Iterator<Item = ResourceHandle<'_>> {
        self.apps.iter().flat_map(|app| {
            app.entries
                .iter()
                .map(move |entry| ResourceHandle::new(app, entry))
        })
    }

    /// 借用式分派：与 [`Self::dispatch`] 同一套匹配规则，但不克隆命中资源的字段。
    ///
    /// 命中顺序照上游分通道实现（见模块文档的表格）：L3 取**第一个**命中
    /// （`ipresource.Index.MatchWhere`），TCP 应用通道取**最后一个**命中（`MatchLastWhere`）；
    /// TCP 走 L3 还要求 `EnableTCPPrefL3`，ICMP 不参与端口匹配。
    ///
    /// **复杂度与用途**：每个目标对目录做一次线性遍历（O(条目数)），是**低频/配置期查询**。
    /// 上游为逐包路径另建区间树（`internal/ipresource.Index`）；逐包查询的宿主应先用
    /// [`Self::entries`] 建索引，再对少量候选调用 [`ResourceHandle::covers_flow`] 等谓词，
    /// 不要对每个包调用本函数。
    #[must_use]
    pub fn dispatch_ref(&self, target: Target) -> DispatchRef<'_> {
        let mut first_l3: Option<ResourceHandle<'_>> = None;
        let mut last_tcp_channel: Option<ResourceHandle<'_>> = None;
        let mut address_hits: Vec<ResourceHandle<'_>> = Vec::new();

        for handle in self.entries() {
            if !handle.covers_ip(target.ip) {
                continue;
            }
            if !handle.covers_flow(target) {
                address_hits.push(handle);
                continue;
            }
            if handle.l3_eligible(target.protocol) {
                if first_l3.is_none() {
                    first_l3 = Some(handle);
                }
            } else {
                // TCP 应用通道：照上游 `matchTCPIPResource` → `MatchLastWhere` 取最后一个命中。
                last_tcp_channel = Some(handle);
            }
        }

        if let Some(handle) = first_l3 {
            return DispatchRef::L3(handle);
        }
        if let Some(handle) = last_tcp_channel {
            return DispatchRef::TcpApplicationChannel(handle);
        }
        if !address_hits.is_empty() {
            return DispatchRef::PortNotCovered { address_hits };
        }
        DispatchRef::NotInCatalog
    }

    /// 查询目标的分派结论（拥有所有权的形态），见 [`Dispatch`]。
    ///
    /// 与 [`Self::dispatch_ref`] 共用同一套匹配规则，只是把命中资源的字段克隆出来；逐包路径
    /// 应改用 [`Self::dispatch_ref`] 或自建索引。
    #[must_use]
    pub fn dispatch(&self, target: Target) -> Dispatch {
        self.dispatch_ref(target).to_dispatch()
    }

    fn from_raw(raw: &RawBody, options: &ResourceParseOptions) -> Self {
        let mut catalog = Self::default();
        catalog.parse_dns(&raw.data.sdp_policy);
        catalog.parse_apps(&raw.data.app_list.data);
        catalog.parse_node_groups(&raw.data.app_list.data.config, options);
        catalog
    }

    fn parse_dns(&mut self, policy: &RawSdpPolicy) {
        let mut option = &policy.data.client_option.dns_option;
        if option.first.trim().is_empty() {
            option = &policy.data.client_option.dns_option_v2;
        }
        for server in [&option.first, &option.second] {
            let server = server.trim();
            if !server.is_empty() {
                self.dns_servers.push(server.to_owned());
            }
        }
    }

    fn parse_apps(&mut self, app_list: &RawAppListData) {
        for (app_info_index, app_info) in app_list.app_info.iter().enumerate() {
            for (app_index, app) in app_info.apps.iter().enumerate() {
                let path = format!("AppInfo[{app_info_index}].Apps[{app_index}]");
                let app_id = app.id.trim().to_owned();
                // 上游 `parse.go` 用精确比较（`appItem.AccessModel != "L3VPN"`）：`l3vpn` 之类的
                // 变体不被猜成 L3VPN，而是按未知访问模型记录跳过。
                if app.access_model != "L3VPN" {
                    self.skipped.push(SkippedEntry {
                        app_id: Some(app_id),
                        field: format!("{path}.AccessModel"),
                        raw: app.access_model.clone(),
                        reason: SkipReason::UnsupportedAccessModel,
                    });
                    continue;
                }
                let mut entries = Vec::new();
                for (address_index, address) in app.address_list.iter().enumerate() {
                    let entry_path = format!("{path}.AddressList[{address_index}]");
                    self.parse_address(&app_id, &entry_path, address, &mut entries);
                }
                self.apps.push(ResourceApp {
                    id: app_id,
                    node_group_id: empty_to_none(&app.node_group_id),
                    access_model: app.access_model.clone(),
                    enable_tcp_pref_l3: app.enable_tcp_pref_l3,
                    addr_pretend: AddrPretend::from_json(app.addr_pretend.as_ref()),
                    entries,
                });
            }
        }
    }

    fn parse_address(
        &mut self,
        app_id: &str,
        entry_path: &str,
        address: &RawAddress,
        entries: &mut Vec<ResourceEntry>,
    ) {
        let Some(protocol) = ResourceProtocol::parse(&address.protocol) else {
            self.skipped.push(SkippedEntry {
                app_id: Some(app_id.to_owned()),
                field: format!("{entry_path}.Protocol"),
                raw: address.protocol.clone(),
                reason: SkipReason::UnsupportedProtocol,
            });
            return;
        };
        let ports = match parse_ports(&address.port) {
            Ok(ports) => ports,
            Err(reason) => {
                self.skipped.push(SkippedEntry {
                    app_id: Some(app_id.to_owned()),
                    field: format!("{entry_path}.Port"),
                    raw: address.port.clone(),
                    reason,
                });
                return;
            }
        };
        let host = match parse_host(&address.host) {
            Ok(host) => host,
            Err(reason) => {
                self.skipped.push(SkippedEntry {
                    app_id: Some(app_id.to_owned()),
                    field: format!("{entry_path}.Host"),
                    raw: address.host.clone(),
                    reason,
                });
                return;
            }
        };

        let mut resolved_ips = Vec::new();
        if matches!(host, ResourceHost::Domain(_)) {
            for (ip_index, raw_ip) in address.ip.iter().enumerate() {
                if let Ok(ip) = Ipv4Addr::from_str(raw_ip.trim()) {
                    resolved_ips.push(ip);
                } else {
                    let reason = if Ipv6Addr::from_str(raw_ip.trim()).is_ok() {
                        SkipReason::Ipv6Unsupported
                    } else {
                        SkipReason::InvalidResolvedAddress
                    };
                    self.skipped.push(SkippedEntry {
                        app_id: Some(app_id.to_owned()),
                        field: format!("{entry_path}.IP[{ip_index}]"),
                        raw: raw_ip.clone(),
                        reason,
                    });
                }
            }
        }
        // 非域名条目上的 `IP[]` 与 DNS 规则无关，上游对它只写一条 debug 日志（不入目录也不报错）；
        // 它不改变任何约束，因此这里不产生跳过记录。

        entries.push(ResourceEntry {
            protocol,
            ports,
            host,
            resolved_ips,
        });
    }

    fn parse_node_groups(&mut self, config: &RawConfig, options: &ResourceParseOptions) {
        self.major_node_group = empty_to_none(&config.node_group_conf.major_node_group.id);
        for (group_index, group) in config.node_group_conf.node_group_list.iter().enumerate() {
            let mut addresses = Vec::new();
            for (address_index, info) in group.address_info.iter().enumerate() {
                let path = format!("NodeGroupList[{group_index}].AddressInfo[{address_index}].Address");
                let class = match info.node_type.trim().to_ascii_lowercase().as_str() {
                    "wan" => NodeClass::Wan,
                    "lan" => NodeClass::Lan,
                    _ => {
                        self.skipped.push(SkippedEntry {
                            app_id: None,
                            field: format!(
                                "NodeGroupList[{group_index}].AddressInfo[{address_index}].Type"
                            ),
                            raw: info.node_type.clone(),
                            reason: SkipReason::UnsupportedNodeType,
                        });
                        continue;
                    }
                };
                let raw_address = info.address.trim();
                if raw_address.is_empty() {
                    self.skipped.push(SkippedEntry {
                        app_id: None,
                        field: path,
                        raw: info.address.clone(),
                        reason: SkipReason::UnsupportedNodeAddress,
                    });
                    continue;
                }
                let resolved = if raw_address.contains(SDPC_HOST_PLACEHOLDER) {
                    if let Some(host) = options.sdpc_host.as_deref() {
                        raw_address.replace(SDPC_HOST_PLACEHOLDER, host)
                    } else {
                        self.skipped.push(SkippedEntry {
                            app_id: None,
                            field: path,
                            raw: info.address.clone(),
                            reason: SkipReason::UnresolvedSdpcPlaceholder,
                        });
                        continue;
                    }
                } else {
                    raw_address.to_owned()
                };
                let (host, port) = match split_node_endpoint(&resolved) {
                    Ok(endpoint) => endpoint,
                    Err(reason) => {
                        self.skipped.push(SkippedEntry {
                            app_id: None,
                            field: path,
                            raw: info.address.clone(),
                            reason,
                        });
                        continue;
                    }
                };
                if let Ok(ip) = Ipv4Addr::from_str(&host) {
                    if !self.node_endpoint_ips.contains(&ip) {
                        self.node_endpoint_ips.push(ip);
                    }
                } else if !self
                    .node_endpoint_hosts
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&host))
                {
                    // 主机名端点（含 `{{sdpcHost}}` 替换结果）：本 crate 不解析，只交给宿主。
                    self.node_endpoint_hosts.push(host.clone());
                }
                addresses.push(NodeAddress { host, port, class });
            }
            self.node_groups.push(NodeGroup {
                id: group.id.trim().to_owned(),
                addresses,
            });
        }
    }
}

fn empty_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn prefix_mask(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len.min(32)))
    }
}

fn normalize_hostname(host: &str) -> String {
    let trimmed = host.trim().trim_end_matches('.');
    trimmed.to_ascii_lowercase()
}

fn parse_port(raw: &str) -> Option<u16> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value: u32 = trimmed.parse().ok()?;
    if (1..=u32::from(u16::MAX)).contains(&value) {
        u16::try_from(value).ok()
    } else {
        None
    }
}

fn parse_ports(raw: &str) -> Result<PortRange, SkipReason> {
    let port = raw.trim();
    if port.is_empty() {
        return Err(SkipReason::InvalidPort);
    }
    if let Some((low, high)) = port.split_once('-') {
        if high.contains('-') {
            return Err(SkipReason::InvalidPortRange);
        }
        let min = parse_port(low).ok_or(SkipReason::InvalidPortRange)?;
        let max = parse_port(high).ok_or(SkipReason::InvalidPortRange)?;
        if min > max {
            return Err(SkipReason::InvalidPortRange);
        }
        return Ok(PortRange { min, max });
    }
    let single = parse_port(port).ok_or(SkipReason::InvalidPort)?;
    Ok(PortRange {
        min: single,
        max: single,
    })
}

fn parse_host(raw: &str) -> Result<ResourceHost, SkipReason> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(SkipReason::EmptyHost);
    }
    if let Ok(ip) = Ipv4Addr::from_str(host) {
        return Ok(ResourceHost::Ip(ip));
    }
    if Ipv6Addr::from_str(host).is_ok() {
        return Err(SkipReason::Ipv6Unsupported);
    }
    if let Some((address, prefix)) = host.split_once('/') {
        if address.contains(':') || prefix.contains(':') {
            return Err(SkipReason::Ipv6Unsupported);
        }
        let Ok(ip) = Ipv4Addr::from_str(address.trim()) else {
            return Err(SkipReason::InvalidCidr);
        };
        let Ok(prefix_len) = prefix.trim().parse::<u8>() else {
            return Err(SkipReason::InvalidCidr);
        };
        if prefix_len > 32 {
            return Err(SkipReason::InvalidCidr);
        }
        let mask = prefix_mask(prefix_len);
        let network = Ipv4Addr::from(u32::from(ip) & mask);
        let last = Ipv4Addr::from(u32::from(network) | !mask);
        return Ok(ResourceHost::Cidr {
            network,
            prefix_len,
            last,
        });
    }
    // 恰好一个 `-` 时才可能是地址区间；域名本身允许包含 `-`。
    if host.matches('-').count() == 1
        && let Some((low, high)) = host.split_once('-')
    {
        let low_trimmed = low.trim();
        let high_trimmed = high.trim();
        if let (Ok(first), Ok(last)) = (
            Ipv4Addr::from_str(low_trimmed),
            Ipv4Addr::from_str(high_trimmed),
        ) {
            if first > last {
                return Err(SkipReason::InvalidAddressRange);
            }
            return Ok(ResourceHost::Range { first, last });
        }
        if Ipv6Addr::from_str(low_trimmed).is_ok() && Ipv6Addr::from_str(high_trimmed).is_ok() {
            return Err(SkipReason::Ipv6Unsupported);
        }
        if low_trimmed.contains(':') || high_trimmed.contains(':') {
            return Err(SkipReason::UnsupportedHostShape);
        }
        // 落回域名路径。
    }
    if host.contains(':') {
        // 带端口的地址或 IPv6 残余形态：不是本实现接受的 `Host` 取值。
        return Err(SkipReason::UnsupportedHostShape);
    }
    if let Some(domain) = normalize_policy_domain(host) {
        return Ok(ResourceHost::Domain(domain));
    }
    Err(SkipReason::UnsupportedDomainPattern)
}

/// 上游 `normalizePolicyDomain`：`*.x` → 通配 `x`（上游存成 `.x`），含 `*` 的其它形态拒绝。
fn normalize_policy_domain(raw: &str) -> Option<DomainPattern> {
    let host = raw.trim();
    if host.is_empty() || host.contains(' ') {
        return None;
    }
    if let Some(suffix) = host.strip_prefix("*.") {
        let normalized = normalize_hostname(suffix);
        if normalized.is_empty() {
            return None;
        }
        return Some(DomainPattern::Wildcard(normalized));
    }
    if host.contains('*') {
        return None;
    }
    let normalized = normalize_hostname(host);
    if normalized.is_empty() {
        return None;
    }
    Some(DomainPattern::Exact(normalized))
}

/// 把节点地址拆成 `host:port`；未带端口时补 [`DEFAULT_NODE_PORT`]。
///
/// 与上游的 `strings.Contains(address, ":")` 判断不同：IPv6 字面量与 `[..]` 形态会显式记录为
/// IPv6 不支持，而不是被误当作“已带端口”。
fn split_node_endpoint(raw: &str) -> Result<(String, u16), SkipReason> {
    if raw.starts_with('[') || raw.matches(':').count() > 1 {
        return Err(SkipReason::Ipv6Unsupported);
    }
    if Ipv6Addr::from_str(raw).is_ok() {
        return Err(SkipReason::Ipv6Unsupported);
    }
    if let Some((host, port)) = raw.split_once(':') {
        let host = host.trim();
        if host.is_empty() {
            return Err(SkipReason::UnsupportedNodeAddress);
        }
        let port = parse_port(port).ok_or(SkipReason::UnsupportedNodeAddress)?;
        return Ok((host.to_owned(), port));
    }
    let host = raw.trim();
    if host.is_empty() || host.contains(' ') {
        return Err(SkipReason::UnsupportedNodeAddress);
    }
    Ok((host.to_owned(), DEFAULT_NODE_PORT))
}

#[derive(Debug, Default, Deserialize)]
struct RawBody {
    #[serde(default, rename = "Data", alias = "data")]
    data: RawData,
}

#[derive(Debug, Default, Deserialize)]
struct RawData {
    #[serde(default, rename = "AppList", alias = "appList")]
    app_list: RawAppList,
    #[serde(default, rename = "SDPPolicy", alias = "sdpPolicy")]
    sdp_policy: RawSdpPolicy,
}

#[derive(Debug, Default, Deserialize)]
struct RawAppList {
    #[serde(default, rename = "Data", alias = "data")]
    data: RawAppListData,
}

#[derive(Debug, Default, Deserialize)]
struct RawAppListData {
    #[serde(default, rename = "AppInfo", alias = "appInfo")]
    app_info: Vec<RawAppInfo>,
    #[serde(default, rename = "Config", alias = "config")]
    config: RawConfig,
}

#[derive(Debug, Default, Deserialize)]
struct RawAppInfo {
    #[serde(default, rename = "Apps", alias = "apps")]
    apps: Vec<RawApp>,
}

#[derive(Debug, Default, Deserialize)]
struct RawApp {
    #[serde(default, rename = "ID", alias = "Id", alias = "id")]
    id: String,
    #[serde(default, rename = "NodeGroupID", alias = "NodeGroupId", alias = "nodeGroupId")]
    node_group_id: String,
    #[serde(default, rename = "AccessModel", alias = "accessModel")]
    access_model: String,
    #[serde(
        default,
        rename = "EnableTCPPrefL3",
        alias = "EnableTcpPrefL3",
        alias = "enableTCPPrefL3"
    )]
    enable_tcp_pref_l3: bool,
    #[serde(default, rename = "AddrPretend", alias = "addrPretend")]
    addr_pretend: Option<serde_json::Value>,
    #[serde(default, rename = "AddressList", alias = "addressList")]
    address_list: Vec<RawAddress>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAddress {
    #[serde(default, rename = "Protocol", alias = "protocol")]
    protocol: String,
    #[serde(default, rename = "Port", alias = "port")]
    port: String,
    #[serde(default, rename = "Host", alias = "host")]
    host: String,
    #[serde(default, rename = "IP", alias = "Ip", alias = "ip")]
    ip: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default, rename = "NodeGroupConf", alias = "nodeGroupConf")]
    node_group_conf: RawNodeGroupConf,
}

#[derive(Debug, Default, Deserialize)]
struct RawNodeGroupConf {
    #[serde(default, rename = "MajorNodeGroup", alias = "majorNodeGroup")]
    major_node_group: RawMajorNodeGroup,
    #[serde(default, rename = "NodeGroupList", alias = "nodeGroupList")]
    node_group_list: Vec<RawNodeGroup>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMajorNodeGroup {
    #[serde(default, rename = "ID", alias = "Id", alias = "id")]
    id: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawNodeGroup {
    #[serde(default, rename = "ID", alias = "Id", alias = "id")]
    id: String,
    #[serde(default, rename = "AddressInfo", alias = "addressInfo")]
    address_info: Vec<RawAddressInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAddressInfo {
    #[serde(default, rename = "Address", alias = "address")]
    address: String,
    #[serde(default, rename = "Type", alias = "type")]
    node_type: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawSdpPolicy {
    #[serde(default, rename = "Data", alias = "data")]
    data: RawSdpData,
}

#[derive(Debug, Default, Deserialize)]
struct RawSdpData {
    #[serde(default, rename = "ClientOption", alias = "clientOption")]
    client_option: RawClientOption,
}

#[derive(Debug, Default, Deserialize)]
struct RawClientOption {
    #[serde(default, rename = "DNSOption", alias = "dnsOption")]
    dns_option: RawDnsOption,
    #[serde(default, rename = "DNSOptionV2", alias = "dnsOptionV2")]
    dns_option_v2: RawDnsOption,
}

#[derive(Debug, Default, Deserialize)]
struct RawDnsOption {
    #[serde(default, rename = "FirstDNS", alias = "FirstDns", alias = "firstDNS")]
    first: String,
    #[serde(default, rename = "SecondDNS", alias = "SecondDns", alias = "secondDNS")]
    second: String,
}

