
//! CSTP session phase (CS-AUTH-02-I): `CONNECT /CSCOSSLC/tunnel` + webvpn
//! cookie + data session.
//!
//! [`CstpSession::open`] runs the full chain over a real
//! [`Bootstrap`](crate::connector::Bootstrap) TLS connection under the injected
//! trust policy: it writes the byte-exact CONNECT request (plan §3.2 verbatim
//! block; openconnect `cstp.c` `start_cstp_connection()` anchors — the tag pin
//! and per-line verification are recorded at the leaf commit, per plan §3
//! provenance discipline), reads the offer up to its `\r\n\r\n` terminator
//! (early EOF is the typed [`SessionError::EofBeforeTerminator`], never a
//! success), validates it into a [`TunnelOffer`] plan (engine version of the
//! school `validate_school_offer` semantics, plan §3.3; an HTTP error status /
//! non-CSTP content is [`SessionError::OfferParseFailed`]), splits the stream,
//! and hands the two halves to two data tasks (engine version of the school
//! `spawn_school_data_tasks` pattern) so the returned session carries working
//! CSTP data channels: frames sent on `write_channel` go on the wire raw, and
//! gateway frames arrive decoded as `CstpFrame::Data` payloads on
//! `read_channel`.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::codec::{Codec, CodecError, CstpFrame};
use crate::connector::{Bootstrap, BootstrapConfig, BootstrapError};
use crate::webvpn::{LoginSession, build_connect_request, build_connect_request_with_user_agent};

/// Upper bound on the offer head before it is rejected as non-CSTP content.
const MAX_OFFER: usize = 16 * 1024;

/// The validated CSTP tunnel offer plan (engine version of the school
/// `validate_school_offer` semantics, plan §3.3).
///
/// `Serialize` only, never `Deserialize` (C01 discipline: a safe business
/// representation, not a live runtime).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelOffer {
    /// The gateway-assigned IPv4 address (`X-CSTP-Address`).
    pub ipv4_address: Ipv4Addr,
    /// The prefix length derived from `X-CSTP-Netmask` (contiguous masks only).
    pub prefix: u8,
    /// The negotiated tunnel MTU (68..=65535).
    pub mtu: u16,
    /// The gateway-assigned DNS servers (`X-CSTP-DNS`, whitespace-separated).
    pub dns_servers: Vec<Ipv4Addr>,
    /// The split-include routes as `"net/prefix"` CIDR strings (school-aligned).
    pub routes: Vec<String>,
}

// ---------------------------------------------------------------------------
// T1 latency probe seam（单侧宿主豁免政策：文件内注释位置/目的/必要性）。
// ---------------------------------------------------------------------------
// 位置：DPD wire 判别常量 + 控制面事件类型放在 session.rs（而非 codec.rs）——engine
// 延迟探测（T1）需要向网关注入 DPD request（上传方向）并感知 DPD response 到达；
// codec.rs 在 T1 的文件边界之外，故常量与事件类型随消费它的控制面出口同居本文件。
// 必要性：codec 只解码 keepalive（0x07）控制帧，DPD 0x03/0x04 走 `UnknownControl`
// 错误路径返回——若不在此转义，读任务会在首个 DPD 响应帧处死亡（会话断链）。
// 目的：把控制面事件（含 DPD response 探测信号）透出给 engine 的延迟探测循环，
// 且保持读任务对未知控制帧的存活（健壮性提升，非 T1 之前的行为破坏）。

/// CSTP wire packet type for a DPD (dead-peer-detection) request (AnyConnect 0x03).
///
/// Defined here rather than in `codec.rs` by the single-host exemption policy (see
/// the module note above): the engine latency probe (T1) sends DPD requests on the
/// upload path and needs the wire discriminant; the codec is outside T1's file
/// boundary. The codec does not decode 0x03/0x04 — they surface as
/// `CodecError::UnknownControl`, and this module maps the response to the probe
/// signal (see [`CstpControlEvent::DpdResponse`]).
pub const CSTP_PACKET_TYPE_DPD_REQUEST: u8 = 0x03;

/// CSTP wire packet type for a DPD response (AnyConnect 0x04) — the RTT probe signal.
pub const CSTP_PACKET_TYPE_DPD_RESPONSE: u8 = 0x04;

/// 读任务退出原因（F4 契约：读任务退出前经 `control_tx` 发出的最后一条
/// `CstpControlEvent::SessionEnded` 携带）。修复前 `Ok(0) | Err(_) => break`
/// 把网关 clean close 与 io 错误合并、codec 真错误与消费端先掉均静默 return，
/// 宿主无法区分掉线原因——本枚举把四个退出点全部可观测化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEndReason {
    /// 服务器协议层终止，正文在 TLS 读取边界脱敏并限制长度。
    ServerDisconnect { kind: u8, code: Option<u8>, reason: String, body_len: usize },
    /// 读到 `Ok(0)`：网关 clean close（典型 = 网关 DPD 超时单方面断开会话）。
    GatewayClosedStream,
    /// TLS 读错误：保留方向、错误类别、原始系统码与安全诊断详情。
    Io(SessionIoError),
    /// 真 codec 流错误（修复前 `session.rs` 读任务静默 `return` 的路径）。
    Codec(CodecError),
    /// `read_tx` 消费端先掉（修复前静默 `return` 的路径）：数据面已无人收数据帧，
    /// 读任务继续解码只会空转。
    ConsumerDropped,
}

// Windows 宿主直修 Common：位置为错误映射与 TLS task 的诊断出口；目的为保留
// 已建立会话的真实 I/O 失败和写入结果；必要性为宿主拿到 ErrorKind/队列后已无法
// 还原 raw_os_error、rustls 错误或 write_all 完成状态。本改动不决定其他宿主验收。
/// TLS 操作方向，避免与宿主 ring reader/writer 的命名混淆。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoDirection {
    Read,
    Write,
}

/// 有界脱敏的 I/O 诊断；不持有 payload、凭据或原始错误对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIoError {
    /// 原始 I/O 错误捕获时刻，UNIX 毫秒；区别于宿主稍后收到事件的时刻。
    pub observed_ms: u64,
    pub direction: IoDirection,
    pub kind: std::io::ErrorKind,
    pub raw_os_error: Option<i32>,
    pub detail: String,
}

impl SessionIoError {
    #[must_use]
    pub fn new(direction: IoDirection, error: &std::io::Error) -> Self {
        let observed_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        // TLS typed error 保留分类与 alert 枚举；自由文本必须脱敏且有界，
        // OS 消息重新由原始错误码生成，避免混入自定义上下文。
        let detail = if let Some(code) = error.raw_os_error() {
            std::io::Error::from_raw_os_error(code).to_string()
        } else if let Some(tls) = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            match tls {
                rustls::Error::AlertReceived(alert) => format!("rustls: AlertReceived({alert:?})"),
                rustls::Error::DecryptError => "rustls: DecryptError".into(),
                rustls::Error::EncryptError => "rustls: EncryptError".into(),
                rustls::Error::InvalidMessage(detail) => {
                    format!("rustls: InvalidMessage({detail:?})")
                }
                rustls::Error::PeerMisbehaved(_) => "rustls: PeerMisbehaved".into(),
                rustls::Error::PeerIncompatible(_) => "rustls: PeerIncompatible".into(),
                rustls::Error::InvalidCertificate(_) => "rustls: InvalidCertificate".into(),
                rustls::Error::InappropriateMessage { .. } => "rustls: InappropriateMessage".into(),
                rustls::Error::InappropriateHandshakeMessage { .. } => {
                    "rustls: InappropriateHandshakeMessage".into()
                }
                rustls::Error::PeerSentOversizedRecord => "rustls: PeerSentOversizedRecord".into(),
                rustls::Error::HandshakeNotComplete => "rustls: HandshakeNotComplete".into(),
                rustls::Error::General(detail) => {
                    format!("rustls: General: {}", redact_io_detail(detail))
                }
                _ => "rustls: other error (detail redacted)".into(),
            }
        } else {
            redact_io_detail(&error.to_string())
        };
        // 上限按 UTF-8 字节计，过滤控制字符以避免多行日志或终端控制序列。
        let mut bounded = String::new();
        for ch in detail.chars().filter(|ch| !ch.is_control()) {
            if bounded.len() + ch.len_utf8() > 256 {
                break;
            }
            bounded.push(ch);
        }
        Self {
            observed_ms,
            direction,
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
            detail: bounded,
        }
    }
}

/// 保留现场错误上下文；遇到凭据字段从该位置截断，URL/带值键整体替换。
/// 不能把无法识别的自定义故障重新压成 ErrorKind；也不输出 URL 查询或 Cookie 值。
fn redact_io_detail(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let sensitive_start = [
        "cookie",
        "authorization",
        "password",
        "passwd",
        "credential",
        "token",
        "secret",
        "webvpn",
        "bearer",
        "username",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min();
    let prefix = &text[..sensitive_start.unwrap_or(text.len())];
    let mut result = prefix
        .split_whitespace()
        .map(|word| {
            if word.contains("://") {
                "[url redacted]"
            } else if word.contains('=') {
                "[value redacted]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if sensitive_start.is_some() {
        result.push_str(" [sensitive detail redacted]");
    }
    result
}

/// 单个 TLS task 的累计采样。写入计数仅在完整帧写入并 flush 成功后更新，
/// 不表示对端已处理该帧。
/// 读操作次数为 TLS read 批次；写操作次数为完整 CSTP 帧数；均不含 payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIoDiagnostics {
    pub direction: IoDirection,
    pub bytes: u64,
    pub operations: u64,
    pub keepalive_frames: u64,
    pub dpd_request_frames: u64,
    pub dpd_response_frames: u64,
    pub last_activity: Option<Instant>,
    pub sampled_at: Instant,
}

impl SessionIoDiagnostics {
    fn new(direction: IoDirection) -> Self {
        Self {
            direction,
            bytes: 0,
            operations: 0,
            keepalive_frames: 0,
            dpd_request_frames: 0,
            dpd_response_frames: 0,
            last_activity: None,
            sampled_at: Instant::now(),
        }
    }

    fn record(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes as u64);
        self.operations = self.operations.saturating_add(1);
        self.last_activity = Some(Instant::now());
    }

    fn publish(&mut self, tx: &mpsc::UnboundedSender<CstpControlEvent>) {
        self.sampled_at = Instant::now();
        let _ = tx.send(CstpControlEvent::IoDiagnostics(self.clone()));
    }
}

/// A CSTP control-plane event surfaced from the data session's read path.
///
/// The codec decodes keepalive control frames into [`CstpFrame::Control`]; DPD
/// responses (wire kind 0x04) surface as `UnknownControl` codec errors and are
/// mapped here to the dedicated probe signal so the engine can time dead-peer
/// detection RTT without the read task dying on an unknown control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstpControlEvent {
    /// A decoded control frame (e.g. keepalive) with its wire kind and body.
    Control { kind: u8, body: Vec<u8> },
    /// A DPD response frame (wire kind 0x04) was received.
    DpdResponse,
    /// 读任务退出（会话终结）：携带退出原因。这是读任务经 `control_tx` 发出的
    /// **最后一条**事件；宿主据此可区分「网关 clean close / io 错误 / codec 错误 /
    /// 消费端先掉」并感知会话已不可恢复。
    SessionEnded(SessionEndReason),
    /// TLS 写失败：诊断事件，不替宿主决定掉线或重连策略。
    WriteFailed(SessionIoError),
    /// 低频累计 I/O 采样与任务退出快照，无逐帧日志。
    IoDiagnostics(SessionIoDiagnostics),
}

/// An established CSTP data session (the outcome of a successful CONNECT
/// phase). Debug never leaks the session cookie (it holds none).
#[derive(Debug)]
pub struct CstpSession {
    /// The validated offer plan (address/prefix/mtu/dns/routes).
    pub offer_plan: TunnelOffer,
    /// SHA-256 hex digest of the peer certificate, when the handshake exposed
    /// one (never the raw certificate).
    pub peer_fingerprint: Option<String>,
    /// Raw on-wire CSTP frames to send (Codec-encoded by the caller).
    pub write_channel: mpsc::UnboundedSender<Vec<u8>>,
    /// Decoded `CstpFrame::Data` payloads received from the gateway.
    pub read_channel: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Control-plane events (keepalive frames + the DPD-response probe signal) that
    /// arrive on the data session's read path. Consumed by the engine's latency
    /// probe; a dropped consumer is non-fatal (control events are best-effort).
    pub control_rx: mpsc::UnboundedReceiver<CstpControlEvent>,
}

/// Typed failure of the CSTP CONNECT/session phase (plan §2.2).
#[allow(
    clippy::module_name_repetitions,
    reason = "API fixed verbatim by the CS-AUTH-02-T test contract (tests/webvpn_connect.rs)"
)]
#[derive(Debug)]
pub enum SessionError {
    /// The TLS bootstrap rejected the peer (untrusted chain / hostname
    /// mismatch / handshake / connect / IPv6 target / DTLS offer).
    TlsFailed(BootstrapError),
    /// Writing the CONNECT request to the gateway failed.
    WriteFailed,
    /// The connection ended (EOF) before the offer's `\r\n\r\n` terminator:
    /// the gateway rejected the request format.
    EofBeforeTerminator,
    /// The offer could not be parsed into a valid tunnel plan (HTTP error
    /// status / non-CSTP content / missing required field).
    OfferParseFailed,
    /// `open` was called without a login session: a cookie-less CONNECT is
    /// rejected BEFORE any network activity.
    MissingSession,
}

impl CstpSession {
    /// Establish the CSTP session over a real TLS connection:
    /// Bootstrap(TrustPolicy from `cfg`) -> byte-exact CONNECT request
    /// (plan §3.2, `build_connect_request` bytes) -> offer read (HTTP-head
    /// style until `\r\n\r\n`; early EOF is [`SessionError::EofBeforeTerminator`])
    /// -> validated [`TunnelOffer`] plan -> stream split -> two data tasks.
    ///
    /// The webvpn cookie captured by the committed CS-AUTH-01 login flows into
    /// the CONNECT request verbatim and IS the credential: no `Authorization`
    /// and no `X-AnyConnect-*` header is ever written.
    ///
    /// # Errors
    ///
    /// * [`SessionError::MissingSession`] — `login` is `None` (no network
    ///   activity happens).
    /// * [`SessionError::TlsFailed`] — the bootstrap rejected the peer.
    /// * [`SessionError::WriteFailed`] — the CONNECT request could not be
    ///   written.
    /// * [`SessionError::EofBeforeTerminator`] — EOF before the offer's
    ///   `\r\n\r\n` terminator.
    /// * [`SessionError::OfferParseFailed`] — non-CSTP offer content (HTTP
    ///   error status / missing required field).
    pub async fn open(
        cfg: BootstrapConfig,
        login: Option<&LoginSession>,
    ) -> Result<CstpSession, SessionError> {
        Self::open_with_user_agent(cfg, login, None).await
    }

    /// Establish the CSTP session with an explicit client `User-Agent` on the
    /// CONNECT request — the additive per-connection override of the login
    /// phase's client name ([`crate::webvpn::default_user_agent`] /
    /// [`WebvpnLogin::perform_login_with_user_agent`](crate::webvpn::WebvpnLogin::perform_login_with_user_agent);
    /// the C++ `make_cstp_connect_request` shape). The webvpn cookie is
    /// still the credential; the UA is the client identity the gateway logs.
    ///
    /// `user_agent: None` writes the legacy UA-less CONNECT (verified working
    /// on the school gateway); `Some` carries the given UA. The client name
    /// is never hardcoded into the flow: the caller passes it here, and a
    /// config/settings layer may substitute its own platform default.
    ///
    /// # Errors
    ///
    /// Same as [`CstpSession::open`].
    pub async fn open_with_user_agent(
        cfg: BootstrapConfig,
        login: Option<&LoginSession>,
        user_agent: Option<&str>,
    ) -> Result<CstpSession, SessionError> {
        Self::open_with_user_agent_observed(cfg, login, user_agent, None).await
    }

    /// Windows 宿主直修 Common 的小观测端口：成功校验 offer 后读取真实 TLS 底层
    /// socket；目的为证明实际 local/peer 与绑定出口。宿主只持通道时无法还原这些事实。
    /// 原入口委托 None，不改变 macOS 或现有调用方的连接/认证行为。
    pub async fn open_with_user_agent_observed(
        cfg: BootstrapConfig,
        login: Option<&LoginSession>,
        user_agent: Option<&str>,
        on_established: Option<&(dyn Fn(&tokio::net::TcpStream) + Send + Sync)>,
    ) -> Result<CstpSession, SessionError> {
        // A cookie-less CONNECT is a typed error BEFORE any network activity
        // (kills the "cookie missing still CONNECTs" mutant).
        let login = login.ok_or(SessionError::MissingSession)?;

        let host = cfg.hostname.clone();
        let bootstrap_session = Bootstrap::system()
            .connect(cfg)
            .await
            .map_err(|failure| SessionError::TlsFailed(failure.error))?;

        // The frozen bootstrap stays opaque; the TLS stream is handed over
        // through the additive `into_stream` accessor (connector.rs, §7
        // governance record). The peer certificate fingerprint is a SHA-256
        // digest, never the raw certificate.
        let mut stream = bootstrap_session.into_stream();
        let fingerprint = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first().cloned())
            .map(|cert| sha256_hex(cert.as_ref()));

        // --- CONNECT request: byte-exact plan §3.2 bytes; the webvpn cookie
        // is the credential (no Authorization, no X-AnyConnect-*). The
        // additive UA variant carries the caller's client name. ---
        let request = match user_agent {
            Some(user_agent) => build_connect_request_with_user_agent(&host, login, user_agent),
            None => build_connect_request(&host, login),
        };
        stream
            .write_all(&request)
            .await
            .map_err(|_| SessionError::WriteFailed)?;
        stream
            .flush()
            .await
            .map_err(|_| SessionError::WriteFailed)?;

        // --- Offer read: HTTP-head style up to the `\r\n\r\n` terminator.
        // EOF before the terminator = the gateway rejected the request format
        // (typed error, never a partial-offer success). ---
        let mut buf = Vec::new();
        let mut tmp = [0u8; 512];
        let head_end = loop {
            match stream.read(&mut tmp).await {
                // The gateway closed the connection before the offer's
                // `\r\n\r\n` terminator (a rejected request format): a clean
                // EOF (`Ok(0)`) and an abrupt close without close_notify
                // (read `Err`) are the same observed rejection signature and
                // surface as the typed EofBeforeTerminator — never a success.
                Ok(0) | Err(_) => return Err(SessionError::EofBeforeTerminator),
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = find_header_end(&buf) {
                        break end;
                    }
                    if buf.len() > MAX_OFFER {
                        return Err(SessionError::OfferParseFailed);
                    }
                }
            }
        };

        // Bytes past the terminator belong to the CSTP binary frame stream and
        // are handed to the read data task (never dropped).
        let remainder = buf.split_off(head_end);
        let head = String::from_utf8_lossy(&buf);
        let offer_plan = parse_offer(&head).ok_or(SessionError::OfferParseFailed)?;
        if let Some(observe) = on_established {
            observe(stream.get_ref().0);
        }

        // --- Split the stream; the two data tasks carry the binary framing
        // (engine version of the school spawn_school_data_tasks pattern) ---
        let (read_half, write_half) = tokio::io::split(stream);
        let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (read_tx, read_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // T1 latency probe seam: control events (keepalive + DPD-response probe
        // signal) ride a dedicated channel so the engine can time dead-peer RTT
        // without mixing control frames into the data payload stream.
        let (control_tx, control_rx) = mpsc::unbounded_channel::<CstpControlEvent>();
        spawn_data_tasks(
            read_half, write_half, read_tx, write_rx, remainder, control_tx,
        );

        Ok(CstpSession {
            offer_plan,
            peer_fingerprint: fingerprint,
            write_channel: write_tx,
            read_channel: read_rx,
            control_rx,
        })
    }
}

/// Parse an offer head (up to the `\r\n\r\n` terminator) into a validated
/// tunnel plan, or `None` when the content is not a valid CSTP offer (plan
/// §3.3 semantics; engine version of the school `validate_school_offer`).
///
/// Both line styles parse into the SAME plan (school L1326-1330): the HTTP
/// header style (`X-CSTP-Address: 10.88.88.1`, colon-split) and the legacy
/// line style (`CSTP_ADDRESS 10.88.88.1`, whitespace-split — the school
/// `validate_school_offer` branch at school.rs L1345-1352, 受控对齐保留).
///
/// HTTP wrapper headers are ignored; `X-CSTP-*` keys normalize to `CSTP_*`; the
/// required `CSTP_MTU` (68..=65535) / `CSTP_ADDRESS` (IPv4) / `CSTP_NETMASK`
/// (contiguous mask -> prefix 0..=32) must all be present; `CSTP_DNS` /
/// `CSTP_SPLIT_INCLUDE` items are validated individually; unknown `CSTP_*` /
/// `X-CSTP-*` keys are ignored (real-gateway extra fields); any `dtls`
/// appearance rejects the offer; a non-2xx HTTP status (rejection page) is
/// non-CSTP content.
fn parse_offer(head: &str) -> Option<TunnelOffer> {
    // The HTTP wrapper status must be 2xx: an error status is non-CSTP offer
    // content and never becomes a data session.
    let status = head.lines().next()?.split_whitespace().nth(1)?;
    let status: u16 = status.parse().ok()?;
    if !(200..=299).contains(&status) {
        return None;
    }

    let mut mtu: Option<u16> = None;
    let mut address: Option<Ipv4Addr> = None;
    let mut netmask: Option<Ipv4Addr> = None;
    let mut dns_servers: Vec<Ipv4Addr> = Vec::new();
    let mut routes: Vec<String> = Vec::new();
    for line in head.lines().skip(1) {
        // Key/value split, school `validate_school_offer` alignment (school.rs
        // L1345-1352): colon first (HTTP header style), else the first
        // whitespace (legacy line style); a keyless line is ignored, never
        // fatal.
        let (key, value) = match line.split_once(':') {
            // HTTP header style: `X-CSTP-Address: 10.88.88.1`.
            Some((key, value)) => (key, value.trim()),
            // Legacy line style: `CSTP_ADDRESS 10.88.88.1`（空白分隔，受控对齐
            // 保留）— the first whitespace token is the key, the rest of the
            // line is the value.
            None => {
                let Some(idx) = line.find(char::is_whitespace) else {
                    continue;
                };
                (line[..idx].trim(), line[idx..].trim())
            }
        };
        // `X-CSTP-Address` -> `CSTP_ADDRESS`（legacy 键原样；CS-AUTH-03 对齐）。
        let key = match key.strip_prefix("X-CSTP-") {
            Some(rest) => format!("CSTP_{}", rest.replace('-', "_")),
            None => key.to_string(),
        };
        match key.to_ascii_uppercase().as_str() {
            "CSTP_MTU" => {
                let v: u16 = value.parse().ok()?;
                if !(68..=u16::MAX).contains(&v) {
                    return None;
                }
                mtu = Some(v);
            }
            "CSTP_ADDRESS" => {
                address = Some(value.parse::<Ipv4Addr>().ok()?);
            }
            "CSTP_NETMASK" => {
                netmask = Some(value.parse::<Ipv4Addr>().ok()?);
            }
            "CSTP_DNS" => {
                for token in value.split_whitespace() {
                    dns_servers.push(token.parse::<Ipv4Addr>().ok()?);
                }
            }
            "CSTP_SPLIT_INCLUDE" => {
                for token in value.split_whitespace() {
                    let (net, prefix) = token.split_once('/')?;
                    net.parse::<Ipv4Addr>().ok()?;
                    let prefix: u8 = prefix.parse().ok()?;
                    if prefix > 32 {
                        return None;
                    }
                    routes.push(token.to_string());
                }
            }
            _ => {
                // Unknown keys (`CSTP_*` / `X-CSTP-*` extra fields of real
                // gateways) are ignored — still CSTP-only; non-CSTP content
                // fails the required-field checks below.
                continue;
            }
        }
    }
    // Any `dtls` appearance rejects the offer (plan §3.3: the MVP is
    // TLS/CSTP only).
    if head.to_ascii_lowercase().contains("dtls") {
        return None;
    }
    let prefix = netmask_to_prefix(netmask?)?;
    Some(TunnelOffer {
        ipv4_address: address?,
        prefix,
        mtu: mtu?,
        dns_servers,
        routes,
    })
}

/// A contiguous IPv4 netmask -> prefix length (a non-contiguous mask is
/// rejected; school `netmask_to_prefix` pattern).
fn netmask_to_prefix(mask: Ipv4Addr) -> Option<u8> {
    let mut prefix = 0u8;
    let mut seen_zero = false;
    for octet in mask.octets() {
        for bit in (0..8).rev() {
            if octet & (1 << bit) != 0 {
                if seen_zero {
                    return None;
                }
                prefix += 1;
            } else {
                seen_zero = true;
            }
        }
    }
    Some(prefix)
}

/// The byte offset just past the `\r\n\r\n` header terminator, if present.
fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

/// The lowercase hex SHA-256 digest of `bytes` (school `sha256_hex` pattern).
fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Map one codec decode outcome to a control-plane event (or `None` for data /
/// non-control outcomes).
///
/// DPD responses surface through the codec's `UnknownControl(0x04)` error path (the
/// codec only decodes keepalive control frames); this maps that error to the RTT
/// probe signal without killing the read task.
///
/// F4 未知控制帧收敛：**所有** `UnknownControl(kind)`（0x04 优先映射
/// `DpdResponse` 之外）一律上抛 `Control { kind, body: Vec::new() }`——读任务保持
/// 存活，宿主可见每一个未知 kind（特别是网关 DPD request 0x03：不回应会被网关在
/// ~60s 后单方面断开会话，macOS 宿主实测记录）。body 恒空：codec 的
/// `UnknownControl` 错误只携带 kind 字节、载荷已消费丢弃，而 wire 上 0x03 本无
/// 载荷，空 body 语义无损。
fn control_event_for_decode(
    outcome: &Result<Option<CstpFrame>, CodecError>,
) -> Option<CstpControlEvent> {
    match outcome {
        Ok(Some(CstpFrame::Control { kind, body })) => Some(CstpControlEvent::Control {
            kind: *kind,
            body: body.clone(),
        }),
        Err(CodecError::UnknownControl(kind)) if *kind == CSTP_PACKET_TYPE_DPD_RESPONSE => {
            Some(CstpControlEvent::DpdResponse)
        }
        Err(CodecError::UnknownControl(kind)) => Some(CstpControlEvent::Control {
            kind: *kind,
            body: Vec::new(),
        }),
        _ => None,
    }
}

/// 纯映射 seam（W5 可测；`spawn_data_tasks` 收具体 TLS 流类型无法 mock）：一次
/// TLS 读结果 → 会话终结原因。`Ok(n>0)`（正常读到字节）→ `None`（继续读）；
/// `Ok(0)`（网关 clean close）→ `GatewayClosedStream`；`Err(e)` → `Io(SessionIoError)`。
fn session_end_reason_for_read(outcome: &std::io::Result<usize>) -> Option<SessionEndReason> {
    match outcome {
        Ok(0) => Some(SessionEndReason::GatewayClosedStream),
        Err(e) => Some(SessionEndReason::Io(SessionIoError::new(
            IoDirection::Read,
            e,
        ))),
        Ok(_) => None,
    }
}

/// 纯映射 seam（W5 可测）：一次 codec 解码结果 → 会话终结原因。
/// `Err` 且**非** `UnknownControl`（未知控制帧经 [`control_event_for_decode`]
/// 上抛后读任务继续存活）→ `Codec(err)`；其余（数据/半帧/未知控制帧）→ `None`。
fn session_end_reason_for_decode(
    outcome: &Result<Option<CstpFrame>, CodecError>,
) -> Option<SessionEndReason> {
    match outcome {
        Ok(Some(CstpFrame::Control { kind: kind @ (0x05 | 0x09), body })) => {
            Some(SessionEndReason::ServerDisconnect {
                kind: *kind,
                code: body.first().copied(),
                reason: redact_io_detail(&String::from_utf8_lossy(body.get(1..).unwrap_or_default()))
                    .chars().filter(|character| !character.is_control()).take(256).collect(),
                body_len: body.len(),
            })
        }
        Err(err) if !matches!(err, CodecError::UnknownControl(_)) => {
            Some(SessionEndReason::Codec(err.clone()))
        }
        _ => None,
    }
}

/// 纯映射 seam（W5 可测）：一次数据帧交付结果 → 会话终结原因。
/// 交付失败（`read_tx` 消费端已 drop）→ `ConsumerDropped`；成功 → `None`。
fn session_end_reason_for_data_send(
    sent: &Result<(), mpsc::error::SendError<Vec<u8>>>,
) -> Option<SessionEndReason> {
    if sent.is_err() {
        Some(SessionEndReason::ConsumerDropped)
    } else {
        None
    }
}

/// Split the stream into the two data tasks: the TLS read half is decoded by
/// the frozen P40 `Codec` and `CstpFrame::Data` payloads are delivered on
/// `read_tx`; frames on `write_rx` are written to the TLS write half raw (engine
/// version of the school `spawn_school_data_tasks` pattern). `offer_remainder`
/// holds bytes that arrived after the offer terminator and belongs to the binary
/// frame stream.
///
/// T1 control-plane surface: decoded control frames (keepalive) and the DPD-response
/// probe signal are forwarded on `control_tx` (best-effort — a dropped consumer is
/// non-fatal). An unknown control frame is SKIPPED rather than fatal: the read task
/// must survive a gateway control frame the codec cannot decode (in particular a
/// DPD response, which the codec reports as `UnknownControl(0x04)`); 每一个未知
/// kind 都经 [`control_event_for_decode`] 上抛为 `Control { kind, .. }`（F4 收敛，
/// 宿主可见且可应答网关 DPD request 0x03）。
///
/// F4 退出可观测：读任务的**每一个**退出点（网关 clean close / io 错误 / codec 真
/// 错误 / 消费端先掉）退出前都先发 `SessionEnded(reason)` 作为最后一条事件。
fn spawn_data_tasks(
    read_half: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    write_half: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    read_tx: mpsc::UnboundedSender<Vec<u8>>,
    write_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    offer_remainder: Vec<u8>,
    control_tx: mpsc::UnboundedSender<CstpControlEvent>,
) {
    let write_control_tx = control_tx.clone();
    // 任一方向退出就取消另一方向；不能留下读任务永久等待，或让旧 TLS 会话跨重连存活。
    let (read_done, read_ended) = tokio::sync::oneshot::channel::<()>();
    let (write_done, write_ended) = tokio::sync::oneshot::channel::<()>();
    let consumer = read_tx.clone();
    let consumer_control = control_tx.clone();
    let read_task = async move {
        let mut diagnostics = SessionIoDiagnostics::new(IoDirection::Read);
        let mut codec = Codec::new();
        codec.feed(&offer_remainder);
        let mut read_half = read_half;
        let mut chunk = [0u8; 4096];
        loop {
            // HTTP offer 后同次 read 中的二进制余量也必须立即解码，不能等下一次网络读。
            loop {
                let outcome = codec.decode();
                match outcome {
                    Ok(Some(CstpFrame::Data(payload))) => {
                        if session_end_reason_for_data_send(&read_tx.send(payload)).is_some() {
                            diagnostics.publish(&control_tx);
                            let _ = control_tx.send(CstpControlEvent::SessionEnded(SessionEndReason::ConsumerDropped));
                            return;
                        }
                    }
                    outcome => {
                        if let Some(event) = control_event_for_decode(&outcome) { let _ = control_tx.send(event); }
                        if let Some(reason) = session_end_reason_for_decode(&outcome) {
                            diagnostics.publish(&control_tx);
                            let _ = control_tx.send(CstpControlEvent::SessionEnded(reason));
                            return;
                        }
                        if matches!(outcome, Ok(None)) { break; }
                    }
                }
            }
            match read_half.read(&mut chunk).await {
                Ok(n) if n > 0 => {
                    diagnostics.record(n);
                    if diagnostics.sampled_at.elapsed() >= Duration::from_secs(15) {
                        diagnostics.publish(&control_tx);
                    }
                    codec.feed(&chunk[..n]);
                }
                outcome => {
                    // 读任务退出（F4：原 `Ok(0) | Err(_) => break` 合并静默 → 拆分
                    // 可观测）。最后一条事件携带原因；控制面通道无消费端时静默失败
                    // （best-effort，与既有语义一致）。
                    if let Some(reason) = session_end_reason_for_read(&outcome) {
                        diagnostics.publish(&control_tx);
                        let _ = control_tx.send(CstpControlEvent::SessionEnded(reason));
                    }
                    return;
                }
            }
        }
    };
    tokio::spawn(async move {
        let _done = read_done;
        tokio::select! {
            _ = write_ended => {}
            _ = consumer.closed() => {
                let _ = consumer_control.send(CstpControlEvent::SessionEnded(SessionEndReason::ConsumerDropped));
            }
            _ = read_task => {}
        }
    });
    tokio::spawn(async move {
        let _done = write_done;
        tokio::select! {
            _ = read_ended => {}
            _ = write_data_task(write_half, write_rx, write_control_tx) => {}
        }
    });
}

/// 将诊断放在实际写入与 flush 之后；独立 AsyncWrite seam 可复现断管及缓冲提交失败。
async fn write_data_task<W: tokio::io::AsyncWrite + Unpin>(
    mut write_half: W,
    mut write_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    control_tx: mpsc::UnboundedSender<CstpControlEvent>,
) {
    let mut diagnostics = SessionIoDiagnostics::new(IoDirection::Write);
    while let Some(frame) = write_rx.recv().await {
        let write_result = async {
            write_half.write_all(&frame).await?;
            write_half.flush().await
        }
        .await;
        if let Err(error) = write_result {
            let error = SessionIoError::new(IoDirection::Write, &error);
            diagnostics.publish(&control_tx);
            let _ = control_tx.send(CstpControlEvent::WriteFailed(error));
            return;
        }
        diagnostics.record(frame.len());
        let control_write = match frame.get(6).copied() {
            Some(crate::codec::CSTP_PACKET_TYPE_KEEPALIVE) => {
                diagnostics.keepalive_frames += 1;
                true
            }
            Some(CSTP_PACKET_TYPE_DPD_REQUEST) => {
                diagnostics.dpd_request_frames += 1;
                true
            }
            Some(CSTP_PACKET_TYPE_DPD_RESPONSE) => {
                diagnostics.dpd_response_frames += 1;
                true
            }
            _ => false,
        };
        if control_write || diagnostics.sampled_at.elapsed() >= Duration::from_secs(15) {
            diagnostics.publish(&control_tx);
        }
    }
    diagnostics.publish(&control_tx);
}

// ---------------------------------------------------------------------------
// 单元测试：T1 控制面事件映射（DPD response 探测信号 + keepalive 转发 + 未知
// 控制帧存活）。`control_event_for_decode` 是纯函数，直接断言解码结果→事件。
// ---------------------------------------------------------------------------

