//! 隔离验证产品 Chrome 启动与取消；只导航到本机关闭端口，不访问学校认证。
use exv_core::atrust_browser::BrowserLogin;
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() {
    let profile = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("EXV/atrust-debug/browser-launch-probe");
    let login = BrowserLogin::default();
    let cancellation = async {
        tokio::time::sleep(Duration::from_secs(12)).await;
        login.cancel();
    };
    let acquisition = login.acquire_with_status("https://127.0.0.1:9", &profile, |waiting| {
        eprintln!("browser_waiting={waiting}");
    });
    let (result, ()) = tokio::join!(acquisition, cancellation);
    match result {
        Ok(_) => {
            eprintln!("unexpected authenticated session from closed local port");
            std::process::exit(1);
        }
        Err(error) => {
            println!("browser_result={error}");
            if error != "浏览器登录已取消" {
                std::process::exit(1);
            }
        }
    }
}
