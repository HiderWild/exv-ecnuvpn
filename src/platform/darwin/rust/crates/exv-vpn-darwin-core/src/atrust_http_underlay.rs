//! 连接前认证的宿主普通直连端口；无 HTTP 代理，不修改宿主路由或 DNS。
//!
//! 只用于非特权宿主的会话探针和 CAS，不反向依赖 Engine。普通 TCP 不保证绕过
//! 外部 TUN；Engine 原生物理出口仍由其自身端口负责。

use exv_vpn_atrust::transport::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay, UnderlayStream,
};
use exv_vpn_atrust::{UnderlayError, UnderlayErrorKind};

pub struct HostAuthUnderlay;

impl Underlay for HostAuthUnderlay {
    fn resolve(&self, host: &str, port: u16) -> ResolveFuture<'_> {
        let host = host.to_owned();
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map(|addresses| addresses.map(|address| address.ip()).collect())
                .map_err(|error| {
                    UnderlayError::from_io(
                        UnderlayErrorKind::ResolveFailed,
                        ConnectPurpose::AuthGateway,
                        "resolve",
                        &error,
                    )
                })
        })
    }

    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        let host = target.host.clone();
        let port = target.port;
        let purpose = target.purpose;
        Box::pin(async move {
            tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map(|stream| Box::new(stream) as Box<dyn UnderlayStream>)
                .map_err(|error| {
                    UnderlayError::from_io(
                        UnderlayErrorKind::ConnectFailed,
                        purpose,
                        "connect",
                        &error,
                    )
                })
        })
    }
}
