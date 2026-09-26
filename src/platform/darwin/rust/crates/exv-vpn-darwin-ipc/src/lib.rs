//! Darwin 本地控制面 IPC 的库边界。
//!
//! 当前阶段不创建 socket、不读取 peer credential，也不定义任何 packet 协议。后续
//! `MAC-IPC-01` 才能在本 crate 中接入 UI→Core 和 Core→Engine 的已认证 UDS。

pub mod auth;
pub mod bootstrap;
pub mod connect_envelope;
pub mod listener;
pub mod path;
pub mod peer;
pub mod service_listener;
pub mod service_revision;
pub mod tonic_bridge;
pub mod ui_core_bootstrap;
pub mod warm_connector;

pub use tonic_bridge::{
    AuthenticatedConnectionCloseSignal, AuthenticatedIncoming, AuthenticatedIncomingSender,
    DarwinConnectInfo,
};
pub use warm_connector::{authenticate_client_to_tonic_channel, connect_service_authenticated_channel};
