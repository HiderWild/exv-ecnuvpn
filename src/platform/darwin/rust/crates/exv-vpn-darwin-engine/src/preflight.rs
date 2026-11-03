//! T6（darwin 连接基建）：连接前预检——并行只读探测 + 按域清理 + 复验。
//!
//! 吸收 Windows NRPT 孤儿事故的元教训（v6 §三 + 计划 §3.2）：承载进程亡 →
//! 自有资源无主的整类问题由**下次连接预检**自愈；外来资源只报不删。
//!
//! 触发（v6 语义的 darwin 落地）：trace 残留（双亡格兜底）/版本升级后首连。
//! 域（darwin 现状，按计划 §3.2 定案）：
//! ① 自有 runtime 目录残留（`/private/tmp/exv-vpn-*`，owner 匹配且内部
//!    engine.sock 探活已死）→ guarded 回收；② 孤儿路由（路由表指向已消失的
//!    utun 接口）→ PF_ROUTE 按原值删除；③ 固定端点 stale（T7 探活已让 bind
//!    自愈）与外来冲突 → 只报告。全部有界（≤3s；预检运行于会话建立前，
//!    与紧急断开无并发面）。
//!
//! 治理边界：不建穷举门禁——预检失败不阻塞连接（best-effort + 日志/trace
//! 事件），未实现域如实留空（系统 DNS/代理 darwin 不写，无清理面）。

use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use crate::platform::route::{AppliedRoute, delete as delete_route};

/// 预检报告（全部域的结果；只含非秘密事实）。
#[derive(Debug, Default)]
pub(crate) struct PreflightReport {
    /// 回收的自有 runtime 目录数。
    pub(crate) stale_runtime_dirs_removed: usize,
    /// 探活跳过（仍有活 oneshot 引擎持有）的目录数。
    pub(crate) stale_runtime_dirs_skipped_live: usize,
    /// 清理的孤儿路由数。
    pub(crate) orphan_routes_removed: usize,
    /// 只报告的观察（外来端点/未知域）。
    pub(crate) observations: Vec<String>,
}

/// 升级标记：与当前构建版本不同 → 首连强制预检（v6 升级迁移）。
/// 返回 `true` 表示版本变化（调用方随后重写标记）。
pub(crate) fn version_marker_changed(base: &Path, current_version: &str) -> bool {
    let marker = base.join("version.v1");
    match std::fs::read_to_string(&marker) {
        Ok(existing) => existing.trim() != current_version,
        Err(_) => true, // 缺失=首装或升级后首启，同样强制一轮。
    }
}

/// 重写升级标记（预检完成后调用）。
pub(crate) fn rewrite_version_marker(base: &Path, current_version: &str) {
    let _ = std::fs::create_dir_all(base);
    let temp = base.join("version.v1.tmp");
    if std::fs::write(&temp, current_version).is_ok() {
        let _ = std::fs::rename(&temp, base.join("version.v1"));
    }
}

/// 枚举候选 stale runtime 目录：`tmp_base` 下 `exv-vpn-` 前缀 ∧ 属主=owner ∧
/// 内部 `engine.sock` 探活已死（活目录=在跑的 oneshot 引擎持有，跳过）。
pub(crate) fn collect_stale_runtime_dirs(tmp_base: &Path, owner_uid: u32) -> (Vec<PathBuf>, usize) {
    let mut stale = Vec::new();
    let mut skipped_live = 0;
    let Ok(entries) = std::fs::read_dir(tmp_base) else {
        return (stale, skipped_live);
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("exv-vpn-") {
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
        if socket.exists()
            && std::os::unix::net::UnixStream::connect(&socket).is_ok()
        {
            // 探活可连=仍有引擎进程监听（它随后自行收敛清理）。
            skipped_live += 1;
            continue;
        }
        stale.push(path);
    }
    (stale, skipped_live)
}

/// guarded 回收单个 stale runtime 目录：只摘已知叶（engine.sock/engine.ticket），
/// 然后 rmdir——非空即保留现场（绝不递归删除未知内容）。
pub(crate) fn cleanup_stale_runtime_dir(dir: &Path) -> bool {
    for leaf in ["engine.sock", "engine.ticket"] {
        let _ = std::fs::remove_file(dir.join(leaf));
    }
    std::fs::remove_dir(dir).is_ok()
}

/// 解析 `netstat -rn -f inet` 输出中指向**已消失 utun 接口**的路由（孤儿）。
/// 纯函数（解析器与执行分离，测试用夹具文本）。
pub(crate) fn parse_orphan_routes(netstat_text: &str, live_interfaces: &[String]) -> Vec<AppliedRoute> {
    let live: std::collections::HashSet<&str> = live_interfaces
        .iter()
        .map(String::as_str)
        .collect();
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

/// 当前在场的 utun 接口名集合（ifconfig 解析；探测只读）。
pub(crate) fn live_utun_interfaces() -> Vec<String> {
    let Ok(output) = std::process::Command::new("ifconfig").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| name.starts_with("utun"))
        .map(str::to_string)
        .collect()
}

/// 执行预检（有界）：自有域清理 + 外来只报。失败不阻塞（best-effort）。
pub(crate) fn run(owner_uid: u32, logs: &crate::log_sink::LogSink) -> PreflightReport {
    let mut report = PreflightReport::default();
    // 域①：自有 runtime 目录。
    let (stale_dirs, skipped_live) = collect_stale_runtime_dirs(Path::new("/private/tmp"), owner_uid);
    report.stale_runtime_dirs_skipped_live = skipped_live;
    for dir in stale_dirs {
        if cleanup_stale_runtime_dir(&dir) {
            report.stale_runtime_dirs_removed += 1;
        } else {
            report
                .observations
                .push(format!("stale runtime dir kept (non-empty): {}", dir.display()));
        }
    }
    // 域②：孤儿路由（只处理指向已消失 utun 的条目；其余域只报）。
    let netstat_text = std::process::Command::new("netstat")
        .args(["-rn", "-f", "inet"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let live = live_utun_interfaces();
    for orphan in parse_orphan_routes(&netstat_text, &live) {
        match delete_route(&orphan) {
            Ok(()) => report.orphan_routes_removed += 1,
            Err(error) => report.observations.push(format!(
                "orphan route delete failed ({orphan:?}): {error}"
            )),
        }
    }
    for line in report.observations.clone() {
        logs.publish("warn", "preflight", "PREFLIGHT_OBSERVATION", line, &[]);
    }
    logs.publish(
        "info",
        "preflight",
        "PREFLIGHT_DONE",
        "preflight completed",
        &[
            ("runtime_dirs_removed", report.stale_runtime_dirs_removed.to_string()),
            ("runtime_dirs_live_skipped", report.stale_runtime_dirs_skipped_live.to_string()),
            ("orphan_routes_removed", report.orphan_routes_removed.to_string()),
        ],
    );
    report
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
    // C2（复审）：netstat 自然掩码缩写——无 / 前缀时按八位组数推类
    // （"202.120.80"=3 组→/24；"10.10"=2→/16；"10"=1→/8）。误判 /32 会让
    // RTM_DELETE ESRCH 被当已删，孤儿路由静默漏删。
    match field.split('.').count() {
        3 => 24,
        2 => 16,
        1 => 8,
        _ => 32,
    }
}

fn parse_gateway(field: &str) -> Option<Ipv4Addr> {
    // 链路层网关（接口名形态）不是我们的隧道网关——跳过。
    field.parse().ok()
}

fn if_nametoindex(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    // SAFETY: 只读取传入接口名；返回值是内核提供的索引。
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    (index != 0).then_some(index)
}
