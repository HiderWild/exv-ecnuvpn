//! macOS 非特权 Core 的库边界。
//!
//! Core 后续只负责 UI 控制语义、Engine 生命周期、认证 IPC 与状态投影。它不持有 CSTP、
//! `utun`、route、DNS 或 packet 数据通路。

pub(crate) mod service_agent_client;
pub mod config;
pub(crate) mod atrust_session_cache;
pub mod atrust_cas_facts;
pub mod atrust_cas_login;
pub mod sso_session_jar;
pub mod atrust_auth_storage;
pub mod atrust_http_underlay;
pub mod atrust_session_probe;
// C 系列 C2（2026-10-28 原生系统浏览器认证模式）：本地 Agent HTTPS 服务端的
// 根 CA/叶子证书生命周期 + 用户 login keychain 信任管理（计划 C2 节）。
pub mod agent_certs;
// C 系列 C3（2026-10-28 原生系统浏览器认证模式）：C1 服务端 + C2 证书的常驻宿主
// （票据到达即兑换 + 免密触发去重透传；计划 C3 节）。
pub mod local_agent_host;
pub(crate) mod config_crypto;
pub(crate) mod config_hygiene;
pub(crate) mod config_paths;
pub mod credential;
pub mod elevation;
pub mod engine_client;
pub mod engine_lifecycle_real;
pub(crate) mod kernel_control_service;
pub mod log_aggregator;
pub mod log_control;
pub(crate) mod power;
pub(crate) mod proxy_tun;
pub(crate) mod system_proxy;
// W3-4/P4 v1：ServiceControl query 的 service agent 健康探测（五态词汇复用，query-only）。
pub(crate) mod service_status;
// 服务修订号观测面（expected/随附/已装三字段——已装=Mach-O 嵌入式元数据直读；
// 正交于健康五态，不进 wire）。
pub mod service_revision;
// P4 v2：ServiceControl install/uninstall/start 的管理员提权编排（守卫例外文件，
// 机制与选型见该模块头）。
pub(crate) mod service_lifecycle;
pub(crate) mod stats;
pub(crate) mod ui_runner;

pub use config::{DarwinConfigError, DarwinConfigItem, DarwinUiConfig, Protocol, AtrustAuthMode};
pub use credential::{
    SECRET_PAYLOAD_VERSION, ATRUST_SECRET_PAYLOAD_VERSION, ATRUST_PLATFORM_IDENTITY, ATRUST_RESOURCE_REQUEST, SavedConnectEnvelopeError, UiCredentialPackage, UiSecretPayload, UiAtrustCredentialPackage, UiConnectMaterials,
    UiCredentialPayloadError, build_connect_envelope, build_saved_connect_envelope,
    parse_ui_secret_payload,
};
pub use engine_client::{ObserveEndpoint, ObserveError, observe_owned_state};
pub use engine_lifecycle_real::EngineLifecycleError;
pub use ui_runner::UiCoreRunnerError;

/// Core 二进制入口调用的唯一库接缝。
///
/// # Errors
///
/// 固定 argv、进程身份、runtime identity、stdin bootstrap 或唯一 authenticated UDS session
/// 任一阶段失败时返回不含路径或认证材料的稳定 [`UiCoreRunnerError`]。
pub fn run() -> Result<(), UiCoreRunnerError> {
    ui_runner::run_from_process()
}

#[path = "../../../build_identity.rs"]
pub(crate) mod build_identity;
