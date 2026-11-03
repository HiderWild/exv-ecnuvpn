//! Windows aTrust 物理出口适配；不操作系统路由、DNS 或代理设置。

use exv_vpn_atrust::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay, UnderlayError,
    UnderlayErrorKind,
};
use exv_vpn_cstp::connector::{GatewayResolver, SocketBinder};
use std::{sync::Arc, time::Duration};

/// 在一次连接选择的物理接口上解析和拨号。调用方仍负责安装节点旁路路由。
/// 当前不提供系统默认解析或代理地址回退；TLS 由上层执行。
pub struct PhysicalUnderlay {
    resolver: Arc<GatewayResolver>,
    binder: Arc<SocketBinder>,
}

impl PhysicalUnderlay {
    /// ifindex 必须来自本次连接的物理接口选择，解析和拨号共用该接口。
    pub fn new(ifindex: u32) -> Result<Self, &'static str> {
        if ifindex == 0 {
            return Err("aTrust physical interface missing");
        }
        Ok(Self {
            resolver: crate::vgdc_connect::production_gateway_resolver(ifindex, 443)
                .map_err(|_| "aTrust physical resolver unavailable")?,
            binder: crate::vgdc_connect::production_socket_binder(ifindex)
                .ok_or("aTrust physical binder unavailable")?,
        })
    }
}

impl Underlay for PhysicalUnderlay {
    // trait 的 resolve 不携带用途，独立解析的错误暂归认证网关。
    // 认证和节点建连应使用 connect；该路径保留 target.purpose，不经此方法。
    fn resolve(&self, host: &str, _port: u16) -> ResolveFuture<'_> {
        let resolution = (self.resolver)(host);
        Box::pin(async move {
            let failed = || {
                UnderlayError::new(
                    UnderlayErrorKind::ResolveFailed,
                    ConnectPurpose::AuthGateway,
                )
            };
            let address = tokio::time::timeout(Duration::from_secs(15), resolution)
                .await
                .map_err(|_| failed())?
                .map_err(|_| failed())?;
            Ok(vec![address.ip()])
        })
    }

    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        let resolution = (self.resolver)(&target.host);
        let port = target.port;
        let purpose = target.purpose;
        Box::pin(async move {
            let failed = |kind| UnderlayError::new(kind, purpose);
            tokio::time::timeout(Duration::from_secs(15), async {
                let mut address = resolution
                    .await
                    .map_err(|_| failed(UnderlayErrorKind::ResolveFailed))?;
                address.set_port(port);
                let socket = tokio::net::TcpSocket::new_v4()
                    .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?;
                (self.binder)(&socket).map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?;
                let stream = socket
                    .connect(address)
                    .await
                    .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?;
                Ok(Box::new(stream) as Box<dyn exv_vpn_atrust::transport::UnderlayStream>)
            })
            .await
            .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?
        })
    }
}
