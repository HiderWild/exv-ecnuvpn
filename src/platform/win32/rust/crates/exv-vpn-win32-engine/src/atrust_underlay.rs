//! Windows aTrust 物理出口适配；不操作系统路由、DNS 或代理设置。

use exv_vpn_atrust::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay, UnderlayError,
    UnderlayErrorKind,
};
use exv_vpn_cstp::connector::{GatewayResolver, SocketBinder};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

/// Windows实验的一次DataNode借用通知；诊断失败由调用方记录，不返回连接错误。
pub type DataNodeConnectedObserver = Arc<dyn Fn(&tokio::net::TcpStream) + Send + Sync>;

/// 在一次连接选择的物理接口上解析和拨号。调用方仍负责安装节点旁路路由。
/// 当前不提供系统默认解析或代理地址回退；TLS 由上层执行。
pub struct PhysicalUnderlay {
    resolver: Arc<GatewayResolver>,
    binder: Arc<SocketBinder>,
    node_nodelay: Option<bool>,
    actual_node_nodelay: AtomicU8,
    node_connected_observer: Option<DataNodeConnectedObserver>,
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
            node_nodelay: None,
            actual_node_nodelay: AtomicU8::new(0),
            node_connected_observer: None,
        })
    }

    /// 正式配置与实验共用的入口；只设置数据节点 socket，true/false 均显式设置并读回。
    /// 认证网关复核及未指定选项的旧实验可继续用 `new()` 保留宿主默认行为。
    pub fn new_with_nodelay(ifindex: u32, nodelay: bool) -> Result<Self, &'static str> {
        let mut underlay = Self::new(ifindex)?;
        underlay.node_nodelay = Some(nodelay);
        Ok(underlay)
    }

    /// 实验显式选用；hook只在实际DataNode装箱前通知一次，不参与认证网关。
    pub fn new_with_node_observer(
        ifindex: u32,
        nodelay: bool,
        observer: DataNodeConnectedObserver,
    ) -> Result<Self, &'static str> {
        let mut underlay = Self::new_with_nodelay(ifindex, nodelay)?;
        underlay.node_connected_observer = Some(observer);
        Ok(underlay)
    }

    /// 最近一次成功创建的数据节点 socket 的实际 NODELAY；建连前为 None。
    pub fn actual_node_nodelay(&self) -> Option<bool> {
        match self.actual_node_nodelay.load(Ordering::Acquire) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
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
                if purpose == ConnectPurpose::DataNode {
                    if let Some(requested) = self.node_nodelay {
                        stream
                            .set_nodelay(requested)
                            .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?;
                        let actual = stream
                            .nodelay()
                            .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?;
                        if actual != requested {
                            return Err(failed(UnderlayErrorKind::ConnectFailed));
                        }
                        self.actual_node_nodelay
                            .store(if actual { 2 } else { 1 }, Ordering::Release);
                    }
                    if let Some(observer) = &self.node_connected_observer {
                        observer(&stream);
                    }
                }
                Ok(Box::new(stream) as Box<dyn exv_vpn_atrust::transport::UnderlayStream>)
            })
            .await
            .map_err(|_| failed(UnderlayErrorKind::ConnectFailed))?
        })
    }
}
