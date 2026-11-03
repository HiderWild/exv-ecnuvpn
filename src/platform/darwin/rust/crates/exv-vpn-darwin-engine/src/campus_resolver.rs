//! M4（darwin aTrust）：校园域分域解析——`/etc/resolver/<domain>` 写入器（设计 §5.3）。
//!
//! macOS 上 NRPT 的等价物：engine（euid=0）为每个配置校园域写一份 resolver 文本
//! （`nameserver` = 隧道 DNS，来自资源目录非接口 DNS），并附 EXV 私有标记键
//! [`MANAGED_MARKER_KEY`] 作「自有文件」判据（预检回收/teardown 删除/冲突裁决共用）。
//!
//! 安装使用原子写入与 HUP 的原返回；可选 flushcache 失败单独记录。
//! 真实安装失败仍清理本次自有文件及本批条目，未完成义务交回现有账本。
//! teardown 只删除含 EXV 标记的文件并发送 HUP，不用生效查询重判结果。
//!
//! 秘密纪律：本模块只处理域名与 DNS 服务器地址，无凭据/会话材料；错误与日志
//! 不携带任何秘密。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
/// 生产 resolver 目录（engine euid=0 可写；测试注入临时目录替代）。
pub(crate) const RESOLVER_DIR: &str = "/etc/resolver";

/// EXV 私有标记键（自有文件判据）。
///
/// 新版以 resolver(5) 注释承载，旧版 XML 标记仅供升级回收识别。
/// 归属检查保持在文件级，不覆盖无 EXV 标记的外来配置。
pub(crate) const MANAGED_MARKER_KEY: &str = "com.exv.vpn.managed";

/// resolver 文本是否携带显式 `domain` 键（M4 校准矩阵开关）。
///
/// macOS 26 有「子域匹配需显式 domain 键」的报告；`ecnu.edu.cn` 属正常 TLD 不受
/// 非 IANA TLD 拦截影响，但后缀匹配行为须真机实测钉死。默认不含（resolver(5) 的
/// 文件名即域语义），校准矩阵做含/不含对照后定档。
pub(crate) const INCLUDE_DOMAIN_KEY: bool = false;

/// 外来同名冲突的稳定 reason 码（typed 拒绝透出 UI，绝不覆盖外来文件）。
pub(crate) const CONFLICT_REASON: &str = "DARWIN_ATRUST_RESOLVER_CONFLICT";

/// 单个 resolver 文件的安装请求。
pub(crate) struct ResolverInstall<'a> {
    /// 校园域（同时是文件名；已规范化——小写/无尾点）。
    pub domain: &'a str,
    /// 隧道 DNS 服务器（来自资源目录 `campus_dns_servers`；首个写入 `nameserver`）。
    pub nameservers: &'a [String],
    /// resolver 文本是否携带显式 `domain` 键（[`INCLUDE_DOMAIN_KEY`] 的逐请求形态）。
    pub include_domain_key: bool,
}

/// 已安装 resolver 的记账条目（teardown 删除自有文件的输入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledResolver {
    pub domain: String,
}

/// resolver 文件操作的 typed 失败；`reason` 只含稳定码（进日志/trace）。
#[derive(Debug)]
pub(crate) enum ResolverError {
    /// 外来同名文件（无 EXV 标记键）：typed 冲突拒绝，绝不覆盖（二轮复审 B1）。
    Conflict { domain: String, path: PathBuf },
    /// 写入层失败（目录创建/临时文件/rename）。
    Write {
        step: &'static str,
        source: io::Error,
    },
    /// 生效动作（HUP）失败——文件已写但在册状态未经生效动作确认。
    Signal { source: io::Error },
}

impl ResolverError {
    /// 稳定 reason 码（非秘密）。
    #[must_use]
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::Conflict { .. } => CONFLICT_REASON,
            Self::Write { .. } => "DARWIN_ATRUST_RESOLVER_WRITE_FAILED",
            Self::Signal { .. } => "DARWIN_ATRUST_RESOLVER_SIGNAL_FAILED",
        }
    }
}

impl std::fmt::Display for ResolverError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { domain, path } => {
                write!(
                    formatter,
                    "{}: {domain} ({})",
                    self.reason(),
                    path.display()
                )
            }
            Self::Write { step, source } => {
                write!(formatter, "{}: {step}: {source}", self.reason())
            }
            Self::Signal { source } => write!(formatter, "{}: {source}", self.reason()),
        }
    }
}

impl std::error::Error for ResolverError {}

/// 命令执行注入点（killall/dscacheutil；生产=`std::process::Command`，
/// 测试=假实现）。返回 stdout 文本。
pub(crate) type CommandRunner<'a> = &'a dyn Fn(&str, &[&str]) -> io::Result<String>;

/// 生产的命令执行器。
pub(crate) fn process_command_runner(program: &str, args: &[&str]) -> io::Result<String> {
    let mut command = resolver_command(program, args);
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{program} exited with status {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(512)
                .collect::<String>(),
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 只在 DNS 子进程中归一化既有 root 权限。Engine 保留调用用户的 real UID，
/// 但 killall 按 real UID 选择目标；继承 501/0 会漏选系统 mDNSResponder。
/// 不改父进程身份，也不改变认证、出口或服务 owner 的语义。
fn resolver_command(program: &str, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.args(args);
    // SAFETY: geteuid 仅查询当前进程身份。
    if unsafe { libc::geteuid() } == 0 {
        command.uid(0);
    }
    command
}


/// 现有同名文件的归属裁决。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolverFileKind {
    /// 文件不存在。
    Absent,
    /// 含 EXV 标记键——自家历史文件（adopt/replace/可删除）。
    Managed,
    /// 无 EXV 标记键——外来文件（冲突拒绝/只报不删）。
    Foreign,
}

/// 判断现有文件的归属（文件不存在=Absent；读取失败按 Foreign 对待——fail-closed
/// 绝不覆盖无法识别的文件）。
pub(crate) fn classify_resolver_file(path: &Path) -> ResolverFileKind {
    let marker = marker_key_line();
    let legacy_marker = format!("<key>{MANAGED_MARKER_KEY}</key>");
    let kind = match std::fs::read_to_string(path) {
        // 保留旧版 XML 标记的回收能力；新文件的标记是 resolver(5) 注释。
        Ok(content)
            if content
                .lines()
                .any(|line| line.trim() == marker || line.trim() == legacy_marker) =>
        {
            ResolverFileKind::Managed
        }
        Ok(_) | Err(_) => ResolverFileKind::Foreign,
    };
    // NotFound 是唯一可识别的「不存在」形态；其余读取失败（权限/IO）按外来对待
    // ——fail-closed，绝不覆盖无法识别的文件。
    if matches!(kind, ResolverFileKind::Foreign)
        && std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    {
        return ResolverFileKind::Absent;
    }
    kind
}

/// resolver(5) 注释中的自有标记；不会作为 DNS 配置键交给系统。
fn marker_key_line() -> String {
    format!("# {MANAGED_MARKER_KEY}")
}

/// /etc/resolver 文件使用关键词和值，不是 SystemConfiguration 的 XML plist。
pub(crate) fn render_resolver(domain: &str, nameserver: &str, include_domain_key: bool) -> String {
    let mut text = format!("{}\nnameserver {nameserver}\n", marker_key_line());
    if include_domain_key {
        text.push_str("domain ");
        text.push_str(domain);
        text.push('\n');
    }
    text
}

/// 安装单个 resolver 文件：裁决 → 原子写入 → 生效动作 → 生效门。
///
/// # Errors
///
/// 见 [`ResolverError`]；外来同名文件在写入层之前即拒绝（文件原样保留）。
pub(crate) fn install(
    dir: &Path,
    request: &ResolverInstall<'_>,
    runner: CommandRunner<'_>,
    logs: &crate::log_sink::LogSink,
) -> Result<InstalledResolver, ResolverError> {
    let path = dir.join(request.domain);
    match classify_resolver_file(&path) {
        ResolverFileKind::Foreign => {
            // 二轮复审 B1：外来文件绝不覆盖——typed 冲突拒绝，稳定 reason 透出 UI
            // 三选（手动清理/调整校园域/放弃连接；UI 面属 M7）。
            logs.publish(
                "warn",
                "platform",
                CONFLICT_REASON,
                format!(
                    "foreign /etc/resolver file present; refusing to overwrite: {}",
                    path.display()
                ),
                &[("domain", request.domain.to_owned())],
            );
            return Err(ResolverError::Conflict {
                domain: request.domain.to_owned(),
                path,
            });
        }
        ResolverFileKind::Managed => logs.publish(
            "info",
            "platform",
            "RESOLVER_REPLACE",
            "replacing EXV-managed resolver file",
            &[("domain", request.domain.to_owned())],
        ),
        ResolverFileKind::Absent => {}
    }
    let Some(nameserver) = request.nameservers.first() else {
        // 无隧道 DNS：不写文件（目录未下发解析器时不制造空 resolver）。
        logs.publish(
            "info",
            "platform",
            "RESOLVER_SKIPPED_NO_SERVERS",
            "catalog provided no campus DNS servers; no resolver file written",
            &[("domain", request.domain.to_owned())],
        );
        return Err(ResolverError::Write {
            step: "NO_SERVERS",
            source: io::Error::other("campus dns servers empty"),
        });
    };
    let contents = render_resolver(request.domain, nameserver, request.include_domain_key);
    write_atomic(&path, contents.as_bytes())
        .map_err(|(step, source)| ResolverError::Write { step, source })?;
    // HUP 是原通知动作；可选 flushcache 的失败单独记录。
    if let Err(source) = runner("killall", &["-HUP", "mDNSResponder"]) {
        if let Err(cleanup) = std::fs::remove_file(&path) {
            log_action_failure(logs, "RESOLVER_PARTIAL_CLEANUP_FAILED", request.domain,
                "install_hup_failure", "REMOVE", &cleanup, cleanup.raw_os_error());
        }
        return Err(ResolverError::Signal { source });
    }
    flush_cache_optional(runner, logs, request.domain, "install");
    Ok(InstalledResolver {
        domain: request.domain.to_owned(),
    })
}

/// 安装域清单。任一原操作失败即清理本批已装条目，
/// 返回仍需清理的条目与错误，由既有资源账本继续持有。
///
/// # Errors
///
/// 首个失败的 [`ResolverError`]；本批已装条目已按安装序回滚删除。
pub(crate) fn install_all(
    dir: &Path,
    domains: &[String],
    nameservers: &[String],
    include_domain_key: bool,
    runner: CommandRunner<'_>,
    logs: &crate::log_sink::LogSink,
) -> (Vec<InstalledResolver>, Option<ResolverError>) {
    let mut installed = Vec::new();
    for domain in domains {
        let request = ResolverInstall {
            domain,
            nameservers,
            include_domain_key,
        };
        match install(dir, &request, runner, logs) {
            Ok(entry) => installed.push(entry),
            Err(error) => {
                // 安装阶段已写文件或通知失败也属于本次 owner，回滚失败必须交回调用方。
                if classify_resolver_file(&dir.join(domain)) == ResolverFileKind::Managed
                    || matches!(
                        error,
                        ResolverError::Signal { .. }
                    )
                {
                    installed.push(InstalledResolver {
                        domain: domain.clone(),
                    });
                }
                let pending = installed
                    .into_iter()
                    .filter(|entry| match remove(dir, entry, runner, logs) {
                        Ok(()) => false,
                        Err(cleanup) => {
                            let (step, errno) = match &cleanup {
                                ResolverError::Write { step, source } => (*step, source.raw_os_error()),
                                ResolverError::Signal { source } => ("HUP", source.raw_os_error()),
                                ResolverError::Conflict { .. } => ("REMOVE", None),
                            };
                            log_action_failure(logs, "RESOLVER_BATCH_CLEANUP_FAILED", &entry.domain,
                                "install_batch_cleanup", step, &cleanup, errno);
                            true
                        }
                    })
                    .collect();
                return (pending, Some(error));
            }
        }
    }
    (installed, None)
}

/// teardown 删除自有文件 + HUP。只删含 EXV 标记键的文件（外来/未知文件原样保留）。
///
/// # Errors
///
/// 删除或 HUP 失败（调用方 best-effort 重试；条目保留在账本语义由调用方维护）。
pub(crate) fn remove(
    dir: &Path,
    entry: &InstalledResolver,
    runner: CommandRunner<'_>,
    logs: &crate::log_sink::LogSink,
) -> Result<(), ResolverError> {
    let path = dir.join(&entry.domain);
    match classify_resolver_file(&path) {
        ResolverFileKind::Managed => {
            std::fs::remove_file(&path).map_err(|source| ResolverError::Write {
                step: "REMOVE",
                source,
            })?;
        }
        ResolverFileKind::Foreign => {
            // 绝不删除外来文件（含自家路径被外来内容替换的异常态——只报）。
            logs.publish(
                "warn",
                "platform",
                "RESOLVER_FOREIGN_KEPT",
                "refusing to remove a foreign resolver file",
                &[("domain", entry.domain.clone())],
            );
        }
        ResolverFileKind::Absent => {}
    }
    runner("killall", &["-HUP", "mDNSResponder"])
        .map_err(|source| ResolverError::Signal { source })?;
    flush_cache_optional(runner, logs, &entry.domain, "remove");
    Ok(())
}

fn log_action_failure(
    logs: &crate::log_sink::LogSink,
    code: &str,
    domain: &str,
    phase: &str,
    action: &str,
    error: &impl std::fmt::Display,
    errno: Option<i32>,
) {
    let mut fields = vec![("domain", domain.to_owned()), ("phase", phase.to_owned()),
        ("action", action.to_owned()), ("error", error.to_string())];
    if let Some(errno) = errno { fields.push(("errno", errno.to_string())); }
    logs.publish("warn", "platform", code, format!("resolver {phase} {action} failed: {error}"), &fields);
}

fn flush_cache_optional(runner: CommandRunner<'_>, logs: &crate::log_sink::LogSink, domain: &str, phase: &str) {
    if let Err(error) = runner("dscacheutil", &["-flushcache"]) {
        log_action_failure(logs, "RESOLVER_FLUSHCACHE_FAILED", domain, phase,
            "dscacheutil -flushcache", &error, error.raw_os_error());
    }
}

/// S2（2026-10-26 验收修复计划）：`/etc/resolver/*` 残留域回收结果。
///
/// 与 2026-10-21 撤除前 `reclaim_managed_files` 的 `(usize, Vec<String>)` 形态
/// 同源，扩展出逐文件失败清单供连接门预检产出稳定 reason id。
#[derive(Default)]
pub(crate) struct ReclaimReport {
    /// 已回收的自有（含 EXV 标记）文件域名清单。
    pub(crate) removed: Vec<String>,
    /// 删除失败的自有文件（域名, 原始 IO 错误）——连接门阻断输入。
    pub(crate) removal_failed: Vec<(String, io::Error)>,
    /// 只报不删的外来文件路径清单（非 EXV 残留，不阻断）。
    pub(crate) foreign_kept: Vec<String>,
}

/// S2：扫描目录并回收含 EXV 标记键的自有 resolver 残留文件；外来文件只报不删。
///
/// 回收数非零时经 `runner` 复用既有 HUP + 可选 flushcache 通知链（与
/// install/remove 同一通知纪律）；任一动作失败只记录，不上抛（清扫/预检的
/// 阻断判定由调用方按 [`ReclaimReport`] 产出）。
pub(crate) fn reclaim_managed_files(
    dir: &Path,
    runner: CommandRunner<'_>,
    logs: &crate::log_sink::LogSink,
) -> ReclaimReport {
    let mut report = ReclaimReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let domain = entry.file_name().to_string_lossy().into_owned();
        match classify_resolver_file(&path) {
            ResolverFileKind::Managed => match std::fs::remove_file(&path) {
                Ok(()) => report.removed.push(domain),
                Err(error) => report.removal_failed.push((domain, error)),
            },
            ResolverFileKind::Foreign => report.foreign_kept.push(path.display().to_string()),
            ResolverFileKind::Absent => {}
        }
    }
    if !report.removed.is_empty() {
        if let Err(error) = runner("killall", &["-HUP", "mDNSResponder"]) {
            logs.publish(
                "warn",
                "platform",
                "RESOLVER_HUP_FAILED",
                format!("managed resolver files reclaimed but mDNSResponder HUP failed: {error}"),
                &[],
            );
        } else {
            flush_cache_optional(runner, logs, &report.removed.join(","), "reclaim");
        }
    }
    report
}

/// 同目录临时文件 + rename 原子写入。失败返回 `(步骤名, 错误)` 供 typed 包装。
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), (&'static str, io::Error)> {
    let dir = path
        .parent()
        .ok_or_else(|| ("PARENT", io::Error::other("resolver path has no parent")))?;
    std::fs::create_dir_all(dir).map_err(|source| ("MKDIR", source))?;
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id(),
    ));
    let write_result = (|| -> io::Result<()> {
        let mut file = match std::fs::File::create(&temp) {
            Ok(file) => file,
            Err(source) => {
                let _ = std::fs::remove_file(&temp);
                return Err(source);
            }
        };
        if let Err(source) = file.write_all(contents) {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err(source);
        }
        drop(file);
        Ok(())
    })();
    write_result.map_err(|source| ("TEMP", source))?;
    std::fs::rename(&temp, path).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        ("RENAME", source)
    })?;
    Ok(())
}
