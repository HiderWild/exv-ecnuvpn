//! 本轮候选连接的有限 EStats 观察入口；不接入产品。
#[path = "support/tcp_estats_local_observer.rs"]
mod observer;

fn main() {
    if let Err(error) = observer::main_entry() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
