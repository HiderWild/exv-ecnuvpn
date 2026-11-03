// 仅输出本机 v0 ABI；未接线到节点，不创建 socket，也不产生节点指标。
#[path = "support/windows_tcp_info.rs"]
mod windows_tcp_info;

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--abi"] {
        eprintln!("用法：windows_tcp_info_probe --abi");
        std::process::exit(2);
    }
    println!("{}", windows_tcp_info::abi());
}
