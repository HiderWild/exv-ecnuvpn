//! EXV v2 浏览器交接类型的 Windows IPC 入口 re-export。
//!
//! `BrowserSession`/`BrowserSessionError` 与共享 Cookie 纯构造已迁到 Common 的 aTrust crate
//! （`exv_vpn_atrust::auth::browser_session`），本模块只转发，不保留第二份实现或第二份常量。
//! 依赖方向固定为 Common ← 平台（Win32 IPC），Common 不得反向依赖本 crate。

pub use exv_vpn_atrust::auth::browser_session::{
    BrowserSession, BrowserSessionError, CookieSessionError, GATEWAY_COOKIE_NAMES,
    session_from_cookie_pairs,
};
