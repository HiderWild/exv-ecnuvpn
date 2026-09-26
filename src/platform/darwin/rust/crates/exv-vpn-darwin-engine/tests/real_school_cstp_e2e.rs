//! `MAC-CSTP-05` 真机学校验收（opt-in）：用当前用户已保存的 Windows 同构密文凭据，
//! 对真实学校网关执行 WebVPN 登录与 CSTP 协商，取得真实 [`TunnelOffer`]。
//!
//! 范围纪律：本测试不创建 utun、不改路由/DNS/接口；断言本机 utun 接口数量
//! 在测试前后不变（零平台 mutation 的可观察证据）。
//!
//! 运行条件：`~/.exv/` 已保存完整可连接配置（server、username、已加密密码），
//! 且当前宿主可达学校网关。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_REAL_SCHOOL_CSTP=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-engine \
//!   --test real_school_cstp_e2e -- --ignored --nocapture
//! ```
//!
//! 本测试绝不打印用户名或密码；offer 字段（地址/前缀/MTU/DNS 数量）可以打印。

use std::sync::{Arc, Mutex};

use exv_vpn_cstp::session::TunnelOffer;
use exv_vpn_darwin_core::{DarwinUiConfig, build_saved_connect_envelope};
use exv_vpn_darwin_engine::protocol::establish;
use exv_vpn_wire::generated as wire;
use zeroize::Zeroize;

/// 统计当前 utun 接口数量（零 mutation 的可观察证据）。
fn utun_count() -> usize {
    // SAFETY: 复用 physical_route 的公共探测入口，不重复实现。
    exv_vpn_darwin_engine::egress::physical_route::collect_interface_facts()
        .expect("interface probe must succeed on the development host")
        .len()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实学校网关与已保存凭据；必须以 EXV_DARWIN_REAL_SCHOOL_CSTP=1 显式启用"]
async fn real_school_login_produces_an_authoritative_offer() {
    assert!(
        std::env::var_os("EXV_DARWIN_REAL_SCHOOL_CSTP").is_some(),
        "该真机验收必须显式以 EXV_DARWIN_REAL_SCHOOL_CSTP=1 运行"
    );

    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(
        !config.username().is_empty(),
        "缺少已保存用户名，请先在设置页保存"
    );
    let envelope =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    let server = envelope.server().to_owned();
    let user_agent = envelope.user_agent().to_owned();
    let (mut username, mut password) = envelope.take_credentials();

    let utun_before = utun_count();
    let phases: Arc<Mutex<Vec<wire::ConnectPhase>>> = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&phases);
    let established = establish(
        &server,
        username.as_bytes(),
        password.as_bytes(),
        &user_agent,
        || false,
        &move |phase| {
            observer.lock().expect("phase lock").push(phase);
        },
    )
    .await
    .expect("real school WebVPN login + CSTP negotiation must succeed");
    username.zeroize();
    password.zeroize();

    // 真实阶段流：ConnectingControl（登录）→ NegotiatingTunnel（CSTP offer）。
    assert_eq!(
        *phases.lock().expect("phase lock"),
        vec![
            wire::ConnectPhase::ConnectingControl,
            wire::ConnectPhase::NegotiatingTunnel,
        ]
    );

    let TunnelOffer {
        ipv4_address,
        prefix,
        mtu,
        dns_servers,
        routes,
    } = established.offer.clone();
    assert!(
        !ipv4_address.is_unspecified(),
        "offer must carry a real IPv4 address"
    );
    assert!(
        (1..=32).contains(&prefix),
        "offer prefix must be a valid IPv4 prefix"
    );
    assert!(
        (576..=1500).contains(&mtu),
        "offer MTU must be in the MVP range"
    );
    assert!(
        !dns_servers.is_empty() || !routes.is_empty(),
        "a real school offer carries DNS servers or campus routes"
    );
    println!(
        "real offer: address={ipv4_address} prefix={prefix} mtu={mtu} dns={} routes={}",
        dns_servers.len(),
        routes.len()
    );

    // 释放会话：drop 关闭 CSTP TLS 通道。
    drop(established);

    // 零平台 mutation：utun 接口数量不变。
    assert_eq!(
        utun_count(),
        utun_before,
        "CSTP-05 must not create utun interfaces"
    );
}
