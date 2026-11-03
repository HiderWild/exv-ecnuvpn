//! EXV v2 浏览器交接类型的 darwin IPC 入口 re-export（win32-ipc 同款形态）。
//!
//! `BrowserSession`/`BrowserSessionError` 与共享 Cookie 纯构造的唯一实现在 Common 的
//! aTrust crate（`exv_vpn_atrust::auth::browser_session`）；本模块只转发，不保留第二份
//! 实现或第二份常量。依赖方向固定为 Common ← 平台（darwin IPC），Common 不得反向依赖
//! 本 crate。信封 V2 的 aTrust 载荷校验（`connect_envelope`）与 UI 凭据包 v2
//! （darwin-core `credential`）都消费这里的同一类型。

pub use exv_vpn_atrust::auth::browser_session::{
    BrowserSession, BrowserSessionError, CookieSessionError, GATEWAY_COOKIE_NAMES,
    session_from_cookie_pairs,
};
// M5：网关端点规范化权威同样经本入口转发（darwin-core 会话缓存按规范化端点做
// SHA256 键控；校验零重写、依赖方向不变——Common ← 平台）。
pub use exv_vpn_atrust::auth::origin::GatewayOrigin;
// M5：内嵌认证窗的 authConfig 探针路径构造（`AuthClientIdentity::auth_config_path`）
// 与身份声明也经本入口转发（win32 经 exv-core 暴露的同面；路径构造零重写）。
pub use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
