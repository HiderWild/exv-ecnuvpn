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

/// W2.5：请求已安装 root daemon 幂等确保 service engine job 已在系统服务域
/// 加载（已加载视成功——产出受监护的常驻 engine，不做裸 spawn；具体机制在
/// service agent 侧，见 src/platform/darwin/rust/crates/exv-vpn-darwin-service-agent）。
///
/// 帧与 [`request_status`] 同源的 V1 零载荷 action frame（action=6）；daemon 应答
/// accepted/rejected(EnsureEngineFailed)。修复分层（规范 §二.2/E3）：daemon 在场时
/// core 经此免提权拉起 engine。
///
/// # Errors
///
/// socket 不可连、IO 失败或身份非法 → [`ServiceAgentStatusError::Unavailable`]；
/// 得到应答但 outcome 非接受、或应答帧不符合 V1 固定格式 →
/// [`ServiceAgentStatusError::Rejected`]。
pub(crate) async fn ensure_engine(
    credentials: CoreCredentials,
) -> Result<(), ServiceAgentStatusError> {
    if credentials.uid() == 0 {
        return Err(ServiceAgentStatusError::Unavailable);
    }
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .await
        .map_err(|_| ServiceAgentStatusError::Unavailable)?;

    let mut request = [0_u8; REQUEST_LEN];
    request[0..4].copy_from_slice(b"EXVA");
    request[4] = 1;
    request[5] = ENSURE_ENGINE;
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
    if response[0..4] != *b"EXVR"
        || response[4] != 1
        || response[5] != ENSURE_ENGINE
        || response[7] != 0
        || response[12..].iter().any(|byte| *byte != 0)
    {
        return Err(ServiceAgentStatusError::Rejected);
    }
    if response[6] == 0 && response[8..12].iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(ServiceAgentStatusError::Rejected)
    }
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
