//! aTrust 节点 TLS；学校预设携带已批准的精确节点证书，其他网关使用平台信任。

use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::{ConnectPurpose, OutboundTarget, Underlay};
use rustls_platform_verifier::BuilderVerifierExt;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

/// 已批准的华师大节点信任随 Engine 分发，不要求用户手工复制诊断文件。
/// 仅用于固定学校 HTTPS 网关；节点、端口、名称和叶证书约束仍由 configuration 校验。
pub(crate) fn bundled_certificate_for_gateway(gateway: &str) -> Option<&'static [u8]> {
    let origin = GatewayOrigin::parse(gateway).ok()?;
    (origin.host() == "vpn.ecnu.edu.cn" && origin.port() == 443)
        .then_some(include_bytes!("../resources/ecnu-atrust-node.der").as_slice())
}

fn observed_matches(der: &[u8]) -> bool {
    let expected = [
        0x35, 0xac, 0x42, 0xa3, 0x84, 0x75, 0x7c, 0x51, 0xc0, 0x92, 0x44, 0xb0, 0xc2, 0x18, 0x5d,
        0xcd, 0x2b, 0xb8, 0x65, 0xdc, 0xa1, 0x89, 0x8f, 0xfa, 0x8f, 0x9c, 0xc6, 0xc4, 0xf8, 0xfb,
        0x0d, 0x0c,
    ];
    Sha256::digest(der)[..] == expected
}

pub(crate) fn observed_node_matches(host: &str, port: u16, der: &[u8]) -> bool {
    host == "222.66.117.110" && port == 441 && observed_matches(der)
}

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
/// observed_certificate 只能显式传入已获授权的诊断证书；不会更改系统信任。
pub async fn connect_node(
    underlay: &dyn Underlay,
    node: &OutboundTarget,
    server_name: &str,
    observed_certificate: Option<&[u8]>,
) -> Result<
    tokio_rustls::client::TlsStream<Box<dyn exv_vpn_atrust::transport::UnderlayStream>>,
    String,
> {
    let config = configuration(node, server_name, observed_certificate)?;
    let name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
        .map_err(|_| "node-tls-name")?;
    let stream = underlay
        .connect(node)
        .await
        .map_err(|e| format!("node-underlay: {e}"))?;
    let stream = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream),
    )
    .await
    .map_err(|_| "node-tls-timeout")?
    .map_err(|_| "node-tls-verification-failed")?;
    if observed_certificate.is_some()
        && !stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .is_some_and(|cert| observed_matches(cert.as_ref()))
    {
        return Err("node-peer-certificate-mismatch".into());
    }
    Ok(stream)
}
