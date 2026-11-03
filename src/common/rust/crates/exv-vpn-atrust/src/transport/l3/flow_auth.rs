//! 每流授权 JSON 与签名。按固定上游 8de443f 重建，2026-09-22 已通过学校节点的 DNS UDP 授权。
//! 不发包、不改路由；终端环境由调用方提供，不生成上游的默认可信声明。
use crate::error::Secret;
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::{fmt, io::Write, net::Ipv4Addr};
use zeroize::Zeroizing;

/// 对应进入隧道的原始 IPv4 五元组；ICMP 端口由调用方按协议填入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Flow {
    pub protocol: u8,
    pub source: Ipv4Addr,
    pub source_port: u16,
    pub destination: Ipv4Addr,
    pub destination_port: u16,
}

/// 调用方已经完成资源匹配后的授权输入；本类型不决定用户白名单或终端可信状态。
pub struct FlowAuthRequest<'a> {
    pub sid: &'a Secret,
    pub app_id: &'a str,
    pub process_hash: &'a str,
    pub app_token: Option<&'a Secret>,
    pub device_id: &'a str,
    pub connection_id: &'a str,
    pub lang: &'a str,
    pub environment: Option<&'a serde_json::Value>,
    pub conntrack_hash: u64,
    pub flow: Ipv4Flow,
    pub domain: Option<&'a str>,
}
impl fmt::Debug for FlowAuthRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FlowAuthRequest(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FlowAuthError {
    #[error("empty flow authentication credential")]
    EmptyCredential,
    #[error("invalid hexadecimal flow signing key")]
    InvalidSigningKey,
    #[error("flow authentication payload exceeds wire limit")]
    PayloadTooLarge,
    #[error("invalid flow authentication JSON")]
    InvalidJson,
    #[error(
        "invalid flow response shape: code {code}, data {data}, hash {hash}, token {token}, ip {ip}"
    )]
    InvalidResponseShape {
        code: &'static str,
        data: &'static str,
        hash: &'static str,
        token: &'static str,
        ip: &'static str,
    },
    #[error("flow authentication response does not match pending flow")]
    FlowMismatch,
    #[error("flow authentication rejected with status {0}")]
    Status(u8),
    #[error("flow authentication rejected with status {status} and code {code}")]
    StatusWithCode { status: u8, code: i64 },
    #[error("flow authentication rejected with code {0}")]
    Rejected(i64),
    #[error("invalid flow connect token length")]
    InvalidToken,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIp {
    atype: u16,
    protocol: u8,
    dest_addr: Ipv4Addr,
    dest_port: u16,
    src_addr: Ipv4Addr,
    src_port: u16,
}

/// 仅表达本次响应；调用方负责重试次数、排队和会话关闭，不自动执行重试。
#[derive(Debug)]
pub enum FlowAuthorization {
    Granted(Secret),
    RetryAfter(std::time::Duration),
}

#[derive(Deserialize)]
struct Response {
    code: i64,
    data: ResponseData,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponseData {
    conntrack_hash: Option<u64>,
    connect_token: Option<serde_json::Value>,
    ip: Option<WireIp>,
}

/// 解析已由 L3 codec 分离的 0x93 正文。缺失/零 ID 时才按完整 IPv4 五元组回退匹配。
/// 非零但不同的 ID 不回退，避免把另一个等待中连接的 token 交给当前连接。
///
/// # Errors
/// 超长或无效 JSON、连接不匹配、拒绝状态/业务码或无效 token 返回对应错误。
pub fn parse_response(
    status: u8,
    payload: &[u8],
    expected_id: u64,
    flow: Ipv4Flow,
) -> Result<FlowAuthorization, FlowAuthError> {
    if payload.len() > 65535 {
        return Err(FlowAuthError::PayloadTooLarge);
    }
    let response: Response =
        serde_json::from_slice(payload).map_err(|_| response_shape_error(payload))?;
    let matches = match response.data.conntrack_hash.filter(|id| *id != 0) {
        Some(id) => id == expected_id,
        None => response.data.ip.is_some_and(|ip| {
            ip.atype == 0x0800
                && ip.protocol == flow.protocol
                && ip.dest_addr == flow.destination
                && ip.dest_port == flow.destination_port
                && ip.src_addr == flow.source
                && ip.src_port == flow.source_port
        }),
    };
    if !matches {
        return Err(FlowAuthError::FlowMismatch);
    }
    if status == 0x84 {
        return Ok(FlowAuthorization::RetryAfter(std::time::Duration::ZERO));
    }
    if (0x85..=0x87).contains(&status) {
        return Ok(FlowAuthorization::RetryAfter(
            std::time::Duration::from_secs(10),
        ));
    }
    if status != 0 {
        if response.code != 0 {
            return Err(FlowAuthError::StatusWithCode {
                status,
                code: response.code,
            });
        }
        return Err(FlowAuthError::Status(status));
    }
    // 上游只检查 status；EXV 不把显式非零业务码当作成功授权。
    if response.code != 0 {
        return Err(FlowAuthError::Rejected(response.code));
    }
    let token = match response.data.connect_token {
        Some(serde_json::Value::String(value)) => Secret::new(value),
        _ => return Err(FlowAuthError::InvalidToken),
    };
    if token.is_empty() || token.len() > 255 {
        return Err(FlowAuthError::InvalidToken);
    }
    Ok(FlowAuthorization::Granted(token))
}

// 只投影固定字段的类型，不包含原始响应、token 或服务端 message。
fn response_shape_error(payload: &[u8]) -> FlowAuthError {
    fn kind(value: Option<&serde_json::Value>) -> &'static str {
        match value {
            None => "missing",
            Some(serde_json::Value::Null) => "null",
            Some(serde_json::Value::Bool(_)) => "bool",
            Some(serde_json::Value::Number(_)) => "number",
            Some(serde_json::Value::String(_)) => "string",
            Some(serde_json::Value::Array(_)) => "array",
            Some(serde_json::Value::Object(_)) => "object",
        }
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return FlowAuthError::InvalidJson;
    };
    let data = value.get("data");
    FlowAuthError::InvalidResponseShape {
        code: kind(value.get("code")),
        data: kind(data),
        hash: kind(data.and_then(|v| v.get("conntrackHash"))),
        token: kind(data.and_then(|v| v.get("connectToken"))),
        ip: kind(data.and_then(|v| v.get("ip"))),
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireRequest<'a> {
    sid: &'a str,
    app_id: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    proc_hash: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    app_token: &'a str,
    url: String,
    device_id: &'a str,
    connection_id: &'a str,
    rc_applied_info: u8,
    lang: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<&'a serde_json::Value>,
    conntrack_hash: u64,
    ip: WireIp,
    #[serde(skip_serializing_if = "str::is_empty")]
    domain: &'a str,
    x_request_sig: &'a str,
}

struct LimitedJson(Zeroizing<Vec<u8>>);
impl Write for LimitedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > 65535 - self.0.len() {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(request: &WireRequest<'_>) -> Result<Zeroizing<Vec<u8>>, FlowAuthError> {
    let mut raw = LimitedJson(Zeroizing::new(Vec::new()));
    serde_json::to_writer(&mut raw, request).map_err(|_| FlowAuthError::PayloadTooLarge)?;
    let text = std::str::from_utf8(&raw.0).map_err(|_| FlowAuthError::InvalidJson)?;
    // 与 Go encoding/json 默认 HTML 转义一致，签名与最终发送内容均使用同一编码。
    let mut escaped = LimitedJson(Zeroizing::new(Vec::new()));
    for c in text.chars() {
        let mut utf8 = [0u8; 4];
        let piece = match c {
            '<' => "\\u003c",
            '>' => "\\u003e",
            '&' => "\\u0026",
            '\u{2028}' => "\\u2028",
            '\u{2029}' => "\\u2029",
            _ => c.encode_utf8(&mut utf8),
        };
        escaped
            .write_all(piece.as_bytes())
            .map_err(|_| FlowAuthError::PayloadTooLarge)?;
    }
    Ok(escaped.0)
}

/// HMAC-SHA256 对 xRequestSig 为空的 JSON 签名，返回大写十六进制签名后的 JSON。
/// `sign_key` 是上游 `SignKey` 十六进制文本，先解码为字节再签名。返回值含 SID，不能记录。
///
/// # Errors
/// 空凭据、无效十六进制密钥或超过线格式长度上限的 JSON 返回对应错误。
pub fn sign_request(
    request: &FlowAuthRequest<'_>,
    sign_key: &Secret,
) -> Result<Zeroizing<Vec<u8>>, FlowAuthError> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    if request.sid.is_empty() || sign_key.is_empty() {
        return Err(FlowAuthError::EmptyCredential);
    }
    let proto = match request.flow.protocol {
        6 => "tcp",
        17 => "udp",
        1 => "icmp",
        _ => "ip",
    };
    let mut wire = WireRequest {
        sid: request.sid.expose(),
        app_id: request.app_id,
        proc_hash: request.process_hash,
        app_token: request.app_token.map_or("", Secret::expose),
        url: format!(
            "{proto}:{}:{}",
            request.flow.destination, request.flow.destination_port
        ),
        device_id: request.device_id,
        connection_id: request.connection_id,
        rc_applied_info: 0,
        lang: request.lang,
        env: request.environment,
        conntrack_hash: request.conntrack_hash,
        ip: WireIp {
            atype: 0x0800,
            protocol: request.flow.protocol,
            dest_addr: request.flow.destination,
            dest_port: request.flow.destination_port,
            src_addr: request.flow.source,
            src_port: request.flow.source_port,
        },
        domain: request.domain.unwrap_or(""),
        x_request_sig: "",
    };
    let unsigned = encode(&wire)?;
    let text = sign_key.expose().as_bytes();
    if !text.len().is_multiple_of(2) {
        return Err(FlowAuthError::InvalidSigningKey);
    }
    let mut decoded = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for pair in text.chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        decoded.push(hi * 16 + lo);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, &decoded);
    let digest = hmac::sign(&key, &unsigned);
    let mut signature = Zeroizing::new(String::with_capacity(64));
    for b in digest.as_ref() {
        signature.push(HEX[(b >> 4) as usize] as char);
        signature.push(HEX[(b & 15) as usize] as char);
    }
    wire.x_request_sig = &signature;
    encode(&wire)
}

fn hex_nibble(byte: u8) -> Result<u8, FlowAuthError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(FlowAuthError::InvalidSigningKey),
    }
}
