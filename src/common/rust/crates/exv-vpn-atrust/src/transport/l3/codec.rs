//! aTrust L3 隧道帧编解码。
//!
//! # 线格式（对照上游 `client/atrust/l3tunnelconn.go@8de443f` 的读写路径重建）
//!
//! 每帧固定以 `version(0x05) cmd(1 byte)` 开头，随后按命令不同：
//!
//! | 方向 | cmd | 布局（字节） |
//! |---|---|---|
//! | 上行 | `0x13` 认证请求 | `05 13 len_be16 payload` |
//! | 下行 | `0x93` 认证响应 | `05 93 status len_be16 payload` |
//! | 上行 | `0x14` 数据请求 | `05 14 token_len token rsv:2 count {len_be16 packet}*count` |
//! | 下行 | `0x94` 数据响应 | `05 94 len_be16 IPv4 包流` |
//! | 上行 | `0x15` 心跳请求 | `05 15 00 00` |
//! | 下行 | `0x95` 心跳响应 | `05 95 len_be16 payload` |
//! | 下行 | `0x96` 二次 VIP 响应 | `05 96 status len_be16 payload` |
//!
//! 要点：
//!
//! - **一包一消息**（设计 §3.1 的写出契约）：EXV 写出的数据请求每帧**恰好**带一个 IP 包；
//!   编码侧对 `packets.len() != 1` 直接返回 [`L3CodecError::PacketCountNotOne`]，不产出多包帧。
//! - **解码侧容忍 `count > 1` 只是前向兼容**：不把多包帧拆成多条消息，也不会因此接受多包
//!   编码。这个容忍**没有上游依据**——上游 `readFrame` 把入站 `0x14` 当致命协议错误
//!   （`unexpected L3 tunnel command`），`parseDataPayload` 只被上游自测调用；服务端是否真的
//!   下发多包帧**未经验证**，T06 必须用真实样本复核，复核前不得把“服务端会合并”当既证事实。
//! - **定长校验顺序**：先读帧头（2 字节）→ 再读长度字段 → 再读载荷；长度字段超过上限时
//!   在等待/分配载荷之前就拒绝（[`L3CodecError::OversizeFrame`]）。
//! - **数据响应**的载荷是原始 IPv4 包流，包边界由 IP 头给出，服务端可能跨帧切开一个包；
//!   因此这里提供 [`split_ipv4_packets`] 返回“完整包 + 未完成尾部”，尾部由会话层保留
//!   （T06）。协议库不把 IP 包接口暴露成任意字节流（设计 §4.3）。
//! - **未知命令**、截断、超长帧都是错误，不静默跳过。
//! - IPv6 包不在本实现的数据面内：遇到即 [`L3CodecError::Ipv6Unsupported`]，不静默透传。
//!
//! 握手阶段（上游 `authTunnel` 的 `0x05 0xD0` 方法、初始 VIP 头）不属于本模块，见
//! [`crate::transport::l3`] 的接线状态表。

use std::fmt;

use crate::error::L3CodecError;

/// L3 线格式版本字节。
pub const L3_VERSION: u8 = 0x05;

/// 帧头长度：版本 + 命令。
pub const L3_HEADER_LEN: usize = 2;

/// be16 长度字段的字节数。
pub const L3_LENGTH_FIELD_LEN: usize = 2;

/// 单帧载荷的结构上限：长度字段是 be16，因此 65535 是协议硬上限。
pub const L3_MAX_PAYLOAD: usize = u16::MAX as usize;

/// 完整帧（帧头 + 长度字段 + 载荷）的结构上限。
pub const L3_MAX_FRAME_LEN: usize = L3_HEADER_LEN + L3_LENGTH_FIELD_LEN + L3_MAX_PAYLOAD;

/// connect token 的长度上限（上游写入前检查 `token > 0xFF` 即报错）。
pub const L3_MAX_TOKEN_LEN: usize = u8::MAX as usize;

/// 单帧包数的结构上限（`count` 是 1 字节）。
///
/// 这是**解码侧**的结构事实（入站多包帧能被解析出来的上限），不是写出契约：编码侧只接受
/// `count == 1`，见 [`L3CodecError::PacketCountNotOne`]。
pub const L3_MAX_PACKETS_PER_FRAME: usize = u8::MAX as usize;

/// 上行认证请求。
pub const CMD_AUTH_REQUEST: u8 = 0x13;
/// 下行认证响应。
pub const CMD_AUTH_RESPONSE: u8 = 0x93;
/// 上行数据请求。
pub const CMD_DATA_REQUEST: u8 = 0x14;
/// 下行数据响应。
pub const CMD_DATA_RESPONSE: u8 = 0x94;
/// 上行心跳请求。
pub const CMD_HEARTBEAT_REQUEST: u8 = 0x15;
/// 下行心跳响应。
pub const CMD_HEARTBEAT_RESPONSE: u8 = 0x95;
/// 下行二次 VIP 分配响应。
pub const CMD_SECOND_VIP_RESPONSE: u8 = 0x96;

/// 认证响应状态：立即重试。
pub const AUTH_STATUS_IMMEDIATE_RETRY: u8 = 0x84;
/// 认证响应可重试状态区间下限。
pub const AUTH_STATUS_RETRY_MIN: u8 = 0x85;
/// 认证响应可重试状态区间上限。
pub const AUTH_STATUS_RETRY_MAX: u8 = 0x87;

/// 该认证状态是否表示“立即重试”（上游 `authImmediateRetryStatus`）。
///
/// 只是线上状态区间的事实表达；重试节奏、次数与超时由会话层（T06）决定。
#[must_use]
pub fn is_auth_immediate_retry_status(status: u8) -> bool {
    status == AUTH_STATUS_IMMEDIATE_RETRY
}

/// 该认证状态是否落在可重试区间（上游 `authRetryStatusMin..=authRetryStatusMax`）。
#[must_use]
pub fn is_auth_retryable_status(status: u8) -> bool {
    (AUTH_STATUS_RETRY_MIN..=AUTH_STATUS_RETRY_MAX).contains(&status)
}

/// 一个已解码/待编码的 L3 帧。
///
/// `payload` 在各变体里的含义与线格式一致；本层不解析 JSON，也不解释语义。
#[derive(Clone, PartialEq, Eq)]
pub enum L3Frame {
    /// 上行 `0x13`：连接认证请求（载荷为上行的认证 JSON）。
    AuthRequest {
        /// 认证请求载荷。
        payload: Vec<u8>,
    },
    /// 下行 `0x93`：认证响应（状态 + JSON 载荷）。
    AuthResponse {
        /// 状态字节。
        status: u8,
        /// 响应载荷。
        payload: Vec<u8>,
    },
    /// 上行 `0x14`：数据请求（连接 token + 一个 IP 包；解码侧对多包帧只做前向兼容容忍）。
    DataRequest(DataFrame),
    /// 下行 `0x94`：数据响应（原始 IPv4 包流，用 [`split_ipv4_packets`] 切分）。
    DataResponse {
        /// 原始载荷字节。
        payload: Vec<u8>,
    },
    /// 上行 `0x15`：心跳请求，无载荷。
    HeartbeatRequest,
    /// 下行 `0x95`：心跳响应。
    HeartbeatResponse {
        /// 响应载荷。
        payload: Vec<u8>,
    },
    /// 下行 `0x96`：二次 VIP 分配响应。
    SecondVipResponse {
        /// 状态字节。
        status: u8,
        /// 响应载荷。
        payload: Vec<u8>,
    },
}

impl fmt::Debug for L3Frame {
    /// 不打印载荷字节：帧载荷可能承载用户流量，日志与错误里只保留长度。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthRequest { payload } => f
                .debug_struct("AuthRequest")
                .field("payload_len", &payload.len())
                .finish(),
            Self::AuthResponse { status, payload } => f
                .debug_struct("AuthResponse")
                .field("status", &format_args!("0x{status:02x}"))
                .field("payload_len", &payload.len())
                .finish(),
            Self::DataRequest(data) => f.debug_tuple("DataRequest").field(data).finish(),
            Self::DataResponse { payload } => f
                .debug_struct("DataResponse")
                .field("payload_len", &payload.len())
                .finish(),
            Self::HeartbeatRequest => f.write_str("HeartbeatRequest"),
            Self::HeartbeatResponse { payload } => f
                .debug_struct("HeartbeatResponse")
                .field("payload_len", &payload.len())
                .finish(),
            Self::SecondVipResponse { status, payload } => f
                .debug_struct("SecondVipResponse")
                .field("status", &format_args!("0x{status:02x}"))
                .field("payload_len", &payload.len())
                .finish(),
        }
    }
}

/// 数据请求帧的内容：每流 connect token + 该帧携带的 IP 包。
///
/// `token` 是服务端按流下发的敏感材料，`Debug` 只输出长度；`packets` 是用户流量载荷，
/// `Debug` 同样只输出长度。需要原文时用字段访问，调用点负责不把它写进日志。
///
/// 两个字段都是**裸、可 `Clone` 的字节**：不进入 [`crate::error::Secret`]、不做零化，因为
/// 数据面重连重发需要克隆（边界见 [`crate::error`] 模块文档）。`packets` 在编码时必须是
/// 恰好一个包（见 [`encode_frame`]）。
#[derive(Clone, PartialEq, Eq)]
pub struct DataFrame {
    /// 每流 connect token（敏感，但仍按裸字节传递）。
    pub token: Vec<u8>,
    /// 本帧携带的 IP 包；编码只接受长度为 1 的「一包一消息」形态。
    pub packets: Vec<Vec<u8>>,
}

impl fmt::Debug for DataFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let packet_lens: Vec<usize> = self.packets.iter().map(Vec::len).collect();
        f.debug_struct("DataFrame")
            .field("token_len", &self.token.len())
            .field("packet_lens", &packet_lens)
            .finish()
    }
}

/// 编码任意帧。
///
/// 同时覆盖上行与下行：宿主的活动路径只编码上行帧，下行帧的编码用于差分测试与样本构造。
///
/// # Errors
///
/// 载荷/token 超过线格式上限时返回 [`L3CodecError`]。
pub fn encode_frame(frame: &L3Frame) -> Result<Vec<u8>, L3CodecError> {
    match frame {
        L3Frame::AuthRequest { payload } => encode_length_prefixed(CMD_AUTH_REQUEST, payload),
        L3Frame::AuthResponse { status, payload } => {
            encode_status_length_prefixed(CMD_AUTH_RESPONSE, *status, payload)
        }
        L3Frame::DataRequest(data) => encode_data_request(data),
        L3Frame::DataResponse { payload } => encode_length_prefixed(CMD_DATA_RESPONSE, payload),
        L3Frame::HeartbeatRequest => Ok(vec![L3_VERSION, CMD_HEARTBEAT_REQUEST, 0x00, 0x00]),
        L3Frame::HeartbeatResponse { payload } => {
            encode_length_prefixed(CMD_HEARTBEAT_RESPONSE, payload)
        }
        L3Frame::SecondVipResponse { status, payload } => {
            encode_status_length_prefixed(CMD_SECOND_VIP_RESPONSE, *status, payload)
        }
    }
}

fn check_be16(len: usize) -> Result<(), L3CodecError> {
    if len > L3_MAX_PAYLOAD {
        return Err(L3CodecError::OversizeFrame {
            declared: len,
            limit: L3_MAX_PAYLOAD,
        });
    }
    Ok(())
}

/// `len` 转 be16 长度字段。前置条件：调用方已用 [`check_be16`] 校验不超过 `u16::MAX`。
fn be16_len(len: usize) -> [u8; 2] {
    #[allow(clippy::cast_possible_truncation)]
    let narrowed = len as u16;
    narrowed.to_be_bytes()
}

/// `len` 转 1 字节计数字段。前置条件：调用方已校验不超过 `u8::MAX`。
fn u8_len(len: usize) -> u8 {
    #[allow(clippy::cast_possible_truncation)]
    let narrowed = len as u8;
    narrowed
}

fn encode_length_prefixed(command: u8, payload: &[u8]) -> Result<Vec<u8>, L3CodecError> {
    check_be16(payload.len())?;
    let mut out = Vec::with_capacity(L3_HEADER_LEN + L3_LENGTH_FIELD_LEN + payload.len());
    out.push(L3_VERSION);
    out.push(command);
    out.extend_from_slice(&be16_len(payload.len()));
    out.extend_from_slice(payload);
    Ok(out)
}

fn encode_status_length_prefixed(
    command: u8,
    status: u8,
    payload: &[u8],
) -> Result<Vec<u8>, L3CodecError> {
    check_be16(payload.len())?;
    let mut out = Vec::with_capacity(1 + L3_HEADER_LEN + L3_LENGTH_FIELD_LEN + payload.len());
    out.push(L3_VERSION);
    out.push(command);
    out.push(status);
    out.extend_from_slice(&be16_len(payload.len()));
    out.extend_from_slice(payload);
    Ok(out)
}

/// 编码上行数据请求帧。
///
/// 「一包一消息」是写出契约：`data.packets` 的长度不是 1 时返回
/// [`L3CodecError::PacketCountNotOne`]。线格式的 `count` 字段能表达 1..=255，但本实现不借此
/// 产出多包帧——服务端是否接受多包帧没有任何依据（见模块文档）。
fn encode_data_request(data: &DataFrame) -> Result<Vec<u8>, L3CodecError> {
    if data.token.len() > L3_MAX_TOKEN_LEN {
        return Err(L3CodecError::TokenTooLong {
            len: data.token.len(),
            limit: L3_MAX_TOKEN_LEN,
        });
    }
    if data.packets.len() != 1 {
        return Err(L3CodecError::PacketCountNotOne {
            len: data.packets.len(),
        });
    }

    let mut body = Vec::new();
    body.push(u8_len(data.token.len()));
    body.extend_from_slice(&data.token);
    // 保留字段：上游固定写 0x00 0x00。
    body.push(0x00);
    body.push(0x00);
    body.push(u8_len(data.packets.len()));
    for packet in &data.packets {
        if packet.is_empty() {
            return Err(L3CodecError::MalformedPacket {
                reason: "empty packet payload",
            });
        }
        check_be16(packet.len())?;
        body.extend_from_slice(&be16_len(packet.len()));
        body.extend_from_slice(packet);
    }
    check_be16(body.len())?;

    let mut out = Vec::with_capacity(L3_HEADER_LEN + body.len());
    out.push(L3_VERSION);
    out.push(CMD_DATA_REQUEST);
    out.extend_from_slice(&body);
    Ok(out)
}

/// 流式解码器：喂入字节流，按顺序取出完整帧。
///
/// `decode` 需要更多字节时返回 `Ok(None)`；遇到畸形输入返回错误且**不消费**缓冲区，
/// 便于调用点记录现场后关闭连接。
#[derive(Debug)]
pub struct L3Decoder {
    buffer: Vec<u8>,
    max_payload: usize,
}

impl Default for L3Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl L3Decoder {
    /// 使用结构上限（65535）构造。
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            max_payload: L3_MAX_PAYLOAD,
        }
    }

    /// 使用更紧的策略上限构造；超过结构上限的值会被收敛到结构上限。
    ///
    /// 上限用于拒绝超长帧（[`L3CodecError::OversizeFrame`]），在等待与分配载荷之前生效。
    #[must_use]
    pub fn with_max_payload(max_payload: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_payload: max_payload.clamp(1, L3_MAX_PAYLOAD),
        }
    }

    /// 追加一段到达的字节。
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// 当前残留字节数。
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// 当前残留字节（例如数据响应里未完成的 IPv4 包尾部）。
    #[must_use]
    pub fn remaining(&self) -> &[u8] {
        &self.buffer
    }

    /// 取出一帧。
    ///
    /// # Errors
    ///
    /// 版本不符、未知命令、超长帧、定长帧带载荷等畸形输入返回 [`L3CodecError`]。
    pub fn decode(&mut self) -> Result<Option<L3Frame>, L3CodecError> {
        if self.buffer.len() < L3_HEADER_LEN {
            return Ok(None);
        }
        let version = self.buffer[0];
        if version != L3_VERSION {
            return Err(L3CodecError::UnsupportedVersion {
                actual: version,
                expected: L3_VERSION,
            });
        }
        let command = self.buffer[1];
        match command {
            CMD_AUTH_REQUEST | CMD_HEARTBEAT_RESPONSE | CMD_DATA_RESPONSE => {
                let Some(total) = self.length_prefixed_total(L3_HEADER_LEN)? else {
                    return Ok(None);
                };
                let payload = self.buffer[L3_HEADER_LEN + L3_LENGTH_FIELD_LEN..total].to_vec();
                self.buffer.drain(..total);
                let frame = match command {
                    CMD_AUTH_REQUEST => L3Frame::AuthRequest { payload },
                    CMD_HEARTBEAT_RESPONSE => L3Frame::HeartbeatResponse { payload },
                    _ => L3Frame::DataResponse { payload },
                };
                Ok(Some(frame))
            }
            CMD_AUTH_RESPONSE | CMD_SECOND_VIP_RESPONSE => {
                let status_offset = L3_HEADER_LEN;
                let Some(total) = self.length_prefixed_total(status_offset + 1)? else {
                    return Ok(None);
                };
                let status = self.buffer[status_offset];
                let payload =
                    self.buffer[status_offset + 1 + L3_LENGTH_FIELD_LEN..total].to_vec();
                self.buffer.drain(..total);
                let frame = match command {
                    CMD_AUTH_RESPONSE => L3Frame::AuthResponse { status, payload },
                    _ => L3Frame::SecondVipResponse { status, payload },
                };
                Ok(Some(frame))
            }
            CMD_HEARTBEAT_REQUEST => {
                if self.buffer.len() < L3_HEADER_LEN + L3_LENGTH_FIELD_LEN {
                    return Ok(None);
                }
                let declared = be16(&self.buffer[L3_HEADER_LEN..]);
                if declared != 0 {
                    return Err(L3CodecError::UnexpectedPayloadLength { command, declared });
                }
                self.buffer.drain(..L3_HEADER_LEN + L3_LENGTH_FIELD_LEN);
                Ok(Some(L3Frame::HeartbeatRequest))
            }
            CMD_DATA_REQUEST => {
                let Some(total) = data_request_total(&self.buffer, self.max_payload)? else {
                    return Ok(None);
                };
                let token_len = self.buffer[2] as usize;
                let token = self.buffer[3..3 + token_len].to_vec();
                let mut idx = 3 + token_len + 2;
                let count = self.buffer[idx] as usize;
                idx += 1;
                let mut packets = Vec::with_capacity(count);
                for _ in 0..count {
                    let packet_len = be16(&self.buffer[idx..]);
                    idx += 2;
                    packets.push(self.buffer[idx..idx + packet_len].to_vec());
                    idx += packet_len;
                }
                self.buffer.drain(..total);
                Ok(Some(L3Frame::DataRequest(DataFrame { token, packets })))
            }
            other => Err(L3CodecError::UnknownCommand { command: other }),
        }
    }

    /// 读取“长度字段位于 `length_offset`、载荷紧随其后”的帧总长。
    fn length_prefixed_total(
        &self,
        length_offset: usize,
    ) -> Result<Option<usize>, L3CodecError> {
        if self.buffer.len() < length_offset + L3_LENGTH_FIELD_LEN {
            return Ok(None);
        }
        let declared = be16(&self.buffer[length_offset..]);
        if declared > self.max_payload {
            return Err(L3CodecError::OversizeFrame {
                declared,
                limit: self.max_payload,
            });
        }
        let total = length_offset + L3_LENGTH_FIELD_LEN + declared;
        if self.buffer.len() < total {
            return Ok(None);
        }
        Ok(Some(total))
    }

    /// 流结束：缓冲区为空表示干净结束，否则是截断。
    ///
    /// # Errors
    ///
    /// 缓冲区残留字节时返回 [`L3CodecError::IncompleteAtEof`]。
    pub fn finish(self) -> Result<(), L3CodecError> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(L3CodecError::IncompleteAtEof {
                buffered: self.buffer.len(),
            })
        }
    }
}

fn be16(bytes: &[u8]) -> usize {
    ((bytes[0] as usize) << 8) | bytes[1] as usize
}

/// 计算数据请求帧的总长；字节不足时返回 `Ok(None)`。
///
/// 包长表里声明的长度一旦超过策略上限就立即报错，不等待剩余字节到齐。
fn data_request_total(buffer: &[u8], max_payload: usize) -> Result<Option<usize>, L3CodecError> {
    if buffer.len() < 3 {
        return Ok(None);
    }
    let token_len = buffer[2] as usize;
    let mut idx = 3 + token_len;
    if buffer.len() < idx + 3 {
        return Ok(None);
    }
    // 跳过保留字段，读包数。
    idx += 2;
    let count = buffer[idx] as usize;
    idx += 1;
    for _ in 0..count {
        if buffer.len() < idx + L3_LENGTH_FIELD_LEN {
            return Ok(None);
        }
        let packet_len = be16(&buffer[idx..]);
        idx += L3_LENGTH_FIELD_LEN;
        let frame_len = idx + packet_len;
        if frame_len > L3_HEADER_LEN + max_payload {
            return Err(L3CodecError::OversizeFrame {
                declared: frame_len,
                limit: L3_HEADER_LEN + max_payload,
            });
        }
        if buffer.len() < frame_len {
            return Ok(None);
        }
        idx = frame_len;
    }
    Ok(Some(idx))
}

/// 从数据响应载荷里切出完整的 IPv4 包。
///
/// 返回 `(完整包, 未完成尾部)`：尾部不足一个完整包时原样返回，由会话层保留到下一帧
/// （上游 `splitIncomingIPPackets` 的语义）。服务端可能跨帧切开一个包，因此不能假定
/// 一帧等于若干完整包。
///
/// # Errors
///
/// 总长小于 IPv4 最小头长20字节、未知 IP 版本或 IPv6 返回 [`L3CodecError`]。
/// 已知 IPv4 单元按声明总长拆分；IHL/校验和等完整包验证由调用方负责。
/// 不扫描后续字节或以外层帧边界重新同步 IP 流。
pub fn split_ipv4_packets(stream: &[u8]) -> Result<(Vec<&[u8]>, &[u8]), L3CodecError> {
    let mut packets = Vec::new();
    let mut rest = stream;
    loop {
        if rest.is_empty() {
            return Ok((packets, rest));
        }
        match rest[0] >> 4 {
            4 => {}
            6 => return Err(L3CodecError::Ipv6Unsupported),
            _ => {
                return Err(L3CodecError::MalformedPacket {
                    reason: "ip version is neither 4 nor 6",
                });
            }
        }
        if rest.len() < 4 {
            return Ok((packets, rest));
        }
        let total_len = be16(&rest[2..]);
        if total_len < 20 {
            return Err(L3CodecError::MalformedPacket {
                reason: "ipv4 total length below 20 bytes",
            });
        }
        if rest.len() < total_len {
            return Ok((packets, rest));
        }
        let (packet, tail) = rest.split_at(total_len);
        packets.push(packet);
        rest = tail;
    }
}

