//! Windows aTrust 认证交接的组合入口：显式 Windows 身份 + 产品资源请求体，
//! 委托 Common `browser_handoff` 编排。
//! 由 tunnel_runtime.rs 的 aTrust 分支通过 atrust_prepare 调用。
//! 业务状态机、阶段错误与认证结果都在 Common；本文件不保留实现副本，只做宿主选值。
// Windows 产品路径经 atrust_prepare 进入运行时（tunnel_runtime.rs 的 aTrust 分支）；
// 真实产品流尚待验收，接线与验收边界见 docs/research/atrust/t07-runtime-assembly.md。

use exv_vpn_atrust::Underlay;
use exv_vpn_atrust::auth::AuthClientOptions;
use exv_vpn_atrust::auth::browser_handoff;
use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use std::sync::Arc;

/// 端点解释失败的唯一构造点仍在 Common；本宿主只转发同一文案。
pub(crate) use exv_vpn_atrust::auth::browser_handoff::invalid_https_origin;
pub use exv_vpn_atrust::auth::browser_handoff::{
    AtrustAuthError, AuthStage, AuthenticatedAtrustSession,
};

/// Windows 产品选择的资源请求体：appList、favoriteAppList、featureCenter、uemSpace 组合。
/// 这是产品选择而不是协议默认值，Common 交接函数按字节参数接收。
const RESOURCE_REQUEST: &[u8] = br#"{"resourceType":{"sdpPolicy":{},"appList":{},"favoriteAppList":{},"featureCenter":{},"uemSpace":{"params":{"action":"login"}}}}"#;

/// Windows 产品声明的 aTrust 协议身份；平台名是协议字段，绝不按编译 target 推导。
/// （T6a 起对 crate 内重连路径可复用：源会话复核探测同一身份。）
pub(crate) fn client_identity() -> AuthClientIdentity {
    AuthClientIdentity::new("SDPClient", "Windows", "zh-CN")
        .expect("static Windows client identity is valid")
}

/// 消费浏览器候选会话并建立 Engine 独立认证会话。出口由 Windows 连接 owner 注入。
/// `expected_gateway` 是本次连接已经选定的网关，不能由回传 Cookie 临时改写。
/// UA 使用候选会话的声明（Common 内部强制优先）；identity 与预算由本入口显式提供。
///
/// # Errors
/// 委托 Common 编排；返回失败的精确阶段及协议库已脱敏原因。
pub async fn authenticate_browser_session(
    browser: BrowserSession,
    expected_gateway: &str,
    underlay: Arc<dyn Underlay>,
) -> Result<AuthenticatedAtrustSession, AtrustAuthError> {
    let options = AuthClientOptions {
        user_agent: browser.user_agent().into(),
        ..AuthClientOptions::new(client_identity())
    };
    browser_handoff::authenticate_browser_session(
        browser,
        expected_gateway,
        underlay,
        options,
        RESOURCE_REQUEST,
    )
    .await
}

// 已接入产品 assemble 的 aTrust 分支（经 atrust_prepare）；真实产品流尚待验收。
// 接线与验收边界见 docs/research/atrust/t07-runtime-assembly.md。
