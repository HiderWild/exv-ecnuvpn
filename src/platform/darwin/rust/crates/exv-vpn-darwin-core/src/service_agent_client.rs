//! Core 到已安装服务代理的固定动作帧客户端（`Status` 探测与 `EnsureEngine`
//! 修复阶梯；每会话 `StartEngine` 帧已随 E8 退役——oneshot 经提权弹窗直拉、
//! 服务形态走常驻固定端点）。

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

use crate::elevation::CoreCredentials;

/// 服务代理控制 socket 的固定路径（探测与启动共用；W3-4/P4 v1 起对
/// [`crate::service_status`] 可见）。
pub(crate) const SOCKET_PATH: &str = "/Library/Application Support/EXV/ServiceAgent/control.sock";
const REQUEST_LEN: usize = 16;
const RESPONSE_LEN: usize = 16;
const STATUS: u8 = 1;
const ENSURE_ENGINE: u8 = 6;
const DETAIL_CAPABILITY: u8 = 7;
const ENSURE_DETAILED: u8 = 8;
const DETAIL_MAX: usize = 2204;

/// 服务代理不可达、拒绝 Status 请求或应答帧不符合 V1。
///
/// `Unavailable`（socket 不可连 / IO 失败 / 超时）与 `Rejected`（得到应答但未接受，
/// 或帧校验失败）的区分是探测语义：前者说明 daemon 不在服务，后者说明 daemon 在
/// 但拒绝本 owner（V1 daemon 只服务 enrolled owner）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceAgentStatusError {
    Unavailable,
    Rejected,
}

/// EnsureEngine 原请求错误；仅连接缺席可进入已有提权启动步骤。
#[derive(Debug)]
pub(crate) enum EnsureEngineError {
    InvalidOwner,
    Io { step: &'static str, source: std::io::Error },
    InvalidResponse { outcome: u8, code: u32 },
    Rejected { outcome: u8, code: u32 },
    DetailedFailure { outcome: u8, code: u32, detail: String },
}

impl EnsureEngineError {
    pub(crate) fn daemon_absent(&self) -> bool {
        matches!(self, Self::Io { step: "connect", source }
            if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused))
    }
}

impl std::fmt::Display for EnsureEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOwner => f.write_str("EnsureEngine invalid root owner"),
            Self::Io { step, source } => write!(f, "EnsureEngine {step}: {source} (errno={:?})", source.raw_os_error()),
            Self::InvalidResponse { outcome, code } => write!(f, "EnsureEngine invalid response frame outcome={outcome} code={code}"),
            Self::Rejected { outcome, code } => write!(f, "EnsureEngine rejected outcome={outcome} code={code}"),
            Self::DetailedFailure { outcome, code, detail } => write!(f, "EnsureEngine response outcome={outcome} code={code}; {detail}"),
        }
    }
}

/// 调用方现有绝对期限和取消包围整个future；能力选择与实际请求均不重置预算。
pub(crate) async fn ensure_engine(credentials: CoreCredentials) -> Result<(), EnsureEngineError> {
    if credentials.uid() == 0 { return Err(EnsureEngineError::InvalidOwner); }
    ensure_engine_at(SOCKET_PATH, credentials.uid()).await
}
async fn ensure_engine_at(path: &str, uid: u32) -> Result<(), EnsureEngineError> {
    let mut capability = connect_ensure(path).await?;
    let detailed = select_detailed(&mut capability, uid).await?;
    drop(capability);
    let mut stream = connect_ensure(path).await?;
    request_ensure(&mut stream, uid, if detailed { ENSURE_DETAILED } else { ENSURE_ENGINE }).await
}
async fn connect_ensure(path: &str) -> Result<UnixStream, EnsureEngineError> {
    UnixStream::connect(path).await.map_err(|source| EnsureEngineError::Io { step: "connect", source })
}
async fn send_request(stream: &mut UnixStream, uid: u32, action: u8) -> Result<(), EnsureEngineError> {
    let mut request = [0_u8; REQUEST_LEN];
    request[..4].copy_from_slice(b"EXVA"); request[4] = 1; request[5] = action;
    request[8..12].copy_from_slice(&uid.to_be_bytes());
    stream.write_all(&request).await.map_err(|source| EnsureEngineError::Io { step: "write", source })?;
    stream.shutdown().await.map_err(|source| EnsureEngineError::Io { step: "shutdown", source })
}
fn validate_header(response: &[u8; RESPONSE_LEN], action: u8) -> Result<(u8, u32), EnsureEngineError> {
    let outcome = response[6];
    let code = u32::from_be_bytes(response[8..12].try_into().expect("fixed code"));
    if response[..4] != *b"EXVR" || response[4] != 1 || response[5] != action
        || response[7] != 0 || response[12..].iter().any(|byte| *byte != 0)
        || !matches!((outcome, code), (0, 0) | (1, 1..)) {
        return Err(EnsureEngineError::InvalidResponse { outcome, code });
    }
    Ok((outcome, code))
}
async fn require_response_eof(stream: &mut UnixStream) -> Result<(), EnsureEngineError> {
    let mut extra = [0];
    match stream.read(&mut extra).await {
        Ok(0) => Ok(()),
        Ok(_) => Err(EnsureEngineError::Io { step: "response EOF", source: std::io::Error::new(std::io::ErrorKind::InvalidData, "unexpected response bytes") }),
        Err(source) => Err(EnsureEngineError::Io { step: "response EOF", source }),
    }
}
fn legacy_capability_end(received: usize, result: &std::io::Result<usize>) -> bool {
    received == 0 && match result {
        Ok(0) => true,
        Err(error) => error.kind() == std::io::ErrorKind::ConnectionReset,
        _ => false,
    }
}
async fn select_detailed(stream: &mut UnixStream, uid: u32) -> Result<bool, EnsureEngineError> {
    send_request(stream, uid, DETAIL_CAPABILITY).await?;
    let mut response = [0; RESPONSE_LEN];
    let mut received = 0;
    while received < RESPONSE_LEN {
        let result = stream.read(&mut response[received..]).await;
        if legacy_capability_end(received, &result) { return Ok(false); }
        match result {
            Ok(0) => return Err(EnsureEngineError::Io { step: "capability read", source: std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "partial capability response") }),
            Ok(count) => received += count,
            Err(source) => return Err(EnsureEngineError::Io { step: "capability read", source }),
        }
    }
    let (outcome, code) = validate_header(&response, DETAIL_CAPABILITY)?;
    require_response_eof(stream).await?;
    if outcome == 0 { Ok(true) } else { Err(EnsureEngineError::Rejected { outcome, code }) }
}
async fn request_ensure(stream: &mut UnixStream, uid: u32, action: u8) -> Result<(), EnsureEngineError> {
    // 本次实际Ensure一经派发，所有读写、EOF或codec失败直接返回；没有回放分支。
    send_request(stream, uid, action).await?;
    let mut response = [0; RESPONSE_LEN];
    stream.read_exact(&mut response).await.map_err(|source| EnsureEngineError::Io { step: "read", source })?;
    let (outcome, code) = validate_header(&response, action)?;
    if action == ENSURE_DETAILED {
        let detail = read_detail(stream, outcome).await;
        match detail {
            Ok(detail) => {
                if let Err(error) = require_response_eof(stream).await {
                    return Err(EnsureEngineError::DetailedFailure { outcome, code, detail: format!("{detail}; {error}") });
                }
                if outcome == 0 { return Ok(()); }
                return Err(EnsureEngineError::DetailedFailure { outcome, code, detail });
            }
            Err(detail) => return Err(EnsureEngineError::DetailedFailure { outcome, code, detail }),
        }
    }
    if let Err(error) = require_response_eof(stream).await {
        if outcome == 1 { return Err(EnsureEngineError::DetailedFailure { outcome, code, detail: error.to_string() }); }
        return Err(error);
    }
    if outcome == 0 { Ok(()) } else { Err(EnsureEngineError::Rejected { outcome, code }) }
}
async fn read_detail(stream: &mut UnixStream, outcome: u8) -> Result<String, String> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).await.map_err(|error| format!("detail length: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > DETAIL_MAX || (outcome == 0 && length != 0) || (outcome == 1 && length < 28) {
        return Err(format!("invalid detail length={length}"));
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.map_err(|error| format!("detail payload: {error}"))?;
    if outcome == 0 { Ok(String::new()) } else { decode_detail(&payload) }
}
fn decode_detail(bytes: &[u8]) -> Result<String, String> {
    let invalid = || "invalid EnsureEngine detail payload".to_owned();
    if bytes.len() < 28 || bytes.len() > DETAIL_MAX || bytes[0] != 1 || !matches!(bytes[1], 1 | 2)
        || bytes[2] > 2 || bytes[3] > 2 || bytes[4] & !31 != 0 || bytes[5] != 0
        || (bytes[1] == 2 && (bytes[2] != 0 || bytes[3] != 0))
        || (bytes[1] == 1 && !matches!((bytes[2], bytes[3]), (1, 1) | (2, 2)))
        || (bytes[1] == 2 && bytes[4] & 3 != 0) { return Err(invalid()); }
    let mut fields = Vec::new();
    let mut offset = 28;
    for index in 0..3 {
        let length = u16::from_be_bytes([bytes[22 + index * 2], bytes[23 + index * 2]]) as usize;
        if length > (if index == 2 { 2048 } else { 64 }) || offset + length > bytes.len() { return Err(invalid()); }
        fields.push(std::str::from_utf8(&bytes[offset..offset + length]).map_err(|_| invalid())?);
        offset += length;
    }
    if offset != bytes.len() || fields[0].is_empty() { return Err(invalid()); }
    let mut numbers = [None; 4];
    for index in 0..4 {
        let value = i32::from_be_bytes(bytes[6 + index * 4..10 + index * 4].try_into().expect("fixed detail number"));
        if bytes[4] & (1 << index) != 0 { numbers[index] = Some(value); }
        else if value != 0 { return Err(invalid()); }
    }
    let action = ["none", "kickstart", "bootstrap"][bytes[2] as usize];
    let target = ["none", "system/com.exv.vpn.engine", "/Library/LaunchDaemons/com.exv.vpn.engine.plist"][bytes[3] as usize];
    Ok(format!("{} source={} action={action} target={target} exit={:?} signal={:?} errno={:?} restore_errno={:?} step={} reason={} truncated={}",
        fields[0], bytes[1], numbers[0], numbers[1], numbers[2], numbers[3], fields[1], fields[2], bytes[4] & 16 != 0))
}

/// 请求已安装 root 服务代理回答只读 Status ping（W3-4/P4 v1）。
///
/// 帧与 [`start_engine`] 同源的 V1 固定格式：16B 零载荷 action frame
/// （`EXVA` + version + `Status`，owner uid 填 Core 自身普通身份），无第二段载荷；
/// daemon 读毕即要求写半关闭，应答 16B（`EXVR` + version + `Status` + outcome +
/// code）。返回 accepted 事实——service agent 健康探测只关心「daemon 活着并接受本
/// owner 的请求」，不索取更多状态。
///
/// # Errors
///
/// socket 不可连、IO 失败或身份非法（root uid 不能作为 V1 owner）→
/// [`ServiceAgentStatusError::Unavailable`]；得到应答但 outcome 非接受、或应答帧不符合
/// V1 固定格式 → [`ServiceAgentStatusError::Rejected`]。两者都不区分底层 IO 文本。
pub(crate) async fn request_status(
    credentials: CoreCredentials,
) -> Result<(), ServiceAgentStatusError> {
    if credentials.uid() == 0 {
        // V1 request 帧拒绝 root owner（与 ServiceAgentRequestV1::new 同一不变式）；
        // Core 必须是普通用户进程，此处保守回落为不可用而不是构造非法帧。
        return Err(ServiceAgentStatusError::Unavailable);
    }
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .await
        .map_err(|_| ServiceAgentStatusError::Unavailable)?;

    let mut request = [0_u8; REQUEST_LEN];
    request[0..4].copy_from_slice(b"EXVA");
    request[4] = 1;
    request[5] = STATUS;
    request[8..12].copy_from_slice(&credentials.uid().to_be_bytes());

    stream
        .write_all(&request)
        .await
        .map_err(|_| ServiceAgentStatusError::Unavailable)?;
    stream
        .shutdown()
        .await
        .map_err(|_| ServiceAgentStatusError::Unavailable)?;

    let mut response = [0_u8; RESPONSE_LEN];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| ServiceAgentStatusError::Unavailable)?;
    // V1 固定应答校验：magic/version/action/保留位（7 与 12..）全零；outcome 在 [6]
    //（0=accepted），code 在 [8..12]（accepted 时恒零）。
    if response[0..4] != *b"EXVR"
        || response[4] != 1
        || response[5] != STATUS
        || response[7] != 0
        || response[12..].iter().any(|byte| *byte != 0)
    {
        return Err(ServiceAgentStatusError::Rejected);
    }
    // outcome 在 [6]（0=accepted），code 在 [8..12]（accepted 时恒零）。
    if response[6] == 0 && response[8..12].iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(ServiceAgentStatusError::Rejected)
    }
}
