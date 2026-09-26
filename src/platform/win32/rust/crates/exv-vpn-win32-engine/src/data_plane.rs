
//! engine 特权进程内的 Wintun 数据面（R1b）：ring→CSTP→TLS reader + TLS→CSTP→ring
//! writer，**全在 engine 进程内零跨进程**。
//!
//! 本模块是 acceptance `engine_data_plane`（阶段 3b 真实组装）的产品移植——复用同一组
//! 产品资源 seam（`exv-vpn-win32-resource` wintun adapter/session/api + `exv-vpn-cstp`
//! codec），不依赖 acceptance crate（test-only）。R0 归因：数据面空转 sleep 不在 apply
//! 关键路径（零贡献），移植保持原有 50ms 事件等待 / 1ms 空转语义不变。
//!
//! - [`EngineDataPlane::start`]：在**已创建**的 adapter 上启动 engine session
//!   （adapter/lib 由调用方持有——`crate::platform_tunnel` 创建 adapter 并持有到
//!   Stop；本结构只持有 `Arc<Mutex<WintunSession>>` 供 reader/writer 线程共享）。
//! - [`EngineDataPlane::spawn_data_plane`]：spawn 两条数据面线程——
//!   **reader**（ring → `session.receive()` → `Codec::encode(CstpFrame::Data)` →
//!   CSTP `write_channel` → TLS → 学校）与 **writer**（CSTP `read_channel`（TLS
//!   解码帧，IP 包）→ `session.send()` → ring）。
//! - [`EngineDataPlaneThreads::stop_and_join`]：置位 stop → join 两线程 → 返回
//!   `(ring_received_bytes, ring_sent_bytes)`（engine 数据面计数器）。
//!
//! **W17 SAFETY-ORDER（冻结）**：[`EngineDataPlaneThreads`] 在 `stop_and_join`/`Drop`
//! 时**先 join 两个线程，再放掉线程持有的 session Arc 克隆**——`WintunEndSession`
//! 之前 worker 必已 join（0.14.1 的 EndSession 销毁 session 对象，之后 receive 是
//! UAF）。线程的 read-wait event 是 session 管理的（调用方不 CloseHandle），50ms
//! 等待保持 stop 标志可及时观察（可中断 join）。

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_cstp::codec::{CSTP_PACKET_TYPE_KEEPALIVE, Codec, CstpFrame};
use exv_vpn_cstp::session::{
    CSTP_PACKET_TYPE_DPD_REQUEST, CSTP_PACKET_TYPE_DPD_RESPONSE, CstpControlEvent, IoDirection,
    SessionEndReason, SessionIoDiagnostics, SessionIoError,
};
use exv_vpn_win32_resource::wintun_adapter::WintunAdapter;
use exv_vpn_win32_resource::wintun_api::WintunLibrary;
use exv_vpn_win32_resource::wintun_session::WintunSession;
use exv_vpn_wire::generated;
use generated::ConnectPhase;
use tokio::sync::mpsc;
use windows::Win32::Foundation::WAIT_FAILED;
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::stats::StatsRegistry;
use crate::status::{StatusEvent, StatusPublisher};

/// Wintun ring capacity for the engine data plane（同 acceptance `WINTUN_PROBE_RING_CAPACITY`）。
pub const WINTUN_RING_CAPACITY: u32 = 1 << 20;

/// engine 数据面（DP-03 结构）：持有共享的 Wintun session 供 reader/writer 数据面
/// 线程直连。
///
/// 本结构只持有 `Arc<Mutex<WintunSession>>`——**不持有 adapter / WintunLibrary**。
/// 调用方（engine 的 `tunnel_runtime`，经 `platform_tunnel`）持有创建者 adapter 与
/// WintunLibrary 并保证其存活覆盖本结构：teardown 顺序是 `stop_and_join` →
/// drop `EngineDataPlane`（`WintunEndSession`）→ `platform_tunnel::restore`
/// （adapter creator close 移除 adapter、释放 lib）。session 的 `Drop` 用内嵌的
/// exports 副本调用 `WintunEndSession`，不依赖 lib 存活；但 lib 保持 DLL 加载，
/// lib 必须先于 session 存活（调用方 teardown 顺序保证）。
pub struct EngineDataPlane {
    /// engine session（在已创建的 adapter 上启动；最后一个 `Arc` drop 即
    /// `WintunEndSession`）。
    pub session: Arc<Mutex<WintunSession>>,
}

/// 两个工作线程共享一次失败上报；正常停止不制造掉线，提前退出（含 unwind）不能静默。
struct DataPlaneWorkerGuard {
    stop: Arc<AtomicBool>,
    reported: Arc<AtomicBool>,
    operation_id: Arc<Mutex<Vec<u8>>>,
    worker: &'static str,
    aux: DataPlaneAux,
    log: Option<Arc<crate::log_sink::LogSink>>,
}

impl DataPlaneWorkerGuard {
    fn report(&self, trigger: &str, native_code: Option<u32>, reason: Option<&SessionEndReason>) {
        let operation_guard = self.operation_id.lock().unwrap_or_else(|error| error.into_inner());
        if self.stop.load(Ordering::Acquire) || self.reported.swap(true, Ordering::AcqRel) { return; }
        let operation_id = operation_guard.clone();
        drop(operation_guard);
        if let Some(log) = &self.log {
            log.emit(crate::log_sink::LogLevel::Warn, "dataplane", "tunnel.dataplane.worker-failed",
                "数据面工作线程终止，当前会话已不可用",
                &[("worker", self.worker), ("trigger", trigger),
                    ("operation_id", &uuid::Uuid::from_slice(&operation_id).map(|id| id.to_string()).unwrap_or_default()),
                    ("native_code", &native_code.map(|code| code.to_string()).unwrap_or_default())]);
        }
        on_data_plane_lost(&self.aux.notify_data_plane_lost, &self.log, &self.aux.status,
            &operation_id, self.aux.stats.as_deref(), reason);
    }
}

impl Drop for DataPlaneWorkerGuard {
    fn drop(&mut self) {
        self.report(if std::thread::panicking() { "worker_panic" } else { "unexpected_exit" }, None, None);
    }
}

/// 数据面辅助接线（T1：统计注册表 + 延迟探测输入；C2：掉线状态上报）。
///
/// 单一入口避免 `spawn_data_plane` 参数爆炸；`Option` 字段不接线时行为与 T1 之前
/// 一致（只做 ring 计数器诊断，不写 registry、不探测延迟、不发布状态）。
///
/// 注：不 derive `Debug`——`StatusPublisher` 无 `Debug` 实现（内部持 `dyn Fn`）。
#[derive(Clone)]
pub struct DataPlaneAux {
    /// 共享统计注册表（数据面计数 + 延迟写入；`None` = 不接线）。
    pub stats: Option<Arc<StatsRegistry>>,
    /// 延迟探测配置（`None` = 关闭延迟探测）。
    pub latency: Option<LatencyProbeConfig>,
    /// CSTP 控制面事件接收端（DPD response 等；来自 cstp session；`None` = 无）。
    pub control_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<CstpControlEvent>>>>,
    /// 状态发布端点（C2：数据面掉线等运行时状态上报；`None` = 不接线——不发布状态）。
    pub status: Option<Arc<StatusPublisher>>,
    /// 关联的 apply operation_id（掉线状态事件携带；未接线时为空）。
    pub operation_id: Vec<u8>,
    /// T3b（F6）：数据面掉线信号发射端——writer 检测到 read_channel 关闭时调用
    /// **恰好一次**（sweeper 异步清 live，S4）。世代号由构造方
    /// （`tunnel_runtime::assemble`）预制在闭包内，writer 无需感知世代细节（复审
    /// P2-5）。`None` = 未接线（行为与 T3 之前一致）。
    pub notify_data_plane_lost: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for DataPlaneAux {
    fn default() -> Self {
        Self {
            stats: None,
            latency: None,
            control_rx: None,
            status: None,
            operation_id: Vec::new(),
            notify_data_plane_lost: None,
        }
    }
}

/// 方向映射（T1 核对结论）：reader 消费 ring 里的**出站**包（本地应用上传 →
/// CSTP/TLS → 学校）= 上传 = `tx`；writer 把 TLS 解码的**入站**包（学校 → 本地
/// 下载）送进 ring = 下载 = `rx`。与 tunnel_runtime 诊断注释（ring_received 增长 =
/// 出站被 reader 消费；ring_sent 增长 = TLS 回包被 writer 送进 ring）及前端
/// 「下载/上传」标签（rx=下载，tx=上传）一致。
fn count_reader_upload(counter: &AtomicU64, stats: Option<&StatsRegistry>, len: u64) {
    counter.fetch_add(len, Ordering::SeqCst);
    if let Some(stats) = stats {
        stats.record_tx(len);
    }
}

/// 方向映射（见 [`count_reader_upload`]）：writer 送进 ring 的是入站/下载 → `rx`。
fn count_writer_download(counter: &AtomicU64, stats: Option<&StatsRegistry>, len: u64) {
    counter.fetch_add(len, Ordering::SeqCst);
    if let Some(stats) = stats {
        stats.record_rx(len);
    }
}

// ---------------------------------------------------------------------------
// T1 latency design v2：隧道内延迟探测（DPD RTT 探索性优先 → ping fallback）。
// ---------------------------------------------------------------------------
//
// 探测循环跑在 writer 数据面线程（入站包必经 read_channel，DPD response 经
// control_rx 到达）：
//   * DPD（探索性，feature 开关 `dpd_enabled`，默认关）：每 ~5s 经 write_channel
//     发 DPD request（CSTP control 帧 0x03），收到 response（0x04）算 RTT →
//     `registry.record_latency`。学校 ASA 是否应答未验证——连续超时则**永久回退**
//     ping（真实机器验证属 S5）。
//   * ping fallback：每 `ping_interval`（默认 3 分钟）经隧道发 ICMP echo request
//     到学校网关隧道内网地址，匹配 echo reply 计时 → `record_latency`。
//   * 手动立即刷新：前端写 `refresh_marker` 文件（内容 = epoch 毫秒），探测循环
//     每秒轮询一次，发现新值立即执行一次探测。
//
// wire/UI 零变更：延迟最终经既有 `StatsRegistry.latency_ms` → `StatsEvent` →
// `RuntimeSnapshot.stats` 到达前端。

/// DPD 探测间隔（探索性；task T1 约定 ~5s）。
const DPD_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// DPD 应答超时（超过即认为无应答）。
const DPD_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// DPD 连续超时次数达到此值 → 永久回退 ping（学校 ASA 大概率不支持 DPD）。
const DPD_MAX_TIMEOUTS: u32 = 3;
/// ping 探测超时（无 echo reply 则本次探测无结果，不记录）。
const PING_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// 手动刷新标记文件的轮询间隔。
const MARKER_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// F2（2026-09-05 cstp-keepalive 计划）：周期 CSTP keepalive 周期（秒）。
///
/// 裁决：15s 与 darwin 已在真实网关运行的实现字节级对齐（`bootstrap_runtime.rs`
/// 1s tick × 15 发一帧空 body 0x07），且远小于网关 ~60s 断开阈值（`session.rs`
/// GatewayClosedStream 记录）；openconnect 惯例 30s 仅作上限参考不采纳为初值。
/// 与 DPD 探测开关（`DPD_PROBE_ENABLED`）相互独立：keepalive 是会话维持，
/// 不是探测。
pub(crate) const CSTP_KEEPALIVE_INTERVAL_SECS: u64 = 15;

/// 延迟探测配置（由 `tunnel_runtime` 在拿到 CSTP offer 后构建）。
#[derive(Debug, Clone)]
pub struct LatencyProbeConfig {
    /// 隧道内网探测目标地址（默认 = 客户端子网网络基地址；真实学校网关应答 S5 验证）。
    pub target: Ipv4Addr,
    /// 本地隧道分配地址（ICMP echo request 源地址）。
    pub source: Ipv4Addr,
    /// 是否启用 DPD 探测（探索性；默认关——ASA 应答未验证）。
    pub dpd_enabled: bool,
    /// ping 探测周期（默认 3 分钟）。
    pub ping_interval: Duration,
    /// 手动刷新标记文件（`None` = 不轮询；前端「立即刷新」经此触发即时 ping）。
    pub refresh_marker: Option<PathBuf>,
}

/// 探测状态机（writer 线程内；`Instant` 计时经 `tick` 注入便于测试）。
///
/// 注：不 derive `Debug`——持有的 `Codec` 无 `Debug` 实现。
struct ProbeState {
    cfg: LatencyProbeConfig,
    kind: ProbeKind,
    dpd_timeouts: u32,
    last_dpd_sent: Instant,
    last_ping_sent: Instant,
    last_marker_check: Instant,
    last_marker_seen: u64,
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
    fn new(cfg: LatencyProbeConfig) -> Self {
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
            last_marker_check: Instant::now(),
            last_marker_seen: 0,
            pending_dpd: None,
            pending_ping: None,
            next_ident: 0,
            codec: Codec::new(),
        }
    }

    /// 收到 DPD response：若有在途 DPD 请求 → 返回 RTT（ms）并清除请求。
    fn on_dpd_response(&mut self, now: Instant) -> Option<u64> {
        let sent_at = self.pending_dpd.take()?;
        self.dpd_timeouts = 0;
        Some(rtt_ms(sent_at, now))
    }

    /// 收到一个入站包：若匹配在途 ping 的 echo reply → 返回 RTT（ms）并清除请求。
    fn on_icmp_echo_reply(&mut self, pkt: &[u8], now: Instant) -> Option<u64> {
        let pending = self.pending_ping.take()?;
        if is_icmp_echo_reply(pkt, self.cfg.target, self.cfg.source, pending.ident) {
            Some(rtt_ms(pending.sent_at, now))
        } else {
            self.pending_ping = Some(pending);
            None
        }
    }

    /// 探测循环节拍：超时处理 → 手动标记 → 周期触发。`send` 接收已编码的 CSTP
    /// 帧（writer 线程经 write_channel 发出；测试注入记录闭包）。
    fn tick(&mut self, now: Instant, send: &mut dyn FnMut(Vec<u8>)) {
        // 1. 超时：DPD 无应答累计 → 回退 ping；ping 无应答清空。
        if let Some(sent_at) = self.pending_dpd {
            if now.duration_since(sent_at) >= DPD_PROBE_TIMEOUT {
                self.pending_dpd = None;
                self.dpd_timeouts += 1;
                if self.dpd_timeouts >= DPD_MAX_TIMEOUTS {
                    // 学校网关不应答 DPD → 永久回退 ping（真实验证 S5）。
                    self.kind = ProbeKind::Ping;
                }
            }
        }
        if let Some(pending) = self.pending_ping {
            if now.duration_since(pending.sent_at) >= PING_PROBE_TIMEOUT {
                self.pending_ping = None;
            }
        }

        // 2. 手动刷新标记（前端「立即刷新延迟」）：轮询本地标记文件，发现新值立即探测。
        if now.duration_since(self.last_marker_check) >= MARKER_POLL_INTERVAL {
            self.last_marker_check = now;
            if let Some(marker) = &self.cfg.refresh_marker {
                if let Some(ts) = read_refresh_marker(marker) {
                    if ts > self.last_marker_seen {
                        self.last_marker_seen = ts;
                        self.fire(now, send);
                        return;
                    }
                }
            }
        }

        // 3. 周期触发：DPD ~5s / ping `ping_interval`。
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
            self.pending_ping = Some(PendingPing {
                sent_at: now,
                ident,
            });
            send(frame);
        }
    }
}

/// 两次 Instant 的往返毫秒（饱和下取）。
fn rtt_ms(sent_at: Instant, now: Instant) -> u64 {
    u64::try_from(now.duration_since(sent_at).as_millis()).unwrap_or(u64::MAX)
}

/// 读手动刷新标记文件：内容为 epoch 毫秒；不存在/解析失败 → `None`。
fn read_refresh_marker(path: &PathBuf) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// 构建 DPD request 的 CSTP 帧字节（control kind 0x03，body = 4 字节 BE unix 时间，
/// openconnect `cstp.c` DPD 形态）。
#[must_use]
fn build_dpd_request_frame() -> Vec<u8> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(0));
    Codec::encode_raw(CSTP_PACKET_TYPE_DPD_REQUEST, &stamp.to_be_bytes())
        .expect("DPD request frame encodes within bounds")
}

/// 构建 DPD **应答**帧（control kind 0x04，空 body；F1 冻结——darwin
/// `bootstrap_runtime.rs` 同款。wire 上 0x03 本无载荷，应答无需回显任何字节）。
#[must_use]
fn build_dpd_response_frame() -> Vec<u8> {
    Codec::encode_raw(CSTP_PACKET_TYPE_DPD_RESPONSE, &[])
        .expect("DPD response frame encodes within bounds")
}

/// F2 周期 CSTP keepalive 节拍器（writer 线程内；与 [`ProbeState`] 同构，
/// `Instant` 注入可单测）。
///
/// 初始化 `last = Some(连接时刻)`——首帧在连接建立后第 15s（对齐 darwin 时序，
/// 不引入第二套节拍）。生命周期与 writer 线程同生共死（`stop_and_join`/`Drop`
/// 既有路径回收，零新增关停管道）。
struct KeepaliveTicker {
    period: Duration,
    last: Option<Instant>,
}

impl KeepaliveTicker {
    /// 建节拍器：`connected_at` = 连接建立时刻（首帧在其后第 `period` 秒）。
    fn new(period: Duration, connected_at: Instant) -> Self {
        Self {
            period,
            last: Some(connected_at),
        }
    }

    /// 距上次发送 ≥ `period` → 触发一次并重置（返回 `true`）；未到期 → `false`。
    fn due(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) >= self.period => {
                self.last = Some(now);
                true
            }
            _ => false,
        }
    }
}

/// writer 线程控制面事件分派动作（F5 冻结分派表的纯决策输出，可单测）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum EventAction {
    /// DPD response（0x04）→ 既有 `ProbeState` RTT 处理。
    ProbeRtt,
    /// 网关 DPD request（0x03）→ 立即回 0x04 空 body 应答（F1，无条件、不 dedup）。
    ReplyDpd,
    /// 静默忽略（网关 keepalive 0x07 = 预期帧；重复出现的未知 kind）。
    Ignore,
    /// 未知控制 kind 首见 → 记一条 Warn（每 kind 每会话至多 1 条）。
    WarnUnknownKind(u8),
    /// 会话终结 → stash 原因（掉线单条 Warn 携带，不单独成日志）。
    StashSessionEnd(SessionEndReason),
}

/// 单条控制面事件 → 动作（纯函数，可单测；`seen_unknown_kinds` 为 writer 线程
/// 局部去重表——每 kind 每会话至多 1 条 Warn）。F5 冻结分派表：
/// `DpdResponse`→RTT；`Control{0x03}`→应答；`Control{0x07}`→静默；
/// `Control{其他}`→首见 Warn；`SessionEnded`→stash。
fn control_event_action(event: &CstpControlEvent, seen_unknown_kinds: &mut Vec<u8>) -> EventAction {
    match event {
        CstpControlEvent::DpdResponse => EventAction::ProbeRtt,
        CstpControlEvent::Control {
            kind: CSTP_PACKET_TYPE_DPD_REQUEST,
            ..
        } => EventAction::ReplyDpd,
        CstpControlEvent::Control {
            kind: CSTP_PACKET_TYPE_KEEPALIVE,
            ..
        } => EventAction::Ignore,
        CstpControlEvent::Control { kind: 0x05 | 0x09, .. } => EventAction::Ignore,
        CstpControlEvent::Control { kind, .. } => {
            if seen_unknown_kinds.contains(kind) {
                EventAction::Ignore
            } else {
                seen_unknown_kinds.push(*kind);
                EventAction::WarnUnknownKind(*kind)
            }
        }
        CstpControlEvent::SessionEnded(reason) => EventAction::StashSessionEnd(reason.clone()),
        CstpControlEvent::WriteFailed(error) => EventAction::StashSessionEnd(SessionEndReason::Io(error.clone())),
        CstpControlEvent::IoDiagnostics(_) => EventAction::Ignore,
    }
}

/// `SessionEndReason` → 掉线单条 Warn 携带的可读原因标签（如 `gateway_closed_stream`
/// / `io: TimedOut`；不包含 secret——ErrorKind/CodecError 均为类型化枚举）。F5 冻结。
#[must_use]
fn session_end_reason_label(reason: &SessionEndReason) -> String {
    match reason {
        SessionEndReason::ServerDisconnect { kind, code, reason, body_len } =>
            format!("server_disconnect: kind={kind:#04x} code={code:?} body_len={body_len} reason={reason}"),
        SessionEndReason::GatewayClosedStream => "gateway_closed_stream".to_string(),
        SessionEndReason::Io(error) => format!("io: {:?}", error.kind),
        SessionEndReason::Codec(err) => format!("codec: {err:?}"),
        SessionEndReason::ConsumerDropped => "consumer_dropped".to_string(),
    }
}

/// 安全的错误字段，原始文本已在 TLS 边界分类；缺失 OS 码不能写成 0。
fn io_error_fields(error: &SessionIoError) -> Vec<(&'static str, String)> {
    vec![
        (
            "io_direction",
            match error.direction {
                IoDirection::Read => "read",
                IoDirection::Write => "write",
            }
            .into(),
        ),
        ("io_kind", format!("{:?}", error.kind)),
        (
            "raw_os_error",
            error
                .raw_os_error
                .map_or_else(|| "none".into(), |code| code.to_string()),
        ),
        ("io_detail", error.detail.clone()),
        ("error_observed_ms", error.observed_ms.to_string()),
    ]
}

fn diagnostic_identity(operation_id: &[u8]) -> Vec<(&'static str, String)> {
    vec![
        (
            "operation_id",
            uuid::Uuid::from_slice(operation_id)
                .map_or_else(|_| "unknown".into(), |id| id.to_string()),
        ),
        (
            "operation_id_hex",
            operation_id.iter().map(|b| format!("{b:02x}")).collect(),
        ),
        (
            "event_observed_ms",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
        ),
    ]
}

fn log_write_failure(
    log: &Option<Arc<crate::log_sink::LogSink>>,
    operation_id: &[u8],
    error: &SessionIoError,
) {
    if let Some(log) = log {
        let mut fields = io_error_fields(error);
        fields.extend(diagnostic_identity(operation_id));
        let refs: Vec<_> = fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        log.emit(
            crate::log_sink::LogLevel::Warn,
            "engine",
            "tunnel.dataplane.tls-write-failed",
            "CSTP TLS write_all failed; queued frames are not confirmed written",
            &refs,
        );
    }
}

/// 在清理通知之前留下本线程观察点；只记录诊断，不变更失败与清理的既有顺序。
fn log_data_plane_lifecycle(
    log: &Option<Arc<crate::log_sink::LogSink>>,
    operation_id: &[u8],
    code: &str,
    outcome: &str,
    reason: Option<&SessionEndReason>,
) {
    if let Some(log) = log {
        let mut fields = diagnostic_identity(operation_id);
        fields.push(("outcome", outcome.to_owned()));
        if let Some(reason) = reason {
            fields.push(("reason", session_end_reason_label(reason)));
            if let SessionEndReason::Io(error) = reason {
                fields.extend(io_error_fields(error));
            }
        }
        let refs: Vec<_> = fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        log.emit(
            crate::log_sink::LogLevel::Debug,
            "engine",
            code,
            "数据面生命周期观察点",
            &refs,
        );
    }
}

/// 会话级诊断只存计数与时刻。TLS 累计值来自真实 task 的低频采样，未知不能报零。
struct DataPlaneDiagnostics {
    started: Instant,
    last_summary: Instant,
    keepalive_queued: u64,
    dpd_reply_queued: u64,
    keepalive_received: u64,
    dpd_requests_received: u64,
    dpd_responses_received: u64,
    control_queue_failures: u64,
    read: Option<SessionIoDiagnostics>,
    write: Option<SessionIoDiagnostics>,
}

impl DataPlaneDiagnostics {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last_summary: Instant::now(),
            keepalive_queued: 0,
            dpd_reply_queued: 0,
            keepalive_received: 0,
            dpd_requests_received: 0,
            dpd_responses_received: 0,
            control_queue_failures: 0,
            read: None,
            write: None,
        }
    }

    fn observe(&mut self, event: &CstpControlEvent) {
        match event {
            CstpControlEvent::IoDiagnostics(snapshot) => match snapshot.direction {
                IoDirection::Read => self.read = Some(snapshot.clone()),
                IoDirection::Write => self.write = Some(snapshot.clone()),
            },
            CstpControlEvent::DpdResponse => self.dpd_responses_received += 1,
            CstpControlEvent::Control {
                kind: CSTP_PACKET_TYPE_KEEPALIVE,
                ..
            } => self.keepalive_received += 1,
            CstpControlEvent::Control {
                kind: CSTP_PACKET_TYPE_DPD_REQUEST,
                ..
            } => self.dpd_requests_received += 1,
            _ => {}
        }
    }

    fn emit(
        &mut self,
        log: &Option<Arc<crate::log_sink::LogSink>>,
        operation_id: &[u8],
        trigger: &str,
    ) {
        self.last_summary = Instant::now();
        let Some(log) = log else {
            return;
        };
        let value = |snapshot: Option<&SessionIoDiagnostics>,
                     select: fn(&SessionIoDiagnostics) -> u64| {
            snapshot.map_or_else(|| "unobserved".into(), |s| select(s).to_string())
        };
        let age = |instant: Option<Instant>| {
            instant.map_or_else(
                || "unobserved".into(),
                |t| t.elapsed().as_millis().to_string(),
            )
        };
        let read = self.read.as_ref();
        let write = self.write.as_ref();
        let mut fields = vec![
            ("trigger", trigger.to_string()),
            (
                "connected_for_ms",
                self.started.elapsed().as_millis().to_string(),
            ),
            (
                "keepalive_interval_secs",
                CSTP_KEEPALIVE_INTERVAL_SECS.to_string(),
            ),
            ("keepalive_queued", self.keepalive_queued.to_string()),
            ("dpd_reply_queued", self.dpd_reply_queued.to_string()),
            ("keepalive_received", self.keepalive_received.to_string()),
            (
                "dpd_requests_received",
                self.dpd_requests_received.to_string(),
            ),
            (
                "dpd_responses_received",
                self.dpd_responses_received.to_string(),
            ),
            (
                "control_queue_failures",
                self.control_queue_failures.to_string(),
            ),
            ("tls_read_bytes", value(read, |s| s.bytes)),
            ("tls_write_completed_bytes", value(write, |s| s.bytes)),
            ("tls_write_completed_frames", value(write, |s| s.operations)),
            ("keepalive_written", value(write, |s| s.keepalive_frames)),
            (
                "dpd_request_written",
                value(write, |s| s.dpd_request_frames),
            ),
            ("dpd_reply_written", value(write, |s| s.dpd_response_frames)),
            (
                "last_tls_read_ms_ago",
                age(read.and_then(|s| s.last_activity)),
            ),
            (
                "last_tls_write_ms_ago",
                age(write.and_then(|s| s.last_activity)),
            ),
            ("tls_read_sample_ms_ago", age(read.map(|s| s.sampled_at))),
            ("tls_write_sample_ms_ago", age(write.map(|s| s.sampled_at))),
        ];
        fields.extend(diagnostic_identity(operation_id));
        let refs: Vec<_> = fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let level = if trigger == "periodic" {
            crate::log_sink::LogLevel::Debug
        } else {
            crate::log_sink::LogLevel::Info
        };
        log.emit(level, "engine", "tunnel.dataplane.session-summary",
            "CSTP session counters and sampled TLS activity (write completion is not peer acknowledgement)", &refs);
    }
}

/// 构建一个 IPv4 ICMP echo request 包（源/目标/标识/序号）。
#[must_use]
fn build_icmp_echo_request(source: Ipv4Addr, target: Ipv4Addr, ident: u16, seq: u16) -> Vec<u8> {
    let payload = [
        b'E',
        b'X',
        b'V',
        0x01,
        (ident >> 8) as u8,
        (ident & 0xFF) as u8,
    ];
    let payload_len = payload.len();
    let total_len = 20 + 8 + payload_len;
    let mut pkt = vec![0u8; total_len];
    pkt[0] = 0x45; // IPv4, IHL=5
    pkt[2] = ((total_len >> 8) & 0xFF) as u8;
    pkt[3] = (total_len & 0xFF) as u8;
    pkt[8] = 64; // TTL
    pkt[9] = 1; // ICMP
    pkt[12..16].copy_from_slice(&source.octets());
    pkt[16..20].copy_from_slice(&target.octets());
    pkt[20] = 8; // ICMP echo request
    pkt[24..26].copy_from_slice(&ident.to_be_bytes());
    pkt[26..28].copy_from_slice(&seq.to_be_bytes());
    pkt[28..28 + payload_len].copy_from_slice(&payload);
    // IP 头校验和 + ICMP 校验和都必须正确：探测包经 CSTP Data 帧上传，网关解封装后
    // 在校园网内路由——校验和为零的 IP 头会被网关丢弃（无回包）。
    let ip_sum = internet_checksum(&pkt[0..20]);
    pkt[10] = (ip_sum >> 8) as u8;
    pkt[11] = (ip_sum & 0xFF) as u8;
    let icmp_sum = internet_checksum(&pkt[20..]);
    pkt[22] = (icmp_sum >> 8) as u8;
    pkt[23] = (icmp_sum & 0xFF) as u8;
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

/// 标准 Internet checksum（one's complement，按 BE 16 位字）。
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
    !(sum as u16)
}

impl EngineDataPlane {
    /// 在**已创建**的 adapter 上启动 engine session（engine 特权进程自己建
    /// adapter——`platform_tunnel::apply` 的 `WintunAdapter::create` 已完成；本调用
    /// 直接在该创建句柄上 `WintunSession::start`，**不再 open-by-name**）。
    ///
    /// # Errors
    ///
    /// `WintunSession::start` 失败（ring 容量非法 → 87；`WintunStartSession` 失败，
    /// 如非特权）→ typed `String`（绝不 panic）。
    pub fn start(
        lib: &WintunLibrary,
        adapter: &WintunAdapter,
        ring_capacity: u32,
    ) -> Result<Self, String> {
        let session =
            WintunSession::start(lib, adapter, ring_capacity).map_err(|e| format!("{e:?}"))?;
        Ok(Self {
            session: Arc::new(Mutex::new(session)),
        })
    }

    /// 启动引擎数据面线程（ring→CSTP reader + CSTP→ring writer），直连引擎 session。
    ///
    /// - **reader 线程**：`read_wait_event`（WaitForSingleObject 50ms，可中断 join）→
    ///   `session.receive()` → `is_ipv4_packet` → ring 字节计数（上传/tx）→
    ///   `Codec::encode(CstpFrame::Data)` → engine `write_channel`（TLS）→ 学校。
    /// - **writer 线程**：engine `read_channel`（TLS 解码帧，IP 包）→ ring 字节计数
    ///   （下载/rx）→ `session.send()` → ring；同时是**会话维持责任人**
    ///   （2026-09-05 cstp-keepalive 计划 F1/F2/F5）：排空 CSTP 控制面事件——网关
    ///   DPD request（0x03）→ 无条件回 0x04 空 body；每 15s 发一帧 0x07 keepalive
    ///   （[`KeepaliveTicker`]，首帧在连接后第 15s）；未知控制 kind 首见记 Warn；
    ///   `SessionEnded(reason)` stash 进局部变量。read_channel 关闭 =
    ///   [`on_read_channel_closed`]（恰好一条带 reason 的 Warn + stats Failed +
    ///   C2 Failed 事件）。
    ///
    /// `aux`（[`DataPlaneAux`]）携带统计注册表（T1 Part A：方向映射见
    /// [`count_reader_upload`]/[`count_writer_download`]）与延迟探测输入（T1 Part B：
    /// DPD→ping fallback 探测循环 + 手动刷新标记）；C2 起另携带状态发布端点与
    /// operation_id。会话退出经 LogSink 输出汇总，Debug 每 30s 输出采样；不逐帧记日志。
    ///
    /// W17 SAFETY-ORDER：返回的 [`EngineDataPlaneThreads`] 在 `stop_and_join`/`Drop`
    /// 时先 join 两个线程，再放掉持有的 session Arc 克隆——`WintunEndSession` 之前
    /// worker 必已 join。
    #[must_use]
    pub fn spawn_data_plane(
        &self,
        write_channel: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        read_channel: &Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
        log: Option<Arc<crate::log_sink::LogSink>>,
        aux: DataPlaneAux,
    ) -> EngineDataPlaneThreads {
        let reader_stop = Arc::new(AtomicBool::new(false));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let ring_received_bytes = Arc::new(AtomicU64::new(0));
        let ring_sent_bytes = Arc::new(AtomicU64::new(0));
        let failure_reported = Arc::new(AtomicBool::new(false));
        let active_operation_id = Arc::new(Mutex::new(aux.operation_id.clone()));
        let activated = Arc::new(AtomicBool::new(false));
        let reader_activated = activated.clone();
        let writer_activated = activated.clone();
        let reader_guard = DataPlaneWorkerGuard { stop: reader_stop.clone(),
            operation_id: active_operation_id.clone(),
            reported: failure_reported.clone(), worker: "wintun_reader", aux: aux.clone(), log: log.clone() };
        let writer_guard = DataPlaneWorkerGuard { stop: writer_stop.clone(),
            operation_id: active_operation_id.clone(),
            reported: failure_reported.clone(), worker: "wintun_writer", aux: aux.clone(), log: log.clone() };

        // ---- reader 线程：ring → ring 计数 → STF Data 帧 → write_channel（TLS）。 ----
        let reader_session = Arc::clone(&self.session);
        let reader_tx = write_channel.clone();
        let reader_stop_flag = Arc::clone(&reader_stop);
        let ring_recv_counter = Arc::clone(&ring_received_bytes);
        let reader_stats = aux.stats.clone();
        let reader = std::thread::spawn(move || {
            let guard = reader_guard;
            if !wait_for_data_plane_activation(&reader_activated, &reader_stop_flag) { return; }
            'reader: while !reader_stop_flag.load(Ordering::SeqCst) {
                // 事件等待基于 session 管理的 read-wait event（调用方不 CloseHandle），
                // 50ms 超时保持 stop 标志可及时观察（可中断 join）。
                let ev = reader_session.lock().expect("session 锁").read_wait_event();
                // SAFETY: ev 是 session 管理的有效事件句柄（本线程不 CloseHandle）。
                let wr = unsafe { WaitForSingleObject(ev, 50) };
                if wr == WAIT_FAILED {
                    // 必须紧随失败调用取 GetLastError，避免其它 Win32 调用覆盖原始码。
                    let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
                    guard.report("wintun_wait_failed", Some(code), None);
                    break; // 事件句柄无效（session 已结束等）：停止线程，绝不 panic
                }
                // 排空 ring（read-wait 只保证"至少一个"；内部循环直到空）。
                loop {
                    let received = {
                        let mut sess = reader_session.lock().expect("session 锁");
                        match sess.receive() {
                            Ok(Some(pkt)) => Some(pkt.as_slice().to_vec()),
                            // drop(pkt) 即释放 outstanding receive（W17 冻结契约）
                            Ok(None) => None,
                            Err(e) => {
                                eprintln!("[info] engine data-plane receive Err: {e:?}");
                                drop(sess);
                                guard.report("wintun_receive_failed", Some(e.code), None);
                                break 'reader;
                            }
                        }
                    };
                    let Some(pkt) = received else { break };
                    count_reader_upload(
                        &ring_recv_counter,
                        reader_stats.as_deref(),
                        pkt.len() as u64,
                    );
                    if is_ipv4_packet(&pkt) {
                        let Ok(frame) = Codec::new().encode(&CstpFrame::Data(pkt)) else {
                            continue;
                        };
                        // 发送失败 = TLS-write 任务已退出（write_rx drop）：真实掉线，
                        // 或 teardown 之后的尾包竞态。掉线信号统一由 read-channel 关闭
                        // 路径（on_read_channel_closed → Failed 事件）上报，这里不再发
                        // Warn 进 UI 日志——TLS 死后外圈循环在 stop 前每个出站包批都会
                        // 走到此处，曾造成该告警按包批刷屏（2026-09-05 移除）。保留
                        // stderr 供现场诊断。
                        if reader_tx.send(frame).is_err() {
                            eprintln!(
                                "[info] engine data-plane: CSTP write channel closed (TLS task exited)"
                            );
                            // writer 排空控制事件后报告首个 TLS 原因，避免这里抢先丢掉细节。
                            break;
                        }
                    }
                }
            }
        });

        // ---- writer 线程：read_channel（TLS）→ ring 计数 → session.send（ring）。 ----
        let writer_session = Arc::clone(&self.session);
        let tls_reader = Arc::clone(read_channel);
        let writer_stop_flag = Arc::clone(&writer_stop);
        let ring_sent_counter = Arc::clone(&ring_sent_bytes);
        let writer_stats = aux.stats.clone();
        let writer_latency = aux.latency.clone();
        let writer_control_rx = aux.control_rx.clone();
        let probe_tx = write_channel.clone();
        let writer_log = log.clone();
        // C2：掉线状态上报端点与关联 operation_id（read_channel 关闭时发布
        // Failed 事件；`None` 发布端 = 不接线，行为与 C2 之前一致）。
        let writer_operation_id = aux.operation_id.clone();
        // T3b（F6）：数据面掉线信号发射端（sweeper 清 live；`None` = 未接线）。
        let writer = std::thread::spawn(move || {
            let failure = writer_guard;
            if !wait_for_data_plane_activation(&writer_activated, &writer_stop_flag) { return; }
            // T1 latency probe：状态机（DPD→ping fallback + 手动刷新标记）。
            let mut probe = writer_latency.map(ProbeState::new);
            // F2：周期 CSTP keepalive（0x07 空 body）——帧只编码一次；节拍器初始化
            // `last = Some(线程启动时刻 ≈ 连接建立时刻)`，首帧在第 15s（对齐 darwin
            // 时序）。生命周期与 writer 线程同生共死，零新增关停管道。
            let keepalive_frame = Codec::encode_raw(CSTP_PACKET_TYPE_KEEPALIVE, &[])
                .expect("keepalive frame encodes within bounds");
            let mut keepalive = KeepaliveTicker::new(
                Duration::from_secs(CSTP_KEEPALIVE_INTERVAL_SECS),
                Instant::now(),
            );
            // F1：DPD 应答帧（0x04 空 body）——帧只编码一次。
            let dpd_response_frame = build_dpd_response_frame();
            // F5：未知控制 kind 去重表（每 kind 每会话至多 1 条 Warn）+ 会话终结
            // 原因 stash（掉线单条 Warn 携带）。
            let mut seen_unknown_kinds: Vec<u8> = Vec::new();
            let mut session_end: Option<SessionEndReason> = None;
            let mut diagnostics = DataPlaneDiagnostics::new();
            let mut exit_reason = "stop_requested";
            // 引擎 data 通道是 tokio unbounded receiver：轮询 `try_recv`（空则短睡，
            // 保持 stop 标志的可达性），收到解码后的 IP 包 → session.send。
            while !writer_stop_flag.load(Ordering::SeqCst) {
                // F5：排空 CSTP 控制面事件（drain——消除事件积压；1ms 轮询下无性能
                // 影响），按冻结分派表处置。
                if let Some(ctrl) = &writer_control_rx {
                    let mut guard = ctrl.lock().expect("control rx 锁");
                    while let Ok(event) = guard.try_recv() {
                        diagnostics.observe(&event);
                        if let CstpControlEvent::WriteFailed(error) = &event {
                            log_write_failure(&writer_log, &writer_operation_id, error);
                            diagnostics.emit(&writer_log, &writer_operation_id, "tls-write-failed");
                        }
                        match control_event_action(&event, &mut seen_unknown_kinds) {
                            EventAction::ProbeRtt => {
                                if let Some(probe) = probe.as_mut() {
                                    if let Some(stats) = &writer_stats {
                                        if let Some(rtt) = probe.on_dpd_response(Instant::now()) {
                                            stats.record_latency(rtt);
                                        }
                                    }
                                }
                            }
                            EventAction::ReplyDpd => {
                                // F1：无条件应答（与 DPD_PROBE_ENABLED 无关——这是
                                // 被动应答网关，不是主动探测）。失败 = TLS 写任务
                                // 已死 → stderr 单行诊断，不发状态事件、不重试；
                                // 掉线统一由 read-channel 关闭路径上报。
                                if probe_tx.send(dpd_response_frame.clone()).is_ok() {
                                    diagnostics.dpd_reply_queued += 1;
                                } else {
                                    diagnostics.control_queue_failures += 1;
                                    eprintln!(
                                        "[info] engine data-plane: DPD reply send failed (TLS write task exited)"
                                    );
                                }
                            }
                            EventAction::Ignore => {}
                            EventAction::WarnUnknownKind(kind) => {
                                if let Some(log) = &writer_log {
                                    log.emit(
                                        crate::log_sink::LogLevel::Warn,
                                        "engine",
                                        "tunnel.dataplane.unknown-control",
                                        "unknown CSTP control frame ignored (once per kind)",
                                        &[("kind", &format!("{kind:#04x}"))],
                                    );
                                }
                            }
                            EventAction::StashSessionEnd(reason) => {
                                // F5：stash，不单独成日志——掉线单条 Warn 携带。
                                session_end.get_or_insert(reason);
                            }
                        }
                    }
                }
                // 写侧任务结束也会使隧道不可用，不必等待读侧 EOF。
                if session_end.is_none() && probe_tx.is_closed() {
                    session_end = Some(SessionEndReason::Io(SessionIoError::new(
                        IoDirection::Write, &std::io::Error::from(std::io::ErrorKind::BrokenPipe),
                    )));
                }
                if session_end.is_some() {
                    exit_reason = "session_terminated";
                    failure.report("session_terminated", None, session_end.as_ref());
                    break;
                }
                // F2：周期 CSTP keepalive（无条件、不判空闲；15s × ~8 字节开销可
                // 忽略）。发送失败只 stderr 单行（同 F1 失败语义）。
                if keepalive.due(Instant::now()) {
                    if probe_tx.send(keepalive_frame.clone()).is_ok() {
                        diagnostics.keepalive_queued += 1;
                    } else {
                        diagnostics.control_queue_failures += 1;
                        eprintln!(
                            "[info] engine data-plane: keepalive send failed (TLS write task exited)"
                        );
                    }
                }
                // T1：探测节拍（周期 ping / 手动标记 / 超时回退）。
                if let Some(probe) = probe.as_mut() {
                    probe.tick(Instant::now(), &mut |frame: Vec<u8>| {
                        // 发送失败 = TLS-write 任务已退出（write_rx drop）→ 忽略（探测
                        // 请求是尽力而为，数据面仍继续）。
                        if probe_tx.send(frame).is_err() {
                            diagnostics.control_queue_failures += 1;
                        }
                    });
                }
                if diagnostics.last_summary.elapsed() >= Duration::from_secs(30) {
                    diagnostics.emit(&writer_log, &writer_operation_id, "periodic");
                }
                let pkt = match tls_reader.lock().expect("tls reader 锁").try_recv() {
                    Ok(pkt) => Some(pkt),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(1));
                        None
                    }
                    Err(_) => {
                        exit_reason = "read_channel_closed";
                        // read_channel 关闭 = TLS-read 任务已退出 → 先发数据面掉线
                        // 信号（sweeper 异步清 live，F6/S4），再 C2 掉线状态上报
                        // （Failed(DataPlane, RetrySameOperation)；自动重连前置）+
                        // 恰好一条带 reason 的 Warn（F5；reason 来自 SessionEnded
                        // stash，writer 先观察到关闭的理论罕见场景下省略原因字段）。
                        // TLS read 在最终控制事件入队后才关闭 data channel；再次排空
                        // 消除首次 drain 与关闭观察之间的竞态，避免丢失真实错误原因。
                        if let Some(ctrl) = &writer_control_rx {
                            while let Ok(event) = ctrl.lock().expect("control rx 锁").try_recv() {
                                diagnostics.observe(&event);
                                match event {
                                    CstpControlEvent::SessionEnded(reason) => {
                                        session_end.get_or_insert(reason);
                                    }
                                    CstpControlEvent::WriteFailed(error) => {
                                        log_write_failure(&writer_log, &writer_operation_id, &error)
                                    }
                                    _ => {}
                                }
                            }
                        }
                        failure.report("read_channel_closed", None, session_end.as_ref());
                        break;
                    }
                };
                if let Some(pkt) = pkt {
                    // T1：匹配的 ICMP echo reply → 算 ping RTT 写 registry（入站包
                    // 仍正常送进 ring）。
                    if let Some(probe) = probe.as_mut() {
                        if let Some(stats) = &writer_stats {
                            if let Some(rtt) = probe.on_icmp_echo_reply(&pkt, Instant::now()) {
                                stats.record_latency(rtt);
                            }
                        }
                    }
                    let sent = writer_session.lock().expect("session 锁").send(&pkt);
                    if sent.is_ok() {
                        count_writer_download(
                            &ring_sent_counter,
                            writer_stats.as_deref(),
                            pkt.len() as u64,
                        );
                    } else if let Err(error) = sent {
                        // 环满是单包背压；其它错误意味着 ring 已失效，不能继续显示在线。
                        if error.code != windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW.0 {
                            exit_reason = "wintun_send_failed";
                            failure.report(exit_reason, Some(error.code), None);
                            break;
                        }
                    }
                }
            }
            log_data_plane_lifecycle(
                &writer_log,
                &writer_operation_id,
                "tunnel.dataplane.writer-exit",
                exit_reason,
                session_end.as_ref(),
            );
            diagnostics.emit(&writer_log, &writer_operation_id, "writer-exit");
        });

        EngineDataPlaneThreads {
            activated,
            failure_reported,
            active_operation_id,
            reader_stop,
            writer_stop,
            reader: Some(reader),
            writer: Some(writer),
            ring_received_bytes,
            ring_sent_bytes,
        }
    }
}

/// engine 数据面线程句柄（DP-03）：ring→CSTP reader + CSTP→ring writer，直连 engine
/// session。
///
/// 停止语义（W17 SAFETY-ORDER）：`stop_and_join` 置位 stop → join 两个线程 → 再放掉
/// 线程持有的 `Arc<Mutex<WintunSession>>` 克隆。`Drop` 同序兜底——任何退出路径（含
/// 错误）都先 join 再放 session，绝不把未 join 的 reader 留在 `WintunEndSession` 之后
/// （0.14.1 EndSession 销毁 session 对象，之后 receive 是 UAF）。
pub struct EngineDataPlaneThreads {
    activated: Arc<AtomicBool>,
    failure_reported: Arc<AtomicBool>,
    active_operation_id: Arc<Mutex<Vec<u8>>>,
    /// reader 线程 stop 标志。
    reader_stop: Arc<AtomicBool>,
    /// writer 线程 stop 标志。
    writer_stop: Arc<AtomicBool>,
    /// reader 线程句柄（ring→CSTP→TLS）。
    reader: Option<std::thread::JoinHandle<()>>,
    /// writer 线程句柄（TLS→CSTP→ring）。
    writer: Option<std::thread::JoinHandle<()>>,
    /// ring 收到字节计数（engine 数据面计数器；证据读取，不再来自 helper）。
    ring_received_bytes: Arc<AtomicU64>,
    /// ring 发送字节计数（engine 数据面计数器；证据读取，不再来自 helper）。
    ring_sent_bytes: Arc<AtomicU64>,
}

impl EngineDataPlaneThreads {
    /// 只有 owner 安装会话并发布 Connected 后才允许工作线程处理包或报告失败。
    pub fn activate(&self) {
        self.activated.store(true, Ordering::Release);
    }

    /// 掉线已经被观察到时立即失去可复用性，不等待异步清理线程取走 live。
    pub fn is_alive(&self) -> bool {
        !self.failure_reported.load(Ordering::Acquire)
            && !self.reader_stop.load(Ordering::Acquire)
            && !self.writer_stop.load(Ordering::Acquire)
            && !self.reader.as_ref().is_some_and(|thread| thread.is_finished())
            && !self.writer.as_ref().is_some_and(|thread| thread.is_finished())
    }

    /// 复用同一活动会话时，后续掉线必须关联新受理的操作；与报失互斥。
    pub fn rebind_operation(&self, operation_id: &[u8]) -> bool {
        self.rebind_operation_with(operation_id, || {})
    }

    pub fn rebind_operation_with(&self, operation_id: &[u8], on_rebound: impl FnOnce()) -> bool {
        let mut current = self.active_operation_id.lock().unwrap_or_else(|error| error.into_inner());
        if !self.is_alive() { return false; }
        *current = operation_id.to_vec();
        on_rebound();
        true
    }

    /// 置位 stop → join 两个线程（W17 SAFETY-ORDER：先 join 再放 session，绝不把
    /// 未 join 的 reader 留在 `WintunEndSession` 之后）。返回
    /// `(ring_received_bytes, ring_sent_bytes)`——engine 数据面计数器，证据读取。
    ///
    /// **D15 有界 join**：join 是**有界**的——两线程各自以 ≤50ms（reader 事件等待 /
    /// writer 空转睡眠）轮询 stop 标志，置位后必在 50ms 内退出。**不能**在此处做
    /// 「2s 超时 detach 兜底」：detach 后立即放 session（`WintunEndSession`）会让
    /// 未 join 的 worker 在已销毁的 session 上 receive/send = UAF（W17 冻结）。超时
    /// 兜底 force-exit 属进程级退出编排（main.rs / 服务形态，D12「2s 清不掉则记录 +
    /// force-exit」），不在此模块内做。
    ///
    /// 幂等：重复调用 / `Drop` 兜底安全（线程句柄 `take()` 后为空）。
    #[must_use]
    pub fn stop_and_join(&mut self) -> (u64, u64) {
        self.reader_stop.store(true, Ordering::SeqCst);
        self.writer_stop.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        (
            self.ring_received_bytes.load(Ordering::SeqCst),
            self.ring_sent_bytes.load(Ordering::SeqCst),
        )
    }

    /// 当前累计 ring 收到字节（存活线程的实时计数器；`stop_and_join` 后不再变化）。
    #[must_use]
    pub fn ring_received(&self) -> u64 {
        self.ring_received_bytes.load(Ordering::SeqCst)
    }

    /// 当前累计 ring 发送字节（存活线程的实时计数器；`stop_and_join` 后不再变化）。
    #[must_use]
    pub fn ring_sent(&self) -> u64 {
        self.ring_sent_bytes.load(Ordering::SeqCst)
    }

    /// 计数器 Arc 克隆（`(ring_received_bytes, ring_sent_bytes)`）——供诊断线程
    /// 存活期轮询（不持有线程句柄，无 Send 约束）。
    #[must_use]
    pub fn counter_arcs(&self) -> (Arc<AtomicU64>, Arc<AtomicU64>) {
        (
            Arc::clone(&self.ring_received_bytes),
            Arc::clone(&self.ring_sent_bytes),
        )
    }
}

fn wait_for_data_plane_activation(activated: &AtomicBool, stop: &AtomicBool) -> bool {
    while !activated.load(Ordering::Acquire) {
        if stop.load(Ordering::Acquire) { return false; }
        std::thread::sleep(Duration::from_millis(1));
    }
    !stop.load(Ordering::Acquire)
}

impl Drop for EngineDataPlaneThreads {
    fn drop(&mut self) {
        // W17 兜底：任何退出路径（含错误）都先 join 再放 session Arc 克隆。
        self.reader_stop.store(true, Ordering::SeqCst);
        self.writer_stop.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// 包是否为 IPv4（L3 版本字段）。
#[must_use]
pub fn is_ipv4_packet(pkt: &[u8]) -> bool {
    pkt.first().is_some_and(|v| (v >> 4) == 4)
}

// ---------------------------------------------------------------------------
// C2 掉线感知：read_channel 关闭（TLS-read 任务退出）→ 写诊断日志 + 发布数据面
// 掉线 Failed 状态事件（自动重连前置；重连决策属 C3）。
// ---------------------------------------------------------------------------

/// 数据面掉线（连接建立后）的 wire 错误标记。
///
/// 标记方案（C2 决策：复用现有 wire 语义，不新增错误码——proto/domain 已有
/// `ErrorStage::DataPlane` 与 `RetryAdvice::RetrySameOperation`）：
/// - `code = EffectUnknown(13)`：同其它平台类失败（无更精确的既有码）；
/// - `stage = DataPlane(12)`：**区分关键**——现有连接期失败路径
///   （`tunnel_runtime::tunnel_error_to_wire`）一律 `stage=Ingress`，本事件是
///   唯一 `stage=DataPlane` 的来源，core 可据 `coarse=Failed + stage=DataPlane`
///   明确识别「连接建立后数据面掉线」；
/// - `retry = RetrySameOperation(2)`：可重试标记（现有失败路径一律
///   `DoNotRetry`，本事件是唯一 `RetrySameOperation` 来源）——C3 自动重连据此判定。
/// - `native = {Transport, Win32, 0}`：传输层掉线归属。
#[must_use]
fn data_plane_drop_error() -> generated::VpnError {
    generated::VpnError {
        code: generated::ErrorCode::EffectUnknown as i32,
        stage: generated::ErrorStage::DataPlane as i32,
        certainty: 0, // EffectCertainty::Unspecified（对齐现有 Failed 事件惯例）
        retry: generated::RetryAdvice::RetrySameOperation as i32,
        subject: None,
        resource: None,
        native: Some(generated::RedactedNativeError {
            category: generated::NativeErrorCategory::Transport as i32,
            namespace: generated::NativeErrorNamespace::Win32 as i32,
            code: 0,
        }),
    }
}

/// read_channel 关闭时的**完整掉线处置**（T3b 接线，F6+F5）：**先**发数据面掉线
/// 信号（sweeper 异步清 live，S4），**再**做既有掉线上报（恰好一条带 reason 的
/// Warn + stats Failed + C2 Failed 事件）。`notify` 为 `None`（未接线）时行为与
/// T3 之前一致（只做上报）。
///
/// 顺序契约：sweeper 清 live 与 writer 退出解耦（sweeper 异步），信号先于上报
/// 发出、不保证 sweeper 先完成——`is_connected()` 的新鲜度由 sweeper 收敛保证
/// （S4 → S5，计划 9.3 开放窗口如实记录）。writer 闭包持有的本信号发射端随线程
/// 退出 drop（P1-1 双份发送端安放之一；另一份在 EngineCstp/LiveTunnel）。
fn on_data_plane_lost(
    notify: &Option<Arc<dyn Fn() + Send + Sync>>,
    log: &Option<Arc<crate::log_sink::LogSink>>,
    status: &Option<Arc<StatusPublisher>>,
    operation_id: &[u8],
    stats: Option<&StatsRegistry>,
    reason: Option<&SessionEndReason>,
) {
    log_data_plane_lifecycle(
        log,
        operation_id,
        "tunnel.dataplane.loss-observed",
        "before_cleanup_notification",
        reason,
    );
    if let Some(notify) = notify {
        notify();
    }
    on_read_channel_closed(log, status, operation_id, stats, reason);
}

/// read_channel 关闭（TLS-read 任务已退出）时的处理：**恰好一条**带原因的 Warn +
/// stats phase 置 Failed + 数据面掉线状态上报。
///
/// 独立成函数以便单测——writer 线程整体需要真实 Wintun session 才能构造，而本
/// 掉线发布路径（日志 + `StatusEvent::failed` + stats）可不依赖数据面线程直接验证。
/// `status` 为 `None`（未接线）时保持 C2 之前行为（只写日志，不发布状态）；
/// `stats` 为 `None` 时跳过 phase 写入。
///
/// F5 冻结（2026-09-05 cstp-keepalive 计划）：
/// - Warn 每次掉线恰好一条，event key `tunnel.dataplane.read-channel-closed` 不变，
///   消息携带 `reason`（来自读任务 `SessionEnded` 事件的 stash）；stash 为空（理论
///   罕见：writer 先观察到 read_channel 关闭）时如实省略原因字段；
/// - `stats.set_phase(Failed)`——掉线窗口快照诚实化；
/// - `StatusEvent::failed(DataPlane, RetrySameOperation)` 原样保留（C2 契约不变）。
fn on_read_channel_closed(
    log: &Option<Arc<crate::log_sink::LogSink>>,
    status: &Option<Arc<StatusPublisher>>,
    operation_id: &[u8],
    stats: Option<&StatsRegistry>,
    reason: Option<&SessionEndReason>,
) {
    if let Some(log) = log {
        match reason {
            Some(reason) => {
                let label = session_end_reason_label(reason);
                let mut fields = diagnostic_identity(operation_id);
                fields.push(("reason", label.clone()));
                if let SessionEndReason::ServerDisconnect { kind, code, reason, body_len } = reason {
                    fields.push(("server_packet_type", format!("{kind:#04x}")));
                    fields.push(("server_reason_code", code.map_or_else(|| "none".into(), |code| code.to_string())));
                    fields.push(("server_reason", reason.clone()));
                    fields.push(("server_body_len", body_len.to_string()));
                }
                if let SessionEndReason::Io(error) = reason {
                    fields.extend(io_error_fields(error));
                }
                let refs: Vec<_> = fields
                    .iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect();
                log.emit(
                    crate::log_sink::LogLevel::Warn,
                    "engine",
                    "tunnel.dataplane.read-channel-closed",
                    &format!("CSTP read channel closed (TLS task exited; reason={label})"),
                    &refs,
                );
            }
            None => {
                let fields = diagnostic_identity(operation_id);
                let refs: Vec<_> = fields
                    .iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect();
                log.emit(
                    crate::log_sink::LogLevel::Warn,
                    "engine",
                    "tunnel.dataplane.read-channel-closed",
                    "CSTP read channel closed (TLS task exited)",
                    &refs,
                );
            }
        }
    }
    if let Some(stats) = stats {
        // 掉线窗口快照诚实化：stats phase = Failed（快照不再报告 Connected）。
        stats.set_phase(generated::StatsPhase::Failed);
    }
    if let Some(status) = status {
        status.publish(StatusEvent::failed(
            operation_id.to_vec(),
            ConnectPhase::StartingDataPlane,
            data_plane_drop_error(),
        ));
    }
}

// ---------------------------------------------------------------------------
// 单元测试：纯函数 `is_ipv4_packet` + 线程句柄计数器访问器（无需真实 Wintun
// adapter；真实数据面线程的 W17 顺序由集成路径覆盖）。
// ---------------------------------------------------------------------------
