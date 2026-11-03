//! `secret_payload` 的引擎侧解码分派（M2 接线；M1 冻结的 wire 裁决消费点）。
//!
//! 按 wire 头 `MAGIC + VERSION` 分派两版信封：
//! - VERSION=1 → [`DarwinEngineConnectV1::decode`]（**Cisco 路径字节不变**——
//!   既有 V1 wire 的接受/拒绝行为逐字保持，字节锁定测试守卫在 darwin-ipc）；
//! - VERSION=3 → Cisco V3（内嵌完整旧 V1 + 显式域列表）。
//! - VERSION=2（含 Cisco/aTrust 两判别位）→ [`DarwinEngineConnectV2::decode`]；
//!   `V2::Cisco` 还原为 V1 实例（构造性逐字一致），`V2::Atrust` 取出
//!   [`DarwinAtrustConnect`]（BrowserSession wire + 身份材料，M2 认证编排输入）。
//!
//! 两个 decoder 都在**所有**返回路径清零输入 buffer；本分派不额外复制 wire 字节。
//! 未知 VERSION 沿用 V2 decoder 的 typed `UnsupportedVersion`（镜像 win32
//! `secret_payload` v1/v2 纪律：混合版本得到 typed 失败而非误解析）。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use exv_vpn_darwin_ipc::connect_envelope::{
    ConnectEnvelopeError, DarwinAtrustConnect, DarwinEngineConnectV1, DarwinEngineConnectV2, DarwinEngineConnectV3,
    EngineConnectPayload,
};

/// 与 darwin-ipc `connect_envelope` 同值的 wire MAGIC（`EXVC`）重声明（MAGIC 是
/// 私有常量；此处仅做头部分派判定，不产生第二份 codec）。
const WIRE_MAGIC: [u8; 4] = *b"EXVC";

/// wire 头长度：MAGIC(4) + VERSION(2)。
const WIRE_HEADER_LEN: usize = WIRE_MAGIC.len() + 2;

/// 分派后的一次性连接意图。
pub(crate) enum ConnectIntent {
    /// Cisco（V1 wire，或 V2::Cisco 载荷——内嵌 V1 body 逐字还原）。
    Cisco(DarwinEngineConnectV3),
    /// aTrust（V2 wire 的 aTrust 判别载荷）。
    Atrust(DarwinAtrustConnect),
}

/// 引擎侧统一解码入口：消费 `secret_payload` 并按 MAGIC+VERSION 分派。
///
/// # Errors
///
/// MAGIC 不符、版本未知、判别位非法或任一字段校验失败时返回
/// [`ConnectEnvelopeError`]（typed；输入已在 decoder 内全路径清零）。
pub(crate) fn decode_connect_intent(
    payload: Vec<u8>,
) -> Result<ConnectIntent, ConnectEnvelopeError> {
    // 只读头部分派：不消费、不复制 wire（所有权整体移交对应 decoder）。
    let version = payload
        .get(..WIRE_HEADER_LEN)
        .filter(|header| header[..WIRE_MAGIC.len()] == WIRE_MAGIC)
        .map(|header| u16::from_be_bytes([header[4], header[5]]));
    match version {
        Some(1) => DarwinEngineConnectV1::decode(payload).map(DarwinEngineConnectV3::legacy).map(ConnectIntent::Cisco),
        Some(3) => DarwinEngineConnectV3::decode(payload).map(ConnectIntent::Cisco),
        Some(_) => match DarwinEngineConnectV2::decode(payload)?.into_payload() {
            EngineConnectPayload::Cisco(envelope) => Ok(ConnectIntent::Cisco(DarwinEngineConnectV3::legacy(envelope))),
            EngineConnectPayload::Atrust(atrust) => Ok(ConnectIntent::Atrust(atrust)),
        },
        // 头部不足或 MAGIC 不符：交给 V1 decoder 给出统一的 typed InvalidEncoding
        // （其输入清零语义覆盖本路径）。
        None => DarwinEngineConnectV1::decode(payload).map(DarwinEngineConnectV3::legacy).map(ConnectIntent::Cisco),
    }
}
