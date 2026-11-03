//! F8独立本机实验；仅显式--run领用随机创建者adapter，不配置地址/路由或发送包。
#[path = "support/wintun_raw_local_lab.rs"]
mod lab;

fn main() {
    let options = match lab::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(2);
        }
    };
    if !lab::run(options, &mut |value| println!("{value}")) {
        std::process::exit(1);
    }
}
