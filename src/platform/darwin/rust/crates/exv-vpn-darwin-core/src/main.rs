//! macOS 非特权 Core 进程入口。
//!
//! `MAC-ALIGN-00` 只固定 binary → library 的边界。后续任务才会接入 UI→Core 控制面、
//! Engine 生命周期和 shutdown；当前入口必须 fail closed。

fn main() {
    if let Err(error) = exv_vpn_darwin_core::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
