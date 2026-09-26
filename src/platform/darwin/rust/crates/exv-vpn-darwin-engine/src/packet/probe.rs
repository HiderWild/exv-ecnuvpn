//! 隧道内延迟探测（win32 engine `data_plane.rs` T1 latency design v2 的 darwin 平移）。
//!
//! 语义对齐 win32：[`ProbeState`] 状态机周期性经隧道发 IPv4 ICMP echo request 到
//! 网关侧隧道子网网络基地址（[`network_base`]），匹配 echo reply 计时 RTT 写
//! `StatsRegistry::record_latency`；DPD 通道（探索性，[`DPD_PROBE_ENABLED`] 默认
//! 关——学校 ASA 应答未验证）无应答连续超时后永久回退 ping。组包/匹配
//! （[`build_icmp_echo_request`]/[`is_icmp_echo_reply`]/[`internet_checksum`]）为
//! 手工字节运算，零平台依赖。
//!
//! 与 win32 的差异只在接线面（darwin 泵是双阻塞线程，win32 是 writer 轮询线程）：
//! * 节拍由 `bootstrap_runtime.rs` 管线保持循环的 1s tick 驱动 [`ProbeState::tick`]，
//!   探测包经管线持有的 `write_channel` clone 注入上行（CSTP Data 帧）；
//! * 回包匹配挂下行泵线程写 utun 之前（[`LatencyTap`]），RTT 经共享注册表原子回传；
//! * 网关 DPD request 的被动应答（0x03→0x04）是既有会话维持职责，留在
//!   `bootstrap_runtime.rs` 保持循环内，与本模块的主动探测（默认关）互不混淆。
//!
//! **不做手动刷新 marker 文件**（win32 有、darwin 刻意不移植）：win32 前端
//! 「立即刷新延迟」写用户配置目录的 marker 文件、writer 线程每秒轮询；darwin
//! engine 是 root 进程、argv 固定无用户配置目录，路径共识不成立（上轮调研结论）。
//! v1 = 周期 [`LATENCY_PING_INTERVAL_SECS`]（180s）探测；tauri 侧
//! `trigger_latency_refresh` 命令由 UI 车道以 no-op 语义实现。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_cstp::codec::{Codec, CstpFrame};
use exv_vpn_cstp::session::CSTP_PACKET_TYPE_DPD_REQUEST;

use crate::stats::StatsRegistry;

/// DPD 探测间隔（探索性；对齐 win32 task T1 约定 ~5s）。
const DPD_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// DPD 应答超时（超过即认为无应答）。
const DPD_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// DPD 连续超时次数达到此值 → 永久回退 ping（学校 ASA 大概率不支持 DPD）。
const DPD_MAX_TIMEOUTS: u32 = 3;
/// ping 探测超时（无 echo reply 则本次探测无结果，不记录）。
const PING_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// DPD 主动探测开关（探索性，默认关——学校 ASA 应答未验证，真机验证沿 win32 S5 口径）。
pub const DPD_PROBE_ENABLED: bool = false;
/// ping 探测周期（秒；每 3 分钟 1 次，服务端负荷最小，对齐 win32）。
pub const LATENCY_PING_INTERVAL_SECS: u64 = 180;

/// 延迟探测配置（由管线在拿到 CSTP offer 后构建；对齐 win32 `LatencyProbeConfig`
/// 减去 darwin 不做的 refresh_marker 字段）。
#[derive(Debug, Clone)]
pub struct LatencyProbeConfig {
    /// 隧道内网探测目标地址（默认 = 客户端子网网络基地址；真实学校网关应答待真机校准）。
    pub target: Ipv4Addr,
    /// 本地隧道分配地址（ICMP echo request 源地址）。
    pub source: Ipv4Addr,
    /// 是否启用 DPD 探测（探索性；默认关——ASA 应答未验证）。
    pub dpd_enabled: bool,
    /// ping 探测周期（默认 3 分钟）。
    pub ping_interval: Duration,
}

/// 探测状态机（`Instant` 计时经 `tick` 注入便于测试；跨线程共享见 [`LatencyTap`]）。
///
/// 注：不 derive `Debug`——持有的 `Codec` 无 `Debug` 实现（对齐 win32）。
pub struct ProbeState {
    cfg: LatencyProbeConfig,
    kind: ProbeKind,
    dpd_timeouts: u32,
    last_dpd_sent: Instant,
    last_ping_sent: Instant,
    pending_dpd: Option<Instant>,
    pending_ping: Option<PendingPing>,
    next_ident: u16,
    codec: Codec,
}

/// 当前探测通道（DPD 优先，无应答回退 ping）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    Dpd,
    Ping,
}

/// 一次在途 ping 的状态。
#[derive(Debug, Clone, Copy)]
struct PendingPing {
    sent_at: Instant,
    ident: u16,
}

impl ProbeState {
    /// 建一个探测状态机：`dpd_enabled` 时从 DPD 通道开始，否则直接用 ping。
    #[must_use]
    pub fn new(cfg: LatencyProbeConfig) -> Self {
        let kind = if cfg.dpd_enabled {
            ProbeKind::Dpd
        } else {
            ProbeKind::Ping
        };
        Self {
            cfg,
            kind,
            dpd_timeouts: 0,
            last_dpd_sent: Instant::now(),
            last_ping_sent: Instant::now(),
            pending_dpd: None,
            pending_ping: None,
            next_ident: 0,
            codec: Codec::new(),
        }
    }

    /// 收到 DPD response：若有在途 DPD 请求 → 返回 RTT（ms）并清除请求。
    pub fn on_dpd_response(&mut self, now: Instant) -> Option<u64> {
        let sent_at = self.pending_dpd.take()?;
        self.dpd_timeouts = 0;
        Some(rtt_ms(sent_at, now))
    }

    /// 收到一个入站包：若匹配在途 ping 的 echo reply → 返回 RTT（ms）并清除请求。
    pub fn on_icmp_echo_reply(&mut self, pkt: &[u8], now: Instant) -> Option<u64> {
        let pending = self.pending_ping.take()?;
        if is_icmp_echo_reply(pkt, self.cfg.target, self.cfg.source, pending.ident) {
            Some(rtt_ms(pending.sent_at, now))
        } else {
            self.pending_ping = Some(pending);
            None
        }
    }

    /// 探测循环节拍：超时处理 → 周期触发。`send` 接收已编码的 CSTP 帧（调用方经
    /// `write_channel` 发出；测试注入记录闭包）。win32 的手动刷新 marker 轮询分支
    /// 不移植（见模块注释）。
    pub fn tick(&mut self, now: Instant, send: &mut dyn FnMut(Vec<u8>)) {
        // 1. 超时：DPD 无应答累计 → 回退 ping；ping 无应答清空。
        if let Some(sent_at) = self.pending_dpd
            && now.duration_since(sent_at) >= DPD_PROBE_TIMEOUT
        {
            self.pending_dpd = None;
            self.dpd_timeouts += 1;
            if self.dpd_timeouts >= DPD_MAX_TIMEOUTS {
                // 学校网关不应答 DPD → 永久回退 ping（真机验证沿 win32 S5 口径）。
                self.kind = ProbeKind::Ping;
            }
        }
        if let Some(pending) = self.pending_ping
            && now.duration_since(pending.sent_at) >= PING_PROBE_TIMEOUT
        {
            self.pending_ping = None;
        }

        // 2. 周期触发：DPD ~5s / ping `ping_interval`。
        match self.kind {
            ProbeKind::Dpd => {
                if now.duration_since(self.last_dpd_sent) >= DPD_PROBE_INTERVAL {
                    self.fire(now, send);
                }
            }
            ProbeKind::Ping => {
                if now.duration_since(self.last_ping_sent) >= self.cfg.ping_interval {
                    self.fire(now, send);
                }
            }
        }
    }

    /// 按当前通道发一次探测。
    fn fire(&mut self, now: Instant, send: &mut dyn FnMut(Vec<u8>)) {
        match self.kind {
            ProbeKind::Dpd => self.fire_dpd(now, send),
            ProbeKind::Ping => self.fire_ping(now, send),
        }
    }

    /// 发 DPD request（CSTP control 帧 0x03）并记录在途请求。
    fn fire_dpd(&mut self, now: Instant, send: &mut dyn FnMut(Vec<u8>)) {
        self.last_dpd_sent = now;
        self.pending_dpd = Some(now);
        let frame = build_dpd_request_frame();
        send(frame);
    }

    /// 发 ICMP echo request（经隧道）并记录在途 ping。
    fn fire_ping(&mut self, now: Instant, send: &mut dyn FnMut(Vec<u8>)) {
        self.last_ping_sent = now;
        let ident = self.next_ident;
        self.next_ident = self.next_ident.wrapping_add(1);
        let pkt = build_icmp_echo_request(self.cfg.source, self.cfg.target, ident, 0);
        if let Ok(frame) = self.codec.encode(&CstpFrame::Data(pkt)) {
            self.pending_ping = Some(PendingPing { sent_at: now, ident });
            send(frame);
        }
    }
}

/// 下行泵的延迟匹配接线：共享状态机 + RTT 写入端。
///
/// darwin 的结构适配点（对齐 win32「writer 线程匹配回包」职责）：下行泵线程在每个
/// 入站 payload 写 utun 之前调用 [`Self::on_inbound`]——匹配在途 ping 的 echo
/// reply 即算 RTT 写注册表；锁只在匹配瞬时持有（1s 一次 tick 与稀有回包，无竞争面）。
pub struct LatencyTap {
    probe: Arc<Mutex<ProbeState>>,
    stats: Arc<StatsRegistry>,
}

impl LatencyTap {
    /// 绑定共享状态机与统计注册表（状态机 Arc 与管线 tick 驱动共用同一实例）。
    #[must_use]
    pub fn new(probe: Arc<Mutex<ProbeState>>, stats: Arc<StatsRegistry>) -> Self {
        Self { probe, stats }
    }

    /// 下行泵每个入站 IP payload 写 utun 之前调用一次；匹配即记录 RTT。
    ///
    /// # Panics
    ///
    /// 状态机互斥锁中毒（持锁线程 panic；探测路径无 panic 源）→ panic。
    pub fn on_inbound(&self, payload: &[u8]) {
        let mut probe = self.probe.lock().expect("latency probe lock");
        if let Some(rtt) = probe.on_icmp_echo_reply(payload, Instant::now()) {
            self.stats.record_latency(rtt);
        }
    }
}

/// 两次 Instant 的往返毫秒（饱和下取）。
fn rtt_ms(sent_at: Instant, now: Instant) -> u64 {
    u64::try_from(now.duration_since(sent_at).as_millis()).unwrap_or(u64::MAX)
}

/// 构建 DPD request 的 CSTP 帧字节（control kind 0x03，body = 4 字节 BE unix 时间，
/// openconnect `cstp.c` DPD 形态；对齐 win32 同名函数）。
#[must_use]
fn build_dpd_request_frame() -> Vec<u8> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(0));
    Codec::encode_raw(CSTP_PACKET_TYPE_DPD_REQUEST, &stamp.to_be_bytes())
        .expect("DPD request frame encodes within bounds")
}

/// 构建一个 IPv4 ICMP echo request 包（源/目标/标识/序号；对齐 win32 同名函数，
/// 头字段写入改用 `to_be_bytes` 消除窄化转换）。
#[must_use]
fn build_icmp_echo_request(source: Ipv4Addr, target: Ipv4Addr, ident: u16, seq: u16) -> Vec<u8> {
    let ident_bytes = ident.to_be_bytes();
    let payload = [b'E', b'X', b'V', 0x01, ident_bytes[0], ident_bytes[1]];
    let total_len = 20 + 8 + payload.len();
    let mut pkt = vec![0u8; total_len];
    pkt[0] = 0x45; // IPv4, IHL=5
    pkt[2..4].copy_from_slice(&u16::try_from(total_len).unwrap_or(u16::MAX).to_be_bytes());
    pkt[8] = 64; // TTL
    pkt[9] = 1; // ICMP
    pkt[12..16].copy_from_slice(&source.octets());
    pkt[16..20].copy_from_slice(&target.octets());
    pkt[20] = 8; // ICMP echo request
    pkt[24..26].copy_from_slice(&ident_bytes);
    pkt[26..28].copy_from_slice(&seq.to_be_bytes());
    pkt[28..28 + payload.len()].copy_from_slice(&payload);
    // IP 头校验和 + ICMP 校验和都必须正确：探测包经 CSTP Data 帧上传，网关解封装后
    // 在校园网内路由——校验和为零的 IP 头会被网关丢弃（无回包）。
    let ip_sum = internet_checksum(&pkt[0..20]);
    pkt[10..12].copy_from_slice(&ip_sum.to_be_bytes());
    let icmp_sum = internet_checksum(&pkt[20..]);
    pkt[22..24].copy_from_slice(&icmp_sum.to_be_bytes());
    pkt
}

/// `pkt` 是否为匹配本探测的 ICMP echo reply（IPv4/ICMP 类型 0/源=请求目标/
/// 目标=请求源/标识匹配）。IP 头布局：字节 12-16 = 源地址，16-20 = 目标地址；
/// echo reply 的源 = 被 ping 的 `target`，目标 = 原始发送方 `source`。
#[must_use]
fn is_icmp_echo_reply(pkt: &[u8], target: Ipv4Addr, source: Ipv4Addr, ident: u16) -> bool {
    if pkt.len() < 28 {
        return false;
    }
    if pkt[0] >> 4 != 4 {
        return false; // IPv4 only
    }
    if pkt[9] != 1 {
        return false; // ICMP
    }
    if pkt[12..16] != target.octets() {
        return false; // reply 源 = 被 ping 的目标
    }
    if pkt[16..20] != source.octets() {
        return false; // reply 目标 = 请求源
    }
    if pkt[20] != 0 {
        return false; // echo reply
    }
    pkt[24..26] == ident.to_be_bytes()
}

/// 标准 Internet checksum（one's complement，按 BE 16 位字；折叠循环保证结果
/// 高 16 位为零，`u16::try_from` 不可能失败——失败路径按全 1 回退，不 panic）。
#[must_use]
fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in bytes.chunks(2) {
        let word = u16::from_be_bytes([pair[0], pair.get(1).copied().unwrap_or(0)]);
        sum = sum.wrapping_add(u32::from(word));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !u16::try_from(sum).unwrap_or(u16::MAX)
}

/// 客户端隧道子网的网络基地址（探测目标默认值；`addr` 按 `prefix` 掩码清零主机位；
/// win32 `tunnel_runtime.rs` 同名函数平移，另修其 `prefix=0` 时的 32 位移位溢出）。
/// 管线在拿到 CSTP offer 后调用。
#[must_use]
pub(crate) fn network_base(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = match prefix {
        0 => 0,
        1..=31 => u32::MAX << (32 - u32::from(prefix)),
        32..=u8::MAX => u32::MAX,
    };
    Ipv4Addr::from(u32::from(addr) & mask)
}

// ---------------------------------------------------------------------------
// 单元测试：与 win32 `data_plane.rs` 的 T1 Part B 用例一同平移（marker 用例除外）。
// ---------------------------------------------------------------------------
