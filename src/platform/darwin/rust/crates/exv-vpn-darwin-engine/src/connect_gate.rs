//! S2 连接门预检与启动清扫（darwin engine；2026-10-26 验收修复计划 §S2）。
//!
//! 事故链背景：engine 建隧道写 `/etc/resolver/<校园域>`（root 属主）→ SIGKILL 后
//! 孤儿 → 全系统 DNS 死路 → core 探测超时被合流为凭据缺失。本模块按计划恢复
//! 条件驱动预检（推翻 2026-10-21 撤除决定），新形态满足当年撤除批评的三点：
//! **条件触发、扫描有界、报告有真实消费者**（稳定 reason id + 阻断码 + 状态/
//! 日志事件 + 文件日志底账）。
//!
//! 结构（对齐 win32 `connect_gate.rs` 模型）：
//! - **痕迹两件套**（engine 唯一读写者；命名与字段沿用被删 `trace.rs`/版本标记
//!   原设计）：`/Library/Application Support/EXV/Engine/<uid>/trace.v1.json`
//!   （`TraceV1` 字段）+ `/Library/Application Support/EXV/Engine/version.v1`
//!   （机器级版本标记；缺失/损坏/读失败=版本未知，保守触发）。
//! - **三触发源**：连接痕迹文件存在性 ‖ 版本标记变更 ‖ 进程首连（进程内
//!   `AtomicBool`；同代次只跑一轮）。
//! - **三域有界预检**（总预算 [`PREFLIGHT_BUDGET`]≤3s）：managed resolver 文件 /
//!   死接口孤儿路由（netstat 路由表 × 存活 utun 接口集）/ `/private/tmp`
//!   `exv-vpn-*` 孤儿目录（形状规则复用服务代理 `sweep_runtime_residue` 的
//!   匹配规则）。发现→清→复验。
//! - **失败分级**：resolver/路由域清理失败→稳定码
//!   `DARWIN_CORE_PREFLIGHT_BLOCKED|<reason_ids>` 阻断；目录域失败仅记日志继续
//!   （对齐 win32「NRPT 降级不阻断」的分级思路）。
//!
//! 治理边界：不建穷举门禁——探测超时按 win32 语义「Timeout 域不计阻断」（无法
//! 完成观察不等于发现残留），且超时不重写版本标记（半途预检下次连接重试，
//! 保守方向）。外来 resolver 文件只报不删（与安装冲突裁决同一纪律）。

use std::{
    io,
    net::Ipv4Addr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::campus_resolver::{self, CommandRunner};
use crate::log_sink::LogSink;
use crate::platform::route::AppliedRoute;

/// 痕迹与版本标记的生产根目录（沿用被删 `trace.rs` 的 `TRACE_ROOT` 原设计）。
pub(crate) const TRACE_ROOT: &str = "/Library/Application Support/EXV/Engine";

/// 每用户连接痕迹文件名（沿用 `trace.rs` 的 `TRACE_FILE` 原设计）。
const TRACE_FILE: &str = "trace.v1.json";

/// 机器级版本标记文件名（沿用被删 `preflight.rs` 的 `version.v1` 原设计）。
const VERSION_MARKER_FILE: &str = "version.v1";

/// 预检阻断稳定码前缀；完整 message 形态
/// `DARWIN_CORE_PREFLIGHT_BLOCKED|<id>[,<id>…]|<摘要>`（对齐 win32
/// `preflight_blocked|<ids>` 形态；core 侧 `engine_session_status` 对
/// `failed_precondition` 原样透传，前端按稳定码路由文案）。
pub(crate) const PREFLIGHT_BLOCKED_CODE: &str = "DARWIN_CORE_PREFLIGHT_BLOCKED";

/// resolver 域清理动作失败的稳定 reason id。
const REASON_RESOLVER_CLEANUP_FAILED: &str = "resolver_cleanup_failed";
/// resolver 域复验仍有自有残留的稳定 reason id。
const REASON_RESOLVER_CLEANUP_UNRESOLVED: &str = "resolver_cleanup_unresolved";
/// 路由域清理动作失败的稳定 reason id。
const REASON_ROUTE_CLEANUP_FAILED: &str = "route_cleanup_failed";
/// 路由域复验仍有孤儿路由的稳定 reason id。
const REASON_ROUTE_CLEANUP_UNRESOLVED: &str = "route_cleanup_unresolved";

/// 预检总预算（win32 `PREFLIGHT_BUDGET_TARGET` 同款 3s 上界）。
const PREFLIGHT_BUDGET: Duration = Duration::from_secs(3);

/// runtime 孤儿目录的固定父目录（服务代理 `RUNTIME_RESIDUE_PARENT` 同款）。
const RUNTIME_RESIDUE_PARENT: &str = "/private/tmp";
/// runtime 目录名 `exv-vpn-<pid:x>-<16 hex>` 的 pid hex 位数上界（下界 1 位）。
const RUNTIME_RESIDUE_PID_HEX_MAX: usize = 8;
/// runtime 目录内允许被 unlink 的已知 leaf；其余条目一律保留现场
/// （服务代理 `RUNTIME_RESIDUE_KNOWN_LEAVES` 同款；两处刻意各自实现——
/// engine 不依赖 service-agent crate，与服务代理/壳层卸载的既有同源关系一致）。
const RUNTIME_RESIDUE_KNOWN_LEAVES: [&str; 3] = ["engine.sock", "engine.ticket", "control.sock"];

/// 命令执行源（netstat/ifconfig/killall/dscacheutil；生产=既有
/// [`campus_resolver::process_command_runner`]，测试注入假实现）。
type CommandSource = Arc<dyn Fn(&str, &[&str]) -> io::Result<String> + Send + Sync>;

/// 孤儿路由删除源（生产=既有 `platform::route::delete`，测试注入假实现；
/// 错误字符串化保持报告非秘密）。
type RouteDelete = Arc<dyn Fn(&AppliedRoute) -> Result<(), String> + Send + Sync>;

/// 文件日志转发闭包（形态对齐 [`crate::file_log::FileLog::log`]；生产转发
/// 进程级 `default_log()`，测试捕获注入——避免阻塞扫描任务借用测试局部）。
type FileLogFn = Arc<dyn Fn(&str, &str, &str, &str, &[(&str, String)]) + Send + Sync>;

/// 生产的文件日志转发（S7 设施；component 由调用方传入）。
fn production_file_log() -> FileLogFn {
    Arc::new(|level, component, code, message, fields| {
        crate::file_log::default_log().log(level, component, code, message, fields);
    })
}

/// 生产的命令执行源：复用 resolver 模块的生产执行器（含 euid=0 归一化）。
fn production_command() -> CommandSource {
    Arc::new(campus_resolver::process_command_runner)
}

/// 生产的路由删除源：复用既有 PF_ROUTE 删除。
fn production_route_delete() -> RouteDelete {
    Arc::new(|route: &AppliedRoute| {
        crate::platform::route::delete(route).map_err(|error| error.to_string())
    })
}

// ---------------------------------------------------------------------------
// 痕迹两件套：连接痕迹（per-uid）+ 版本标记（机器级）。
// ---------------------------------------------------------------------------

/// 痕迹状态（沿用被删 `trace.rs` 的 `TraceState` 原设计；最小形态只写 Connecting，
/// 存在性即触发信号）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TraceState {
    Connecting,
    Connected,
    Terminal,
}

/// 单条连接痕迹（v1 schema；字段沿用被删 `trace.rs` 的 `TraceV1` 原设计——
/// 只记非秘密事实，凭据/会话材料绝不入痕）。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TraceV1 {
    schema: u32,
    state: TraceState,
    owner_uid: u32,
    mode: String,
    server: String,
    username: String,
    updated_at_ms: i64,
    terminal: Option<String>,
    teardown_verified: bool,
}

const TRACE_SCHEMA_VERSION: u32 = 1;

/// owner 的痕迹目录（engine 唯一写者；0700 root 属主——最小形态不 chown 给
/// 用户，痕迹是 engine 内部触发信号，无用户侧读者）。
fn trace_dir(base: &Path, owner_uid: u32) -> PathBuf {
    base.join(owner_uid.to_string())
}

fn trace_path(base: &Path, owner_uid: u32) -> PathBuf {
    trace_dir(base, owner_uid).join(TRACE_FILE)
}

/// 是否存在残留痕迹（预检触发信号；双亡格兜底——engine 被杀时无人删痕）。
fn trace_exists(base: &Path, owner_uid: u32) -> bool {
    trace_path(base, owner_uid).exists()
}

/// 受理即留痕：写 Connecting 态整文件原子替换（0600）。
fn write_trace(
    base: &Path,
    owner_uid: u32,
    server: &str,
    username: &str,
    mode: &str,
    file_log: &FileLogFn,
) {
    let record = TraceV1 {
        schema: TRACE_SCHEMA_VERSION,
        state: TraceState::Connecting,
        owner_uid,
        mode: mode.to_owned(),
        server: server.to_owned(),
        username: username.to_owned(),
        updated_at_ms: now_epoch_ms(),
        terminal: None,
        teardown_verified: false,
    };
    let Ok(payload) = serde_json::to_vec(&record) else {
        return;
    };
    let dir = trace_dir(base, owner_uid);
    let target = dir.join(TRACE_FILE);
    let temp = dir.join(format!("{TRACE_FILE}.tmp"));
    let written = (|| -> io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        // umask 不可靠（默认 022 下 create_dir_all 产物 0755）——显式 0700/0600。
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(&temp, &payload)?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&temp, &target)
    })();
    match written {
        Ok(()) => file_log(
            "info",
            "preflight",
            "TRACE_WRITTEN",
            "connect trace written at admission",
            &[
                ("action", "trace_write".to_owned()),
                ("result", "ok".to_owned()),
            ],
        ),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            file_log(
                "warn",
                "preflight",
                "TRACE_WRITE_FAILED",
                "connect trace write failed; gate trigger falls back to version/first-connect",
                &[
                    ("action", "trace_write".to_owned()),
                    ("result", "failed".to_owned()),
                    ("reason", error.to_string()),
                ],
            );
        }
    }
}

/// 删除痕迹文件（触发信号解除；目录保留——下次受理复用）。
fn delete_trace(base: &Path, owner_uid: u32, action: &str, file_log: &FileLogFn) {
    let path = trace_path(base, owner_uid);
    let outcome = if path.exists() {
        std::fs::remove_file(&path)
            .map(|()| "ok".to_owned())
            .map_err(|error| error.to_string())
    } else {
        Ok("absent".to_owned())
    };
    let (result, reason) = match outcome {
        Ok(result) => (result, String::new()),
        Err(reason) => ("failed".to_owned(), reason),
    };
    file_log(
        if result == "failed" { "warn" } else { "info" },
        "preflight",
        "TRACE_DELETED",
        "connect trace deleted",
        &[
            ("action", action.to_owned()),
            ("result", result),
            ("reason", reason),
        ],
    );
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(0)
        })
}

/// 版本标记是否变化（触发源：不同/缺失/损坏/读失败=版本未知，均保守触发——
/// win32 `version_marker` 同款语义；内容=当前构建版本）。
fn version_marker_changed(base: &Path, current_version: &str) -> bool {
    match std::fs::read_to_string(base.join(VERSION_MARKER_FILE)) {
        Ok(existing) => existing.trim() != current_version,
        Err(_) => true,
    }
}

/// 重写版本标记（预检完整跑完且干净后调用；temp+rename 原子替换）。
fn rewrite_version_marker(base: &Path, current_version: &str, file_log: &FileLogFn) {
    let marker = base.join(VERSION_MARKER_FILE);
    let temp = base.join(format!("{VERSION_MARKER_FILE}.tmp"));
    let outcome = (|| -> io::Result<()> {
        std::fs::create_dir_all(base)?;
        std::fs::write(&temp, current_version)?;
        std::fs::rename(&temp, &marker)
    })();
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(&temp);
        file_log(
            "warn",
            "preflight",
            "VERSION_MARKER_WRITE_FAILED",
            "version marker rewrite failed; next connect will re-run preflight (conservative)",
            &[
                ("action", "version_marker_write".to_owned()),
                ("result", "failed".to_owned()),
                ("reason", error.to_string()),
            ],
        );
    }
}

// ---------------------------------------------------------------------------
// 三域扫描与清理（纯输入注入；动作真实执行）。
// ---------------------------------------------------------------------------

/// 预检报告（三域计数与失败清单；只含非秘密事实）。
#[derive(Default, Debug)]
struct PreflightReport {
    resolver_removed: Vec<String>,
    resolver_foreign_kept: usize,
    resolver_removal_failed: Vec<String>,
    /// 复验后仍有自有 managed 文件。
    resolver_unresolved: bool,
    orphan_routes_found: usize,
    orphan_routes_removed: usize,
    route_delete_failed: Vec<String>,
    /// 复验后仍有孤儿路由。
    route_unresolved: bool,
    dirs_removed: usize,
    dirs_kept_unknown: usize,
    dir_failures: Vec<String>,
}

impl PreflightReport {
    /// 阻断 reason id 清单（稳定 id；顺序固定便于断言与文案路由）。
    fn blocking_reason_ids(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if !self.resolver_removal_failed.is_empty() {
            reasons.push(REASON_RESOLVER_CLEANUP_FAILED);
        }
        if self.resolver_unresolved {
            reasons.push(REASON_RESOLVER_CLEANUP_UNRESOLVED);
        }
        if !self.route_delete_failed.is_empty() {
            reasons.push(REASON_ROUTE_CLEANUP_FAILED);
        }
        if self.route_unresolved {
            reasons.push(REASON_ROUTE_CLEANUP_UNRESOLVED);
        }
        reasons
    }

    /// 人读摘要（落日志与阻断 message 尾段；非秘密）。
    fn summary(&self) -> String {
        format!(
            "resolver removed={} kept_foreign={} removal_failed={} unresolved={} ; routes found={} removed={} delete_failed={} unresolved={} ; dirs removed={} kept_unknown={} failed={}",
            self.resolver_removed.len(),
            self.resolver_foreign_kept,
            self.resolver_removal_failed.len(),
            self.resolver_unresolved,
            self.orphan_routes_found,
            self.orphan_routes_removed,
            self.route_delete_failed.len(),
            self.route_unresolved,
            self.dirs_removed,
            self.dirs_kept_unknown,
            self.dir_failures.len(),
        )
    }
}

/// 三域扫描+清理+复验（blocking 上下文执行；全部输入注入，无借用逃逸）。
struct PreflightScanInputs {
    owner_uid: u32,
    resolver_dir: PathBuf,
    tmp_base: PathBuf,
    command: CommandSource,
    route_delete: RouteDelete,
    file_log: FileLogFn,
}

impl PreflightScanInputs {
    fn runner(&self) -> CommandRunner<'_> {
        // Arc<dyn Fn> 解引用为生产/测试共用的 CommandRunner 形态。
        &*self.command
    }
}

/// 执行三域预检（发现→清→复验）；动作落文件日志（component=preflight）。
fn run_preflight_scan(inputs: PreflightScanInputs) -> PreflightReport {
    let mut report = PreflightReport::default();
    // 域一：managed resolver 文件（仅删带标记文件；外来跳过；复用既有 HUP+flush）。
    let logs = LogSink::new(); // 域内通知失败的诊断 sink（不接会话流；底账走 file_log）。
    let resolver =
        campus_resolver::reclaim_managed_files(&inputs.resolver_dir, inputs.runner(), &logs);
    for domain in &resolver.removed {
        (inputs.file_log)(
            "info",
            "preflight",
            "RESOLVER_RECLAIMED",
            "managed resolver residue reclaimed",
            &[
                ("action", "resolver_reclaim".to_owned()),
                ("target", domain.clone()),
                ("result", "removed".to_owned()),
            ],
        );
    }
    for (domain, error) in &resolver.removal_failed {
        (inputs.file_log)(
            "warn",
            "preflight",
            "RESOLVER_RECLAIM_FAILED",
            "managed resolver residue removal failed; blocking this connect",
            &[
                ("action", "resolver_reclaim".to_owned()),
                ("target", domain.clone()),
                ("result", "failed".to_owned()),
                ("reason", error.to_string()),
            ],
        );
    }
    report.resolver_removed = resolver.removed;
    report.resolver_foreign_kept = resolver.foreign_kept.len();
    report.resolver_removal_failed = resolver
        .removal_failed
        .iter()
        .map(|(domain, _)| domain.clone())
        .collect();
    // 复验：目录内不应再有 managed 文件（复扫按同一归属裁决）。
    report.resolver_unresolved = managed_resolver_count(&inputs.resolver_dir) > 0;

    // 域二：死接口孤儿路由（netstat 路由表 × ifconfig 存活接口集）。
    let live = live_utun_interfaces(inputs.runner());
    let netstat_text = (inputs.command)("netstat", &["-rn", "-f", "inet"]).unwrap_or_default();
    let orphans = parse_orphan_routes(&netstat_text, &live);
    report.orphan_routes_found = orphans.len();
    for orphan in &orphans {
        let target = format!(
            "{}/{} via {}",
            orphan.destination, orphan.prefix, orphan.gateway
        );
        match (inputs.route_delete)(orphan) {
            Ok(()) => {
                report.orphan_routes_removed += 1;
                (inputs.file_log)(
                    "info",
                    "preflight",
                    "ORPHAN_ROUTE_REMOVED",
                    "orphan route to vanished utun deleted",
                    &[
                        ("action", "orphan_route_delete".to_owned()),
                        ("target", target),
                        ("result", "removed".to_owned()),
                    ],
                );
            }
            Err(error) => {
                report.route_delete_failed.push(target.clone());
                (inputs.file_log)(
                    "warn",
                    "preflight",
                    "ORPHAN_ROUTE_DELETE_FAILED",
                    "orphan route delete failed; blocking this connect",
                    &[
                        ("action", "orphan_route_delete".to_owned()),
                        ("target", target),
                        ("result", "failed".to_owned()),
                        ("reason", error),
                    ],
                );
            }
        }
    }
    // 复验：重扫路由表，指向已消失 utun 的条目应已消失。
    if !orphans.is_empty() {
        let live_after = live_utun_interfaces(inputs.runner());
        let netstat_after = (inputs.command)("netstat", &["-rn", "-f", "inet"]).unwrap_or_default();
        report.route_unresolved = !parse_orphan_routes(&netstat_after, &live_after).is_empty();
    }

    // 域三：/private/tmp 孤儿 runtime 目录（形状匹配 + 属主 + 探活；失败仅记日志）。
    reclaim_stale_runtime_dirs(
        &inputs.tmp_base,
        inputs.owner_uid,
        &mut report,
        &inputs.file_log,
    );
    report
}

/// 目录内 managed resolver 文件计数（复验用；读取失败按 0——清理动作结果由
/// 逐文件报告承担，复验只回答「还有没有」）。
fn managed_resolver_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_file()
                && campus_resolver::classify_resolver_file(&path)
                    == campus_resolver::ResolverFileKind::Managed
        })
        .count()
}

/// 当前在场的 utun 接口名集合（ifconfig 输出解析；探测只读）。
fn live_utun_interfaces(runner: CommandRunner<'_>) -> Vec<String> {
    runner("ifconfig", &[])
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split(':').next())
                .filter(|name| name.starts_with("utun"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 `netstat -rn -f inet` 输出中指向**已消失 utun 接口**的路由（孤儿）。
/// 纯函数（解析器与执行分离；夹具文本测试）。
fn parse_orphan_routes(netstat_text: &str, live_interfaces: &[String]) -> Vec<AppliedRoute> {
    let live: std::collections::HashSet<&str> =
        live_interfaces.iter().map(String::as_str).collect();
    let mut orphans = Vec::new();
    for line in netstat_text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // 目的 网关 标志 Refs Use Netif Expire → 至少 6 列且 Netif 是 utunN。
        if fields.len() < 6 || !fields[5].starts_with("utun") {
            continue;
        }
        if live.contains(fields[5]) {
            continue;
        }
        let Some(destination) = parse_destination(fields[0]) else {
            continue;
        };
        let Some(gateway) = parse_gateway(fields[1]) else {
            continue;
        };
        // ifindex：活接口可解析；已消失接口为 0（PF_ROUTE 删除按 dst/gw 匹配）。
        let ifindex = if_nametoindex(fields[5]).unwrap_or(0);
        orphans.push(AppliedRoute {
            destination,
            prefix: prefix_of(fields[0]),
            gateway,
            ifindex,
        });
    }
    orphans
}

fn parse_destination(field: &str) -> Option<Ipv4Addr> {
    let base = field.split('/').next()?;
    // netstat 缩写尾零（"202.120.80" = "202.120.80.0"；裸主机地址不缩写）。
    let octets = base.split('.').count();
    if !(1..=4).contains(&octets) {
        return None;
    }
    let padded = if octets < 4 {
        format!("{base}{}", ".0".repeat(4 - octets))
    } else {
        base.to_owned()
    };
    padded.parse().ok()
}

fn prefix_of(field: &str) -> u8 {
    if let Some(prefix) = field.split('/').nth(1) {
        return prefix.parse().unwrap_or(32);
    }
    // netstat 自然掩码缩写——无 / 前缀时按八位组数推类（"202.120.80"=3 组→/24）。
    match field.split('.').count() {
        3 => 24,
        2 => 16,
        1 => 8,
        _ => 32,
    }
}

fn parse_gateway(field: &str) -> Option<Ipv4Addr> {
    // 链路层网关（link#N 接口形态）不是隧道网关——跳过。
    field.parse().ok()
}

fn if_nametoindex(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    // SAFETY: 只读取传入接口名；返回值是内核提供的索引。
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    (index != 0).then_some(index)
}

/// runtime 孤儿目录形状判定（服务代理 `runtime_residue_name_matches` 同款：
/// `exv-vpn-<pid:x 1-8 位小写 hex>-<16 位小写 hex>`）。
fn runtime_residue_name_matches(leaf: &str) -> bool {
    let Some(rest) = leaf.strip_prefix("exv-vpn-") else {
        return false;
    };
    let Some((pid_hex, suffix)) = rest.split_once('-') else {
        return false;
    };
    let is_lower_hex = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    (1..=RUNTIME_RESIDUE_PID_HEX_MAX).contains(&pid_hex.len())
        && suffix.len() == 16
        && is_lower_hex(pid_hex)
        && is_lower_hex(suffix)
}

/// guarded 回收单个 stale runtime 目录：只摘已知 leaf，然后 rmdir——非空即保留
/// 现场（绝不递归删除未知内容）；目录域失败仅记日志（不阻断）。
fn reclaim_stale_runtime_dir(dir: &Path) -> Result<(), String> {
    for leaf in RUNTIME_RESIDUE_KNOWN_LEAVES {
        let leaf_path = dir.join(leaf);
        if leaf_path.exists()
            && let Err(error) = std::fs::remove_file(&leaf_path)
        {
            return Err(format!("remove {}: {error}", leaf_path.display()));
        }
    }
    std::fs::remove_dir(dir).map_err(|error| format!("rmdir {}: {error}", dir.display()))
}

/// 域三：枚举并 guarded 回收本 owner 的死 runtime 目录（socket 探活可连=活引擎
/// 持有，跳过）；未知叶保留现场并计数。
fn reclaim_stale_runtime_dirs(
    tmp_base: &Path,
    owner_uid: u32,
    report: &mut PreflightReport,
    file_log: &FileLogFn,
) {
    let Ok(entries) = std::fs::read_dir(tmp_base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !runtime_residue_name_matches(name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_dir() || metadata.uid() != owner_uid {
            continue;
        }
        let socket = path.join("engine.sock");
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            // 探活可连=仍有引擎进程监听（它随后自行收敛清理）。
            continue;
        }
        // 目录内含未知条目 → 保守保留现场（只计数，不递归删除）。
        let has_unknown_leaf = path
            .read_dir()
            .map(|entries| {
                entries.flatten().any(|leaf| {
                    !RUNTIME_RESIDUE_KNOWN_LEAVES
                        .contains(&leaf.file_name().to_string_lossy().as_ref())
                })
            })
            .unwrap_or(true);
        if has_unknown_leaf {
            report.dirs_kept_unknown += 1;
            file_log(
                "info",
                "preflight",
                "RUNTIME_DIR_KEPT",
                "runtime residue dir kept (unknown leaves present)",
                &[
                    ("action", "runtime_dir_reclaim".to_owned()),
                    ("target", path.display().to_string()),
                    ("result", "kept".to_owned()),
                    ("reason", "unknown_leaves".to_owned()),
                ],
            );
            continue;
        }
        let target = path.display().to_string();
        match reclaim_stale_runtime_dir(&path) {
            Ok(()) => {
                report.dirs_removed += 1;
                file_log(
                    "info",
                    "preflight",
                    "RUNTIME_DIR_REMOVED",
                    "stale runtime residue dir reclaimed",
                    &[
                        ("action", "runtime_dir_reclaim".to_owned()),
                        ("target", target),
                        ("result", "removed".to_owned()),
                    ],
                );
            }
            Err(error) => {
                report.dir_failures.push(target.clone());
                file_log(
                    "warn",
                    "preflight",
                    "RUNTIME_DIR_RECLAIM_FAILED",
                    "stale runtime dir reclaim failed (non-blocking)",
                    &[
                        ("action", "runtime_dir_reclaim".to_owned()),
                        ("target", target),
                        ("result", "failed".to_owned()),
                        ("reason", error),
                    ],
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 连接门。
// ---------------------------------------------------------------------------

/// 门受理结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateAdmission {
    /// 无触发源——直接受理（痕迹已写）。
    Fresh,
    /// 触发源命中且预检干净/超时放行——受理（痕迹已重写；版本标记在完整跑完
    /// 且干净时重写）。
    FreshAfterPreflight {
        triggers: Vec<&'static str>,
        /// 预检是否完整跑完（false=预算超时放行——未验证，标记未重写）。
        verified: bool,
    },
}

/// 门拒绝（结构化稳定 id；message 形态 `DARWIN_CORE_PREFLIGHT_BLOCKED|<ids>|<摘要>`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateRejection {
    pub(crate) reason_ids: Vec<&'static str>,
    pub(crate) summary: String,
}

impl GateRejection {
    /// 阻断的 wire message（core 对 `failed_precondition` 原样透传）。
    pub(crate) fn message(&self) -> String {
        format!(
            "{PREFLIGHT_BLOCKED_CODE}|{}|{}",
            self.reason_ids.join(","),
            self.summary
        )
    }
}

/// 连接门：痕迹两件套载体 + 三触发源 + 三域有界预检。
///
/// 进程级单例经 [`process_gate`]（service engine 单 owner 假设；`owner_uid` 按
/// 每次调用传入，痕迹路径按 owner 分目录）。测试经 [`ConnectGate::with_layout`]
/// 注入临时目录与假执行源。
pub(crate) struct ConnectGate {
    trace_root: PathBuf,
    resolver_dir: PathBuf,
    tmp_base: PathBuf,
    version: String,
    budget: Duration,
    preflight_done: AtomicBool,
    command: CommandSource,
    route_delete: RouteDelete,
    file_log: FileLogFn,
    /// 生产门只在 euid=0（真实 engine 形态）激活；非 root 环境（单测）全部
    /// 动作让位——预检/清扫不触真实系统路径（对齐旧预检 C8 测试卫生）。
    active: bool,
}

impl ConnectGate {
    /// 生产布局（固定路径 + 既有命令/路由执行器 + S7 文件日志）。
    #[must_use]
    pub(crate) fn new_production(version: &'static str) -> Self {
        // SAFETY: geteuid 只读取当前进程凭证。
        let active = unsafe { libc::geteuid() } == 0;
        Self::with_layout(
            PathBuf::from(TRACE_ROOT),
            PathBuf::from(campus_resolver::RESOLVER_DIR),
            PathBuf::from(RUNTIME_RESIDUE_PARENT),
            version.to_owned(),
            PREFLIGHT_BUDGET,
            production_command(),
            production_route_delete(),
            production_file_log(),
        )
        .with_root_activation(active)
    }

    /// 激活位覆盖（生产构造按 euid 判定；测试布局恒激活）。
    fn with_root_activation(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// 可注入布局（单测：临时目录 + 假命令/假路由删除 + 捕获式文件日志）。
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_layout(
        trace_root: PathBuf,
        resolver_dir: PathBuf,
        tmp_base: PathBuf,
        version: String,
        budget: Duration,
        command: CommandSource,
        route_delete: RouteDelete,
        file_log: FileLogFn,
    ) -> Self {
        Self {
            trace_root,
            resolver_dir,
            tmp_base,
            version,
            budget,
            preflight_done: AtomicBool::new(false),
            command,
            route_delete,
            file_log,
            active: true,
        }
    }

    /// 评估触发源（痕外三源；返回命中 id 清单，空=无）。
    fn triggers(&self, owner_uid: u32) -> Vec<&'static str> {
        let mut triggers = Vec::new();
        if trace_exists(&self.trace_root, owner_uid) {
            triggers.push("stale_trace");
        }
        if version_marker_changed(&self.trace_root, &self.version) {
            triggers.push("version_upgrade");
        }
        if !self.preflight_done.load(Ordering::Acquire) {
            triggers.push("engine_first_connect");
        }
        triggers
    }

    /// 连接受理门（ApplyTunnel 受理点调用——先于任何系统变更）：
    /// 无触发源→受理即留痕；触发源命中→三域有界预检（发现→清→复验），
    /// resolver/路由域清不掉以 [`PREFLIGHT_BLOCKED_CODE`] 阻断（什么都不写——
    /// 不落会误导下次预检的新痕，遗留痕原样保留）；干净→删旧痕（预检复验代理
    /// 善后验证）→重写版本标记→落新痕。
    ///
    /// # Errors
    ///
    /// 预检未决发现（resolver/路由域清理失败或复验未收敛）→ [`GateRejection`]。
    pub(crate) async fn admit_connect(
        &self,
        owner_uid: u32,
        server: &str,
        username: &str,
        mode: &str,
        logs: &LogSink,
    ) -> Result<GateAdmission, GateRejection> {
        if !self.active {
            // 非 root（单测/非生产形态）：门让位，连接路径维持既有行为。
            return Ok(GateAdmission::Fresh);
        }
        let stale_trace = trace_exists(&self.trace_root, owner_uid);
        let triggers = self.triggers(owner_uid);
        if triggers.is_empty() {
            write_trace(
                &self.trace_root,
                owner_uid,
                server,
                username,
                mode,
                &self.file_log,
            );
            return Ok(GateAdmission::Fresh);
        }
        logs.publish(
            "info",
            "preflight",
            "PREFLIGHT_STARTED",
            "connect gate preflight triggered (stale residue possible)",
            &[("triggers", triggers.join(","))],
        );
        (self.file_log)(
            "info",
            "preflight",
            "PREFLIGHT_STARTED",
            "connect gate preflight started",
            &[
                ("action", "preflight_run".to_owned()),
                ("target", triggers.join(",")),
                ("result", "started".to_owned()),
            ],
        );
        let scan = PreflightScanInputs {
            owner_uid,
            resolver_dir: self.resolver_dir.clone(),
            tmp_base: self.tmp_base.clone(),
            command: Arc::clone(&self.command),
            route_delete: Arc::clone(&self.route_delete),
            file_log: Arc::clone(&self.file_log),
        };
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            self.budget,
            tokio::task::spawn_blocking(run_preflight_scan_inputs(scan)),
        )
        .await;
        // 触发源已消费（win32 语义：跑了即算，不因结果重复跑）。
        self.preflight_done.store(true, Ordering::Release);
        let verified = matches!(outcome, Ok(Ok(_)));
        match outcome {
            Ok(Ok(report)) => {
                let reasons = report.blocking_reason_ids();
                if !reasons.is_empty() {
                    logs.publish(
                        "warn",
                        "preflight",
                        "PREFLIGHT_BLOCKED",
                        "preflight found unresolved residue; connect blocked",
                        &[
                            ("reason_ids", reasons.join(",")),
                            ("triggers", triggers.join(",")),
                            ("elapsed_ms", started.elapsed().as_millis().to_string()),
                        ],
                    );
                    (self.file_log)(
                        "warn",
                        "preflight",
                        "PREFLIGHT_BLOCKED",
                        "preflight found unresolved residue; connect blocked",
                        &[
                            ("action", "preflight_run".to_owned()),
                            ("result", "blocked".to_owned()),
                            ("reason", reasons.join(",")),
                        ],
                    );
                    return Err(GateRejection {
                        reason_ids: reasons,
                        summary: report.summary(),
                    });
                }
                logs.publish(
                    "info",
                    "preflight",
                    "PREFLIGHT_PASSED",
                    "preflight clean; residue reclaimed and verified",
                    &[
                        ("triggers", triggers.join(",")),
                        (
                            "resolver_removed",
                            report.resolver_removed.len().to_string(),
                        ),
                        ("routes_removed", report.orphan_routes_removed.to_string()),
                        ("dirs_removed", report.dirs_removed.to_string()),
                        ("elapsed_ms", started.elapsed().as_millis().to_string()),
                    ],
                );
                // 完整跑完且干净 → 版本标记消费 + 遗留痕删除权（预检复验代理
                // 善后验证）；半途/超时路径两者都不做（保守方向）。
                rewrite_version_marker(&self.trace_root, &self.version, &self.file_log);
                if stale_trace {
                    delete_trace(
                        &self.trace_root,
                        owner_uid,
                        "trace_reclaim_after_preflight",
                        &self.file_log,
                    );
                }
            }
            verified_unavailable => {
                // 超时/阻塞任务异常：按 win32「探测 Timeout 域不计阻断」放行，
                // 但未验证——不重写版本标记、不删遗留痕（下次连接重复触发，
                // 保守方向；新痕直接覆盖受理位）。
                let reason = match verified_unavailable {
                    Err(_) => "budget_timeout".to_owned(),
                    Ok(Err(_)) => "scan_task_failed".to_owned(),
                    _ => unreachable!(),
                };
                logs.publish(
                    "warn",
                    "preflight",
                    "PREFLIGHT_UNVERIFIED",
                    "preflight did not finish within the budget; admitting without verification",
                    &[("reason", reason.clone())],
                );
                (self.file_log)(
                    "warn",
                    "preflight",
                    "PREFLIGHT_UNVERIFIED",
                    "preflight did not finish within the budget; admitting without verification",
                    &[
                        ("action", "preflight_run".to_owned()),
                        ("result", "unverified".to_owned()),
                        ("reason", reason),
                    ],
                );
            }
        }
        write_trace(
            &self.trace_root,
            owner_uid,
            server,
            username,
            mode,
            &self.file_log,
        );
        Ok(GateAdmission::FreshAfterPreflight { triggers, verified })
    }

    /// 善后已验证的删除权入口（StopTunnel 清理完成 / 会话收敛确认后调用）：
    /// 删除本次 owner 的痕迹文件（双亡路径不调用本函数——残留痕即下次连接的
    /// 预检触发信号）。
    pub(crate) fn finish_verified_teardown(&self, owner_uid: u32) {
        if !self.active {
            return;
        }
        delete_trace(
            &self.trace_root,
            owner_uid,
            "trace_delete_verified_teardown",
            &self.file_log,
        );
    }

    /// S2 启动清扫（挂 fresh runtime 启动路径；携带 runtime 复用跳过由调用方
    /// 承担）：managed resolver 标记回收域——仅删带标记文件、外来跳过、回收后
    /// 按需补 HUP+flush（复用既有通知链）。动作落文件日志（component=sweep）。
    pub(crate) fn sweep_startup_residue(&self, owner_uid: u32, logs: &LogSink) {
        if !self.active {
            return;
        }
        let report =
            campus_resolver::reclaim_managed_files(&self.resolver_dir, &*self.command, logs);
        for domain in &report.removed {
            (self.file_log)(
                "info",
                "sweep",
                "SWEEP_RESOLVER_REMOVED",
                "startup sweep reclaimed managed resolver residue",
                &[
                    ("action", "resolver_reclaim".to_owned()),
                    ("target", domain.clone()),
                    ("result", "removed".to_owned()),
                ],
            );
        }
        for (domain, error) in &report.removal_failed {
            (self.file_log)(
                "warn",
                "sweep",
                "SWEEP_RESOLVER_FAILED",
                "startup sweep could not remove managed resolver residue; connect-gate preflight will retry",
                &[
                    ("action", "resolver_reclaim".to_owned()),
                    ("target", domain.clone()),
                    ("result", "failed".to_owned()),
                    ("reason", error.to_string()),
                ],
            );
        }
        for path in &report.foreign_kept {
            (self.file_log)(
                "info",
                "sweep",
                "SWEEP_RESOLVER_FOREIGN_KEPT",
                "foreign resolver file kept untouched",
                &[
                    ("action", "resolver_reclaim".to_owned()),
                    ("target", path.clone()),
                    ("result", "kept".to_owned()),
                ],
            );
        }
        logs.publish(
            "info",
            "sweep",
            "SWEEP_DONE",
            "startup residue sweep finished",
            &[
                ("removed", report.removed.len().to_string()),
                ("removal_failed", report.removal_failed.len().to_string()),
                ("foreign_kept", report.foreign_kept.len().to_string()),
            ],
        );
        (self.file_log)(
            "info",
            "sweep",
            "SWEEP_DONE",
            "startup residue sweep finished",
            &[
                ("action", "startup_sweep".to_owned()),
                ("result", "done".to_owned()),
                ("owner_uid", owner_uid.to_string()),
                ("removed", report.removed.len().to_string()),
                ("removal_failed", report.removal_failed.len().to_string()),
                ("foreign_kept", report.foreign_kept.len().to_string()),
            ],
        );
    }
}

/// spawn_blocking 适配（`run_preflight_scan` 按值消费 inputs）。
fn run_preflight_scan_inputs(
    inputs: PreflightScanInputs,
) -> impl FnOnce() -> PreflightReport + Send {
    move || run_preflight_scan(inputs)
}

/// 进程级生产门（service engine 单 owner 假设；owner 按调用传入）。
pub(crate) fn process_gate() -> &'static ConnectGate {
    static GATE: std::sync::OnceLock<ConnectGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| ConnectGate::new_production(crate::build_identity::release_version()))
}

// ---------------------------------------------------------------------------
// 单元测试：注入临时目录与假执行源；不触真实系统（/etc/resolver、netstat、
// ifconfig、PF_ROUTE 均为假件——实机行为归业务流验收）。
// ---------------------------------------------------------------------------
