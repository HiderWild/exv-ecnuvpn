//! aTrust 节点 TLS（M3 darwin aTrust：设计 §6.2，踩坑 4 对策；win32
//! `atrust_node_tls.rs` 的 darwin 等价实现——照搬语义，不照搬机制之外的东西）。
//!
//! **三重作用域**（与 win32 逐字对齐）：
//! 1. 仅 `vpn.ecnu.edu.cn:443` 网关 → 安装内置 DER（校级预设随 Engine 分发，
//!    不要求用户手工复制诊断证书）；
//! 2. 仅 `222.66.117.110:441` 节点 + observed 证书指纹匹配（SHA256
//!    `35AC42A3…B0D0C`）→ 以内置证书为根；
//! 3. 其余网关/节点 → 平台验证器（rustls-platform-verifier，等价既有路径）。
//!
//! SNI 恒 `sdp.ssl.com`（节点名义名，与真实主机无关）；observed 路径在握手后
//! 复核叶指纹（防作用域降级）。**无自动轮换**（CERT-07 挂账——证书更换需随
//! 产品版本更新内置 DER，测试锁指纹防静默漂移）。
//!
//! 返回前不发送 SID 或流授权材料（先经宿主出口建连，再进行名称/证书验证）。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::transport::UnderlayStream;
use exv_vpn_atrust::{ConnectPurpose, OutboundTarget, Underlay};
use rustls_platform_verifier::BuilderVerifierExt;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

/// 已批准的华师大节点信任随 Engine 分发（与 win32 resources 字节恒等——拷贝
/// 来源 `ecv-vpn-win32-engine/resources/ecnu-atrust-node.der`，测试锁定指纹与
/// 字节恒等）。仅用于固定学校 HTTPS 网关；节点、端口、名称和叶证书约束仍由
/// [`Self::configuration`] 校验。
pub(crate) fn bundled_certificate_for_gateway(gateway: &str) -> Option<&'static [u8]> {
    let origin = GatewayOrigin::parse(gateway).ok()?;
    (origin.host() == "vpn.ecnu.edu.cn" && origin.port() == 443)
        .then_some(include_bytes!("../resources/ecnu-atrust-node.der").as_slice())
}

/// observed 叶证书期望指纹（SHA256；win32 同值，证书公共材料见
/// `docs/research/atrust/t06-node-entry-tls-evidence.md`）。
const OBSERVED_LEAF_SHA256: [u8; 32] = [
    0x35, 0xac, 0x42, 0xa3, 0x84, 0x75, 0x7c, 0x51, 0xc0, 0x92, 0x44, 0xb0, 0xc2, 0x18, 0x5d,
    0xcd, 0x2b, 0xb8, 0x65, 0xdc, 0xa1, 0x89, 0x8f, 0xfa, 0x8f, 0x9c, 0xc6, 0xc4, 0xf8, 0xfb,
    0x0d, 0x0c,
];

fn observed_matches(der: &[u8]) -> bool {
    Sha256::digest(der)[..] == OBSERVED_LEAF_SHA256
}

/// observed 证书的作用域判定：仅 `222.66.117.110:441` + 指纹匹配。
pub(crate) fn observed_node_matches(host: &str, port: u16, der: &[u8]) -> bool {
    host == "222.66.117.110" && port == 441 && observed_matches(der)
}

/// 按三重作用域构造节点 TLS 配置；observed 路径要求主机/端口/名称三重匹配。
fn configuration(
    node: &OutboundTarget,
    name: &str,
    observed: Option<&[u8]>,
) -> Result<rustls::ClientConfig, &'static str> {
    if node.purpose != ConnectPurpose::DataNode {
        return Err("not-a-data-node");
    }
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "node-tls-config")?;
    match observed {
        Some(der) => {
            if !observed_node_matches(&node.host, node.port, der) || name != "sdp.ssl.com" {
                return Err("observed-certificate-scope-mismatch");
            }
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(rustls::pki_types::CertificateDer::from(der.to_vec()))
                .map_err(|_| "node-certificate-invalid")?;
            Ok(builder.with_root_certificates(roots).with_no_client_auth())
        }
        None => Ok(builder
            .with_platform_verifier()
            .map_err(|_| "node-platform-verifier")?
            .with_no_client_auth()),
    }
}

/// 先经宿主出口建连，再进行名称/证书验证。返回前不发送 SID 或流授权材料。
/// `observed_certificate` 只能是 [`bundled_certificate_for_gateway`] 的内置证书
/// （win32 另有显式诊断材料入口，darwin M3 不迁移该入口——作用域一致）；
/// 不会更改系统信任。
///
/// # Errors
///
/// 返回稳定错误类别字符串（win32 同词表）；不含 SID、节点载荷或证书材料。
pub(crate) async fn connect_node(
    underlay: &dyn Underlay,
    node: &OutboundTarget,
    server_name: &str,
    observed_certificate: Option<&[u8]>,
) -> Result<tokio_rustls::client::TlsStream<Box<dyn UnderlayStream>>, String> {
    let config = configuration(node, server_name, observed_certificate)?;
    let name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
        .map_err(|_| "node-tls-name".to_owned())?;
    let stream = underlay
        .connect(node)
        .await
        .map_err(|e| format!("node-underlay: {e}"))?;
    let stream = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream),
    )
    .await
    .map_err(|_| "node-tls-timeout".to_owned())?
    .map_err(|_| "node-tls-verification-failed".to_owned())?;
    if observed_certificate.is_some()
        && !stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .is_some_and(|cert| observed_matches(cert.as_ref()))
    {
        return Err("node-peer-certificate-mismatch".to_owned());
    }
    Ok(stream)
}
