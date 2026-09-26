//! macOS 特权 Engine 进程入口。
//!
//! E4 只启动 bootstrap 与 Observe-only 控制面；连接、CSTP、`utun` 和网络资源事务仍由
//! 后续已批准任务接线。

fn main() {
    if exv_vpn_darwin_engine::run().is_err() {
        std::process::exit(1);
    }
}
