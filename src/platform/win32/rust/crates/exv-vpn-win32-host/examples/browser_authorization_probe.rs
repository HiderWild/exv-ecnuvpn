//! 从产品浏览器模块取得会话并交给真实 Engine 认证；不配置虚拟网卡或路由。
use exv_core::atrust_browser::BrowserLogin;
use exv_engine::{atrust_auth::authenticate_browser_session, atrust_underlay::PhysicalUnderlay};
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), String> {
    let ifindex = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v != 0)
        .ok_or("需要显式指定物理接口索引")?;
    let profile = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA")?)
        .join("EXV/atrust-browser");
    let login = BrowserLogin::default();
    let candidate = tokio::time::timeout(
        Duration::from_secs(25),
        login.acquire_with_status("https://vpn.ecnu.edu.cn", &profile, |active| {
            println!(
                "{}",
                serde_json::json!({"stage":"browser-wait","active":active})
            );
        }),
    )
    .await
    .map_err(|_| "浏览器交接诊断超时")??;
    println!(
        "{}",
        serde_json::json!({"stage":"browser-candidate","received":true})
    );
    let bytes = candidate.encode().map_err(|_| "会话编码失败")?;
    drop(candidate);
    let received = BrowserSession::decode(&bytes).map_err(|_| "会话解码失败")?;
    drop(bytes);
    let underlay = Arc::new(PhysicalUnderlay::new(ifindex)?);
    let session = tokio::time::timeout(
        Duration::from_secs(190),
        authenticate_browser_session(received, "https://vpn.ecnu.edu.cn", underlay),
    )
    .await
    .map_err(|_| "Engine 认证诊断超时")?
    .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({
            "stage":"engine-authenticated", "authenticated":true,
            "resource_bytes":session.resource_bytes(), "resource_entries":session.catalog().entry_count(),
            "wintun_configured":false
        })
    );
    Ok(())
}
