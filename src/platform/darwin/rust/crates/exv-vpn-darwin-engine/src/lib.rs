//! macOS 无签名 Rust Engine 的唯一 runtime 入口。
//!
//! 当前骨架固定为无特权前台应用和提升权限 Engine 的控制边界。Engine 未来独占 `utun`、
//! 网络资源事务、CSTP session 与 IPv4 packet pump；Core 不能承担这些职责。在接线完成前，
//! 启动必须明确失败，不能发布虚假的 Connected。

// 服务修订号作为二进制元数据嵌入 `__TEXT,__exv_revision`（编译期常量 → 4 字节
// u32 little-endian 数据 section；`#[used]` 防回收）。查询侧（core）不运行本进程、
// 直接解析 Mach-O 读该 section——见 `exv_vpn_darwin_ipc::service_revision`。
exv_vpn_darwin_ipc::embed_service_revision!();

mod bootstrap_runtime;
mod reconnect_fault;
mod connect_gate;
mod core_exit_policy;
#[path = "../../../build_identity.rs"]
pub(crate) mod build_identity;
pub mod egress;
pub mod file_log;
pub mod log_sink;
pub mod packet;
pub mod platform;
pub mod protocol;
pub mod service_runtime;
mod session_diagnostics;
pub mod stats;
pub mod system_proxy;

/// Engine 二进制入口调用的唯一库接缝。
///
/// # Errors
///
/// E4 bootstrap 建立 root Engine 的 Common 控制服务；`MAC-CSTP-05` 起该服务
/// 执行真实 WebVPN/CSTP 协商并发布真实阶段事件，但不创建 `utun`、不应用
/// 路由/DNS、不启动数据面。
pub fn run() -> Result<(), bootstrap_runtime::EngineBootstrapError> {
    if let Err(error) = system_proxy::cleanup_installed() {
        eprintln!("previous proxy policy cleanup pending: {error}");
    }
    bootstrap_runtime::install_signal_shutdown();
    bootstrap_runtime::run_from_process()
}

mod stop_operations;

pub mod atrust_auth;
pub mod atrust_node_tls;
pub mod atrust_prepare;
pub mod atrust_tunnel;
pub mod campus_resolver;
pub mod connect_intent;
pub mod tunnel_hold;
pub mod underlay;

mod official_auth_receiver;

/// 完整卸载前恢复当前用户原官方认证服务配置，复用现有记录及路径。
///
/// # Errors
/// 返回实际恢复步骤、系统码与原错误，调用方保留卸载入口供重试。
pub fn restore_auth_receiver_for_uninstall(owner_uid: u32) -> Result<(), String> {
    official_auth_receiver::restore_for_uninstall(owner_uid)
}
