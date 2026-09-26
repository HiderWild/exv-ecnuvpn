
//! Win32 特权 helper 平台 crate（构建接缝，空模块占位）。
//!
//! 本 crate 当前仅为 `vpn-rust-native-runtime-mvp` acceptance 提供空构建接缝，
//! 不含任何产品行为。实际实现由各叶 worker 在对应模块中填充。

#[path = "../../../build_identity.rs"]
pub mod build_identity;
pub mod composition;
pub mod data_plane;
pub mod grpc_server;
pub mod grpc_transport;
pub mod heartbeat;
pub mod log_sink;
pub mod mutation_ingress;
pub mod owner_lease;
pub mod packet_relay;
pub mod platform_tunnel;
pub mod secret_payload;
pub mod service;
pub mod service_batch;
pub mod shutdown;
pub mod stats;
pub mod status;
pub mod system_proxy_journal;
pub mod token_slot;
pub mod tunnel_runtime;
pub mod vgdc_connect;
pub mod wintun_dependency;

