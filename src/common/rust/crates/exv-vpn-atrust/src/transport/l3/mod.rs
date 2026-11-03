//! L3 隧道传输子层。
//!
//! 当前包含线格式编解码（[`codec`]）、初始 IPv4 交互（[`handshake`]）、每流授权消息
//! （[`flow_auth`]）、持续帧连接（[`connection`]）与生产多流循环（[`packet_session`]）。
//! Windows 生产入口已接入（`atrust_prepare` → `atrust_tunnel` → [`packet_session::run_packets`]），
//! Darwin 未接线；诊断路径已通过学校真实节点的认证、VIP/每流授权和一次 DNS UDP 往返。
//! 真实节点持续流量与自动重连仍未验收：
//!
//! | 能力 | 活动路径 | 未接线原因 | 重新接线条件 |
//! |---|---|---|---|
//! | 节点选择与持续隧道握手（上游 `authTunnel`、`getBestNodes`） | 宿主在授权目录内选择节点并建连，本模块 [`handshake`] 完成初始 IPv4 申请，[`packet_session`] 维持持续循环 | 自动重连未实现；Darwin 未接线 | 宿主注入已验证 TLS 流并重跑真实业务流 |
//! | 会话认证与连接映射（上游 `conntrackMgr`） | [`flow_auth`] 提供请求签名及响应解析，连接映射与调度由 [`packet_engine`] 与 [`packet_session`] 承担 | 真实节点持续流量未验收 | 宿主注入已验证流并重跑真实业务流 |
//! | 心跳与重连 | [`connection`] 已管理心跳，生产循环与诊断入口均已使用 | 连续心跳仅有本地测试；自动重连未实现 | 真实持续会话及宿主重连验收 |
//! | 收发数据包（`ReadPacket`/`WritePacket`） | [`packet_session::run_packets`] 为生产循环（Windows 经 `AtrustPacketTunnel` 接入）；[`udp_probe`] 仅单包诊断 | 真实节点持续流量未验收 | 真实持续流量验收（含跨帧包边界） |
//! | 分配 VIP 的应用路径 | 无 | 宿主网卡地址更新属宿主 | 宿主在 VIP 变化时更新自有网卡地址 |
//! | 帧级/连接级有界背压（读侧暂停、写侧水位） | [`connection`] 两方向各复用 [`crate::session::QueueBudget`]，写侧含在途帧 | 预算取值未按真实节点持续流量复核 | 真实持续流量验收后复核预算取值 |
//!
//! 已固定的边界语义（供 T06 依赖，不重复设计）：一包一消息（编码侧 `count` 必须为 1；解码侧
//! 对 `count > 1` 的前向兼容容忍**未经验证**，见 [`codec`] 模块文档）、长度先校验后分配、
//! 未知命令报错、关闭幂等由 [`crate::session`] 承担、`Debug` 不打印 payload 与 token、帧与
//! 队列里的 token/载荷仍是裸字节（不进 [`crate::error::Secret`]、不零化）。

pub mod codec;
pub mod connection;
pub mod packet_engine;
pub mod packet_session;
mod packet_ipv4;
pub mod handshake;
pub mod flow_auth;
pub mod udp_probe;

pub use codec::{
    AUTH_STATUS_IMMEDIATE_RETRY, AUTH_STATUS_RETRY_MAX, AUTH_STATUS_RETRY_MIN, CMD_AUTH_REQUEST,
    CMD_AUTH_RESPONSE, CMD_DATA_REQUEST, CMD_DATA_RESPONSE, CMD_HEARTBEAT_REQUEST,
    CMD_HEARTBEAT_RESPONSE, CMD_SECOND_VIP_RESPONSE, DataFrame, L3_MAX_PACKETS_PER_FRAME,
    L3_MAX_PAYLOAD, L3_MAX_TOKEN_LEN, L3_VERSION, L3Decoder, L3Frame,
};

