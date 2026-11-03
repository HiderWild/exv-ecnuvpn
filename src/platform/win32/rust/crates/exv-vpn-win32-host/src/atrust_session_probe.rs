//! 宿主直连网关会话探针（内嵌认证路径脱离页面上下文，方案 A）。
//!
//! 背景：内嵌路径原先靠「注入脚本 → 页面内 `await fetch` → 写 cookie → 宿主轮询读回」，
//! 一旦落在被门户反复 reload 的 `/portal/shortcut.html` 或 JSON 视图上，promise 随页面
//! realm 销毁而丢失 → 零结果 → 停滞误报。本模块让宿主自己直接向**固定网关 origin** 发
//! 一次 HTTPS GET `authConfig`，完全不经过页面。
//!
//! 复用点：内部使用引擎侧成熟的 [`AuthClient`]，路径与页面探针**逐字同源**
//! （[`crate::atrust_browser::client_identity`] 的 `auth_config_path(false)`）。权威判定仍在
//! 引擎（`browser_handoff` 用 `PhysicalUnderlay` 复核 `is_login`），本探针只承担「是否已登录」
//! 的存活探测。
//!
//! 边界（强制）：
//! - 只向固定网关 origin 发送（由 [`AuthClient`] 的同源约束保证）；
//! - 调用方只在窗口存活且 lease 未取消时调用；
//! - 短超时（[`PROBE_TIMEOUT`]）避免拖慢既有 ~1s 轮次；
//! - **不落日志、不落盘** cookie 值与完整正文；错误只以脱敏分类返回（[`ProbeError`]）。

use exv_vpn_atrust::auth::{AuthClient, AuthClientOptions, AuthHttpError, AuthStatus};
use exv_vpn_atrust::transport::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay,
};
use exv_vpn_atrust::{Secret, UnderlayError, UnderlayErrorKind};
use std::{sync::Arc, time::Duration};

/// 直连探针超时：覆盖拨号、TLS、响应头与正文；明显短于 `AuthClient` 默认 30s，
/// 避免拖慢既有 ~1s 轮次。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// 宿主直连探针结果。绝不携带 cookie 值或正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProbe {
    /// 网关 `authConfig` 业务码为 0 且 JSON 形状可解析（等价页面探针的 `code_ok`）。
    /// 返回 `Ok` 时恒为 `true`；失败由 [`probe_auth_config`] 的 `Err` 分支表达。
    pub code_ok: bool,
    /// 网关 `data.isLogin == 1`。
    pub is_login: bool,
    /// `data.firstAuth[0]`（登录入口路径）；缺失或空数组为 `None`。
    pub first_auth: Option<String>,
}

/// 直连探针失败：只保留脱敏稳定分类，不含 cookie、正文或自由文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeError {
    /// 稳定分类码（N5 脱敏白名单词汇）。
    pub code: &'static str,
    /// HTTP 非 2xx 时的状态码；其它失败为 `None`。
    pub http_status: Option<u16>,
}

impl ProbeError {
    /// 供 `embedded.host_probe result=...` 使用的稳定标签：HTTP 状态优先，否则分类码。
    #[must_use]
    pub fn label(self) -> String {
        self.http_status.map_or_else(
            || self.code.to_owned(),
            |status| format!("http:{status}"),
        )
    }

    fn from_auth(error: AuthHttpError) -> Self {
        if let Some(diagnostic) = error.transport_diagnostic() {
            log::warn!(target: "exv.atrust_auth", "session_probe.transport {diagnostic}");
        }
        let (code, http_status) = match error {
            AuthHttpError::InvalidConfiguration => ("config", None),
            AuthHttpError::InvalidCookie => ("cookie", None),
            AuthHttpError::Timeout => ("timeout", None),
            AuthHttpError::Underlay(_) => ("underlay", None),
            AuthHttpError::Tls { .. } => ("tls", None),
            AuthHttpError::Http { .. } => ("http", None),
            AuthHttpError::HttpStatus { status } => ("http", Some(status)),
            AuthHttpError::SizeLimit => ("size_limit", None),
            AuthHttpError::Protocol => ("protocol", None),
            AuthHttpError::Rejected { .. } => ("rejected", None),
            AuthHttpError::MissingCsrf => ("csrf", None),
            AuthHttpError::MissingSignKey => ("sign_key", None),
        };
        Self { code, http_status }
    }
}

/// 宿主直连出口：初始连接阶段尚无 EXV 隧道，系统默认路由即物理出口，普通 TCP 连接即可。
/// 该出口**只**服务于本探针；权威交接仍由引擎 `PhysicalUnderlay` 完成。
///
/// 注意：这里刻意使用标准库默认拨号（`TcpStream::connect`），因为该路径先于任何 EXV
/// 隧道/路由安装；不修改系统网卡、路由、DNS 或代理。
pub struct DirectUnderlay;

impl Underlay for DirectUnderlay {
    fn resolve(&self, host: &str, port: u16) -> ResolveFuture<'_> {
        let endpoint = format!("{host}:{port}");
        Box::pin(async move {
            tokio::net::lookup_host(endpoint)
                .await
                .map(|addrs| addrs.map(|addr| addr.ip()).collect())
                .map_err(|_| {
                    UnderlayError::new(
                        UnderlayErrorKind::ResolveFailed,
                        ConnectPurpose::AuthGateway,
                    )
                })
        })
    }

    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        let host = target.host.clone();
        let port = target.port;
        let purpose = target.purpose;
        Box::pin(async move {
            tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map(|stream| Box::new(stream) as Box<dyn exv_vpn_atrust::transport::UnderlayStream>)
                .map_err(|_| UnderlayError::new(UnderlayErrorKind::ConnectFailed, purpose))
        })
    }
}

/// 用宿主直连向固定网关发一次 `authConfig` 探针（路径与页面探针逐字同源）。
///
/// `cookie_header` 只用于本次请求，**不落日志、不落盘**；调用方只在窗口存活且 lease 未取消时调用。
///
/// # Errors
/// 返回脱敏分类的 [`ProbeError`]；不返回 cookie、正文或自由文本。
pub async fn probe_auth_config(
    gateway: &str,
    cookie_header: &str,
    user_agent: &str,
) -> Result<SessionProbe, ProbeError> {
    // 身份与路径来自与页面探针同一个 `client_identity()`，保证 `auth_config_path(false)` 字节一致。
    let options = AuthClientOptions {
        user_agent: user_agent.to_owned(),
        timeout: PROBE_TIMEOUT,
        ..AuthClientOptions::new(crate::atrust_browser::client_identity())
    };
    let mut client =
        AuthClient::new(gateway, Arc::new(DirectUnderlay), options).map_err(ProbeError::from_auth)?;
    // E11（2026-10-09 区域 C 计划 §5.3 方案 A）：空/空白 cookie 头按「无 cookie」
    // 语义处理——跳过 import（客户端本就无 cookie 起步，语义等价导入空 cookie 集），
    // 而不把空串喂给 `import_gateway_cookies`：common 侧 `"".split(';')` 产出单个
    // 空片段、`cookie_pair` 的 `split_once('=')` 失败，恒判 `InvalidCookie`（标签
    // `cookie`），冷缓存预探针因此从未走通 `not_logged_in` 直达快路径。
    // 不动 common：`import_gateway_cookies` 的「空串=非法」设计语义保持；其余两个
    // 生产调用点（engine `browser_handoff`/`reconnect`）由 `session_from_cookie_pairs`
    // 强制非空 sid 的不变量保证恒传非空头（`browser_session.rs` `MissingSession`）。
    if !cookie_header.trim().is_empty() {
        client
            .import_gateway_cookies(Secret::new(cookie_header.to_owned()))
            .map_err(ProbeError::from_auth)?;
    }
    let status = client
        .auth_config(false)
        .await
        .map_err(ProbeError::from_auth)?;
    Ok(probe_from_status(status))
}

/// 把 [`AuthStatus`] 映射为探针结果：`firstAuth` 取首个入口（等价页面探针的 `firstAuth[0]`）。
fn probe_from_status(status: AuthStatus) -> SessionProbe {
    SessionProbe {
        code_ok: true,
        is_login: status.is_login,
        first_auth: status.first_auth.and_then(|paths| paths.into_iter().next()),
    }
}
