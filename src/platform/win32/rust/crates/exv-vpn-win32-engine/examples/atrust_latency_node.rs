//! Windows 真实节点无网卡实验；认证只从 stdin 的 BrowserSession 获取。
//! 物理出口、认证、TLS、L3、PacketEngine 和实验样本循环均复用既有实现。
#![allow(dead_code)] // path include 同时带入 Common 本地对端的辅助模块。
#[path = "../../../../../../common/rust/crates/exv-vpn-atrust/examples/support/latency_lab/mod.rs"]
mod latency_lab;
#[path = "support/latency_node/mod.rs"]
mod latency_node;
#[path = "support/node_tcp_observation.rs"]
mod node_tcp_observation;
#[path = "support/windows_tcp_info.rs"]
mod windows_tcp_info;

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let options = match latency_node::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(2);
        }
    };
    let cancel = exv_vpn_atrust::session::SessionFactory::new().open();
    let interrupt = cancel.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = interrupt.close();
        }
    });
    let passed = latency_node::run(options, cancel, &mut |value| println!("{value}")).await;
    signal.abort();
    let _ = signal.await;
    if !passed {
        std::process::exit(1);
    }
}
