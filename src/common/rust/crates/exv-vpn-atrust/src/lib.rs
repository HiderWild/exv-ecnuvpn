
//! aTrust 协议库：平台无关的线上消息、资源约束与会话关闭语义。
//!
//! # 本 crate 负责什么
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`error`] | 全部 typed 错误与唯一的脱敏约定（敏感材料只以占位符出现） |
//! | [`resource`] | 服务端 `clientResource` JSON → 保留约束的 [`resource::ResourceCatalog`]，带原因的跳过记录与未匹配语义查询 |
//! | [`transport`] | 注入式 [`transport::Underlay`] 出口端口 |
//! | [`transport::l3::codec`] | L3 帧编解码：显式消息边界、长度校验、畸形输入拒绝、分片/粘包流式解析 |
//! | [`session`] | 会话关闭的幂等契约与有界排队预算：重复关闭、关闭后排空、迟到结果无法进入新会话、超预算入队被拒绝 |
//!
//! # 本 crate 绝不做什么
//!
//! 协议库不创建或修改系统网卡、路由、系统 DNS、系统代理，不注册浏览器协议处理器，不启动
//! 浏览器，不启动或依赖官方 aTrust / Go 上游运行时。库内**所有**出站连接必须经由注入的
//! [`transport::Underlay`] 完成；本 crate 不提供默认拨号器，也不做任何隐式解析或直连。
//! 外部浏览器请求沿用浏览器自身的网络，不受该端口约束（设计 §3.1）。
//!
//! 宿主的出口决策（VGDC/binder、防回环、节点地址排除）由宿主实现；本 crate 只把节点地址
//! 作为数据交给宿主，见 [`resource::ResourceCatalog::node_endpoint_ips`] 与
//! [`resource::ResourceCatalog::node_endpoint_hosts`]。**排除清单不等价于“节点往来一定安全”**：
//! `node_endpoint_ips` 只收 IP 字面量端点，主机名端点（含 `{{sdpcHost}}` 替换后的网关域名）
//! 必须由宿主在解析后自行排除，宿主不得把本 crate 的列表当作节点地址全集。
//!
//! # 未接线能力与限制（活动路径 / 未接线原因 / 重新接线条件）
//!
//! T03 只交付可独立测试的协议数据与语义，下列能力**故意没有**在本 crate 中留空壳；表中也
//! 登记已经落地但只覆盖一部分语义的能力（例如帧与队列里的 token/载荷不进 `Secret`），
//! 免得未落地部分从清单里消失。接线状态按宿主区分：Windows 生产入口已接入，Darwin 未接线：
//!
//! | 能力 | 活动路径 | 未接线原因 | 重新接线条件 |
//! |---|---|---|---|
//! | 认证入口发现与票据兑换 | [`auth::AuthClient`]：固定 HTTPS 网关会话、票据兑换；Windows 经 [`auth::browser_handoff`] 编排 | 不含入口发现与浏览器回调；Darwin 未接线 | Darwin 复用该编排并重跑当地真实业务流 |
//! | 资源获取（`clientResource` 请求） | [`auth::AuthClient::client_resource`] 返回有界正文；Windows 产品入口传入资源请求体并交给 `from_json` | 资源策略由宿主提供；Darwin 未接线 | Darwin 复用该路径并重跑当地真实业务流 |
//! | 节点握手与会话认证 | [`transport::l3::handshake`] 与 [`transport::l3::flow_auth`] | 原生协议已实现，真实节点持续流量未验收 | 宿主注入已验证 TLS 流与认证材料 |
//! | L3 数据面（收发循环、心跳、连接映射） | 生产多流循环为 [`transport::l3::packet_session::run_packets`]，Windows 经 `AtrustPacketTunnel` 接入；[`transport::l3::udp_probe`] 仅单包诊断 | 自动重连未实现，真实节点持续流量未验收 | Darwin 接线后重跑当地真实业务流 |
//! | TCP 资源通道 | 无 | T08 范围，且是否必需由 T01 资源实测决定 | T08 新增 `transport::tcp/`；资源侧约束已由目录保留 |
//! | 网卡、路由、DNS、出口决策 | 无 | 属宿主（T07/T10），本 crate 不涉足 | 宿主在活动连接入口调用本 crate 的数据接口 |
//! | L3 有界背压 | 会话队列有显式 [`session::QueueBudget`]（条数 + 字节）；[`transport::l3::connection`] 读/写两方向各复用同一预算，写侧含在途帧 | 预算取值未按真实节点持续流量复核 | 真实持续流量验收后复核预算取值 |
//! | 帧与队列里的 token/载荷 | 裸、可 `Clone` 的 `Vec<u8>`（[`transport::l3::codec::DataFrame`]、[`session::PendingUnit`]） | **不进入 [`error::Secret`]、不零化**：协议重试与数据面重发需要克隆这些字节；`Debug` 只打印长度 | T04 的认证材料与 T06 的会话材料**必须**走 [`error::Secret`]；需要零化帧内字节时先改数据流，不新增半成品包装 |
//!
//! # 样本来源规则
//!
//! `tests/fixtures/` 下的协议样本是**按上游 8de443f 的结构构造的合成样本**，不是网络抓取。
//! `node-observed-public.cer` 是例外：它是实际节点的公共证书，来源与指纹见
//! `docs/research/atrust/t06-node-entry-tls-evidence.md`，不含私钥或登录材料。
//! 合成样本不伪称抓包：资源样本在 JSON 内自带 `_fixture` 说明块，
//! 帧样本用 `#` 注释头写明来源、协议阶段、预期解释与脱敏方法。真实 Cookie、ticket、登录
//! 结果、密码、证书私钥、连接 token 一律不得进入本目录。
//!
//! # 上游分歧（实现时显式选择，不照抄）
//!
//! 1. 证书验证：上游在认证会话里关闭校验，本库路径一律保持验证开启（计划 T03 验收）。
//! 2. 非 `L3VPN` 资源：上游只写 debug 日志后跳过；本库记录 [`resource::SkippedEntry`]。
//! 3. IPv6：上游一律跳过；本库同样不声明校园 IPv6 支持，但把跳过原因显式记录。
//! 4. 数据接口形态：上游把 L3 暴露成 `io.ReadWriteCloser` 裸流；本库只提供带长度边界的帧与包切分。
//! 5. 未匹配语义：上游 `matchL3IPResource` 对 TCP 还要求 `EnableTCPPrefL3`；本库据此把
//!    “命中 L3 / 需要 TCP 应用通道 / 不属于任何资源”分开表达，**不**把未匹配表述为未授权。

pub mod auth;
pub mod error;
pub mod resource;
pub mod session;
pub mod transport;

pub use error::{
    Error, L3CodecError, REDACTED, ResourceError, Result, Secret, SessionError, UnderlayError,
    UnderlayErrorKind, redacted_sized,
};
pub use resource::{
    AddrPretend, Dispatch, DispatchReason, DispatchRef, DomainPattern, FlowProtocol, NodeAddress,
    NodeClass, NodeGroup, PortRange, ResourceApp, ResourceCatalog, ResourceEntry, ResourceHandle,
    ResourceHost, ResourceParseOptions, ResourceProtocol, ResourceRef, SkipReason, SkippedEntry,
    Target,
};
pub use session::{
    CloseOutcome, PendingUnit, QueueBudget, Session, SessionFactory, SessionId, TaggedResult,
};
pub use transport::l3::codec::{DataFrame, L3Decoder, L3Frame};
pub use transport::{ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay};

