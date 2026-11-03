//! 数据节点的独立 IPv4 地址申请与持续隧道初始握手。
//!
//! 按固定上游 `ip.go::getIP/readIPTunnelResponses` 与
//! `l3tunnelconn.go::wrapAuthReqData@8de443f` 重建；持续 IPv4 握手已于 2026-09-22
//! 随 Rust 校内 DNS 探针通过学校真实节点验证，独立地址申请仍只有本地证据。
//! 调用者负责经 Underlay 建连并完成节点 TLS 身份校验；这里接收已验证的流，
//! 不自行建立 socket、改变 TLS 策略或修改网卡/路由。
//! [`request_ipv4`] 最终释放传入流值；[`open_ipv4_tunnel`] 成功后保留流供后续每流授权与数据收发。
//! 此处只实现初始 IPv4 握手，不包含每流授权、心跳或数据循环。

use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::Secret;

/// 单次地址申请的限时和响应大小配置，不包含重试策略。
#[derive(Debug, Clone, Copy)]
pub struct IpRequestOptions {
    /// 覆盖写请求及全部响应的总时限。
    pub timeout: Duration,
    /// 认证 JSON 最大字节数，在分配及读取之前检查。
    pub max_response_bytes: usize,
}

impl Default for IpRequestOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_response_bytes: 8192,
        }
    }
}

/// 固定阶段和数字错误，不包含 SID、响应正文或服务端 message。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IpRequestError {
    /// 参数不符合本交互要求。
    #[error("IP request: invalid options")]
    InvalidOptions,
    /// SID 为空。
    #[error("IP request: empty SID")]
    EmptySid,
    /// 编码后的 SID JSON 不适配 be16 长度。
    #[error("IP request: request too large")]
    RequestTooLarge,
    /// 总时限耗尽；没有后台重试任务。
    #[error("IP request: timeout")]
    Timeout,
    /// 流的读写错误，只有固定阶段和 `ErrorKind`。
    #[error("IP request {stage}: {kind:?}")]
    Io {
        stage: &'static str,
        kind: ErrorKind,
    },
    /// 方法协商响应不是 05 D0。
    #[error("IP request: invalid method response")]
    InvalidMethod,
    /// 认证响应版本不是 53。
    #[error("IP request: invalid auth version")]
    InvalidAuthVersion,
    /// 认证帧状态非零。
    #[error("IP request: authentication status {0}")]
    AuthStatus(u8),
    /// 认证响应超出调用方上限。
    #[error("IP request: response length {declared} exceeds {limit}")]
    ResponseTooLarge { declared: usize, limit: usize },
    /// 认证正文非 JSON，或没有整数 code。
    #[error("IP request: invalid auth JSON")]
    InvalidAuthJson,
    /// 网关拒绝该 SID；保留业务码但不保留自由文本。
    #[error("IP request: rejected with code {code}")]
    Rejected { code: i64 },
    /// VIP 响应版本不正确。
    #[error("IP request: invalid VIP version")]
    InvalidVipVersion,
    /// VIP 响应状态非零。
    #[error("IP request: VIP status {0}")]
    VipStatus(u8),
    /// 本切片只支持 IPv4；不得把 IPv6 数据误读成 IPv4。
    #[error("IP request: unsupported address type {0}")]
    UnsupportedAddressType(u8),
}

fn io_error(stage: &'static str, error: &std::io::Error) -> IpRequestError {
    IpRequestError::Io {
        stage,
        kind: error.kind(),
    }
}

#[derive(Serialize)]
struct Request<'a> {
    sid: &'a str,
}

#[derive(Deserialize)]
struct Reply {
    code: i64,
}

/// 向已经验证 TLS 身份的数据节点申请 IPv4；成功或失败都会释放传入的流值。
///
/// 调用方应转移拥有连接的流，底层连接释放遵循其 `Drop` 实现；不接受局部借用流。
/// `AsyncRead`/`AsyncWrite` 本身不能保证任意自定义类型的底层资源释放行为。
///
/// ```compile_fail
/// use exv_vpn_atrust::{Secret, transport::l3::handshake::{request_ipv4, IpRequestOptions}};
/// async fn borrowed(stream: &mut tokio::io::DuplexStream, sid: &Secret) {
///     let _ = request_ipv4(stream, sid, IpRequestOptions::default()).await;
/// }
/// ```
///
/// # Errors
///
/// 返回参数、限时、I/O、认证与 VIP 格式错误；不自动重试或回退 TLS 校验。
pub async fn request_ipv4<S>(
    mut stream: S,
    sid: &Secret,
    options: IpRequestOptions,
) -> Result<Ipv4Addr, IpRequestError>
where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    negotiate_ipv4(&mut stream, sid, options, false).await
}

/// 执行持续 L3 隧道的初始 IPv4 握手，成功后返回原流及 VIP。
///
/// 上游 `authTunnel` 允许状态成功但正文为空；这与独立 `getIP` 的 JSON 要求不同。
/// 返回的流仍需每流授权才能转发数据；获得 VIP 不等于任意目标已授权。
/// 错误或取消会释放所拥有的流值，底层连接释放遵循其 `Drop` 实现；
/// 不接受局部借用流，成功后的释放由返回流的所有者负责。
///
/// # Errors
///
/// 返回与 [`request_ipv4`] 相同的限时、格式和认证错误。不支持仅 IPv6 或双栈 VIP 响应。
pub async fn open_ipv4_tunnel<S>(
    mut stream: S,
    sid: &Secret,
    options: IpRequestOptions,
) -> Result<(S, Ipv4Addr), IpRequestError>
where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let vip = negotiate_ipv4(&mut stream, sid, options, true).await?;
    Ok((stream, vip))
}

async fn negotiate_ipv4<S>(
    stream: &mut S,
    sid: &Secret,
    options: IpRequestOptions,
    allow_empty_auth: bool,
) -> Result<Ipv4Addr, IpRequestError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if options.timeout.is_zero()
        || options.max_response_bytes == 0
        || options.max_response_bytes > usize::from(u16::MAX)
    {
        return Err(IpRequestError::InvalidOptions);
    }
    if sid.is_empty() {
        return Err(IpRequestError::EmptySid);
    }
    if sid.len() > usize::from(u16::MAX) {
        return Err(IpRequestError::RequestTooLarge);
    }
    let payload = Zeroizing::new(
        serde_json::to_vec(&Request { sid: sid.expose() })
            .map_err(|_| IpRequestError::RequestTooLarge)?,
    );
    let length = u16::try_from(payload.len()).map_err(|_| IpRequestError::RequestTooLarge)?;
    let mut request = Zeroizing::new(Vec::with_capacity(payload.len() + 17));
    request.extend_from_slice(&[5, 1, 0xd0, 0x53, 0]);
    request.extend_from_slice(&length.to_be_bytes());
    request.extend_from_slice(&payload);
    request.extend_from_slice(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]);
    drop(payload);
    tokio::time::timeout(options.timeout, async {
        stream
            .write_all(&request)
            .await
            .map_err(|e| io_error("write", &e))?;
        stream.flush().await.map_err(|e| io_error("flush", &e))?;
        drop(request);
        let mut method = [0; 2];
        stream
            .read_exact(&mut method)
            .await
            .map_err(|e| io_error("method", &e))?;
        if method != [5, 0xd0] {
            return Err(IpRequestError::InvalidMethod);
        }
        let mut header = [0; 4];
        stream
            .read_exact(&mut header)
            .await
            .map_err(|e| io_error("auth-header", &e))?;
        if header[0] != 0x53 {
            return Err(IpRequestError::InvalidAuthVersion);
        }
        if header[1] != 0 {
            return Err(IpRequestError::AuthStatus(header[1]));
        }
        let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
        if length > options.max_response_bytes {
            return Err(IpRequestError::ResponseTooLarge {
                declared: length,
                limit: options.max_response_bytes,
            });
        }
        let mut body = Zeroizing::new(vec![0; length]);
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| io_error("auth-body", &e))?;
        if !allow_empty_auth || !body.is_empty() {
            let reply: Reply =
                serde_json::from_slice(&body).map_err(|_| IpRequestError::InvalidAuthJson)?;
            if reply.code != 0 {
                return Err(IpRequestError::Rejected { code: reply.code });
            }
        }
        drop(body);
        stream
            .read_exact(&mut header)
            .await
            .map_err(|e| io_error("vip-header", &e))?;
        if header[0] != 5 {
            return Err(IpRequestError::InvalidVipVersion);
        }
        if header[1] != 0 {
            return Err(IpRequestError::VipStatus(header[1]));
        }
        if header[3] != 1 {
            return Err(IpRequestError::UnsupportedAddressType(header[3]));
        }
        // 上游保留 header[2]，IPv4 后两个字节不参与地址；此处保持相同线格式。
        let mut vip = [0; 6];
        stream
            .read_exact(&mut vip)
            .await
            .map_err(|e| io_error("vip-body", &e))?;
        Ok(Ipv4Addr::new(vip[0], vip[1], vip[2], vip[3]))
    })
    .await
    .map_err(|_| IpRequestError::Timeout)?
}
