//! 隔离验证产品系统浏览器启动、CDP 交接与取消；不访问学校认证。
use exv_core::atrust_browser::BrowserLogin;
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() {
    let profile = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("需要指定全新的隔离 profile 路径"),
    );
    let reuse = std::env::args_os()
        .nth(2)
        .is_some_and(|flag| flag == "--reuse");
    assert_eq!(
        profile.exists(),
        reuse,
        "首次需使用全新隔离 profile；复用需传 --reuse"
    );
    eprintln!("isolated_profile={}", profile.display());
    let login = BrowserLogin::default();
    let cancellation = async {
        tokio::time::sleep(Duration::from_secs(12)).await;
        let _ = login.cancel();
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
