//! 预检常驻 PowerShell 子进程句柄（2026-09-27 计划 §三 v6 底质）。
//!
//! 照 [`crate::nrpt_worker`] 的常驻模式：一次 spawn（UTF-16LE base64
//! `EncodedCommand`、`CREATE_NO_WINDOW`）、`{"ready":true}` 就绪通告、随后在
//! stdin 上顺序承载一行 JSON 请求 -> 一行 JSON 应答的 op 协议；`{"op":"bye"}`
//! 或 stdin EOF 即退出。与 `nrpt_worker` 的差异：worker 的清理义务挂在 EOF 上，
//! 本子进程的清理是显式 op（预检的清理动作必须逐条记账，不能依赖进程退出语义）。

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::nrpt_worker::encode_powershell_script;

/// 内嵌的生产脚本（随产品发布）。
const SCRIPT: &str = include_str!("preflight_probe.ps1");

/// 子进程冷启动就绪等待（计划 §三：PowerShell 冷启动 0.5–2s）。
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// 单个 op 的请求-应答等待（常驻子进程上查询为毫秒级；2s 是防御上界）。
const OP_TIMEOUT: Duration = Duration::from_secs(2);

/// `restart_dnscache` 的专用应答等待（D8 阶段 2；复审遗漏 3 必改）。
///
/// `Restart-Service Dnscache -Force` 实测语义 1–3 秒、负载下更长；沿用 2s 会把
/// 慢重启误诊为失败或「假重建环」。8s 与接管全程 10s 硬顶（`nrpt_takeover.rs`）
/// 共同成立：定位/journal/漂移复核占用余量、验证 list 走普通 2s op 预算。
/// 超时后的语义在执行器侧：有界延迟 → **新子进程**重验一次（原子进程在
/// Restart-Service 阻塞期间的迟到应答会与后续 op 应答错位，不可复用）。
pub(crate) const RESTART_OP_TIMEOUT: Duration = Duration::from_secs(8);

/// `bye` 应答后的进程退出等待。
const EXIT_TIMEOUT: Duration = Duration::from_millis(500);

/// 单行应答长度上限：路由全表可到几十 KB，1 MiB 上限只防外物失控输出。
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// `CREATE_NO_WINDOW`（与 `nrpt_worker` 同值）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// op 请求（wire 形态 `{"op":"…"}`；serde tag 即 op 名）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum ProbeRequest<'a> {
    /// 列出全部 NRPT 规则。
    List,
    /// 列出全部 IPv4 路由。
    Routes,
    /// 列出全部接口 DNS 服务器。
    Dnservers,
    /// 按规则名删除 NRPT 规则（预检自有前缀残留清理 + D8 阶段 2 静默精准接管的
    /// `rule_table` 分支——后者只删除 `takeover_targets` 判定的外来冲突规则，
    /// journal 前置完成后才调用）。
    NrptDelete {
        /// 规则名（`Get-DnsClientNrptRule` 的 Name）。
        name: &'a str,
    },
    /// 重启 Dnscache 服务（仅 D8 静默精准接管的 `effective_only` 分支调用：
    /// 仅有效表覆盖无公开寻址手段，重启清空内存态规则。护栏：NRPT worker 未
    /// 启动、`install_live` 之前、无线上隧道；应答等待用专用
    /// [`RESTART_OP_TIMEOUT`]，不用普通 2s 预算）。
    RestartDnscache,
    /// 删除一条死接口（dangling）路由。
    RouteDelete {
        /// 目的地址（点分十进制）。
        dest: &'a str,
        /// 前缀长度。
        prefix: u8,
        /// 死接口索引。
        ifindex: u32,
        /// 下一跳（空或 `0.0.0.0` 表示 on-link）。
        next_hop: &'a str,
    },
    /// 清 DNS 解析缓存（`Clear-DnsClientCache`）。
    Flushdns,
    /// 结束子进程。
    Bye,
}

impl ProbeRequest<'_> {
    /// 应答回显的 op 名（校验请求-应答配对）。
    pub(crate) fn op_name(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Routes => "routes",
            Self::Dnservers => "dnservers",
            Self::NrptDelete { .. } => "nrpt_delete",
            Self::RouteDelete { .. } => "route_delete",
            Self::Flushdns => "flushdns",
            Self::RestartDnscache => "restart_dnscache",
            Self::Bye => "bye",
        }
    }
}

/// op 应答（缺省字段按空集合解析；`ok=false` 时携带 `error`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct ProbeResponse {
    /// op 是否成功。
    pub(crate) ok: bool,
    /// 回显的 op 名。
    pub(crate) op: String,
    /// 失败原因（`ok=false` 时）。
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// 原操作异常；旧探针缺省这些字段时继续接收原 error 文本。
    #[serde(default)]
    pub(crate) native_code: Option<u32>,
    #[serde(default)]
    pub(crate) hresult: Option<i64>,
    #[serde(default)]
    pub(crate) exception_type: Option<String>,
    #[serde(default)]
    pub(crate) error_id: Option<String>,
    /// `list` 的 NRPT 规则（本地规则表）。
    #[serde(default)]
    pub(crate) rules: Vec<PsNrptRule>,
    /// `list` 的有效策略表投影（`Get-DnsClientNrptPolicy -Effective`；外来规则
    /// 可能只在此表存在——2026-10-08 事故的数据源缺口 F 修复）。
    #[serde(default)]
    pub(crate) effective: Vec<PsNrptPolicyEntry>,
    /// `routes` 的 IPv4 路由。
    #[serde(default)]
    pub(crate) routes: Vec<PsRouteRow>,
    /// `dnservers` 的接口 DNS。
    #[serde(default)]
    pub(crate) ifaces: Vec<PsDnsIface>,
}

/// NRPT 规则投影（`Get-DnsClientNrptRule`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PsNrptRule {
    /// 规则名（删除按它定位）。
    pub(crate) name: String,
    /// 命名空间集合（一条规则可含多个，如 `ecnu.edu.cn` 与 `.ecnu.edu.cn`）。
    pub(crate) namespaces: Vec<String>,
    /// 指定的解析器。
    pub(crate) servers: Vec<String>,
    /// 规则注释（engine 写入的 owner 标记形如 `EXV-NRPT:<guid>`）。
    pub(crate) comment: String,
    /// 是否要求 DNSSEC 校验（等效接纳的排除条件：任一附加设置启用即不等效）。
    #[serde(default)]
    pub(crate) dnssec: bool,
    /// 是否要求 IPsec（同上；投影属性缺失按 false——旧脚本应答兼容）。
    #[serde(default)]
    pub(crate) ipsec: bool,
    /// 是否启用 DirectAccess（同上）。
    #[serde(default)]
    pub(crate) direct_access: bool,
}

/// 有效策略表投影（`Get-DnsClientNrptPolicy -Effective` 的
/// `DnsClientPolicyConfiguration` 形态）。
///
/// 与本地规则的关键差异：**没有 Name/Comment/DisplayName**（2026-10-08 事故实测
/// 形态）——外来规则可能不经 `Add-DnsClientNrptRule` 持久化路径施加，只出现在
/// 有效表（含 Dnscache 内存态）；owner 归属无从判定，删除也无公开寻址手段。
/// 因此本投影只服务「读」（检测/等效判定/上报），任何清理路径都不消费它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PsNrptPolicyEntry {
    /// 命名空间集合（单条策略通常一个命名空间）。
    pub(crate) namespaces: Vec<String>,
    /// 指定的解析器。
    pub(crate) servers: Vec<String>,
    /// 是否要求 DNSSEC 校验（等效接纳排除条件）。
    #[serde(default)]
    pub(crate) dnssec: bool,
    /// 是否启用 DirectAccess（等效接纳排除条件）。
    #[serde(default)]
    pub(crate) direct_access: bool,
}

/// IPv4 路由投影（`Get-NetRoute`）；`metric` 为 RouteMetric（路由分量）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PsRouteRow {
    /// 目的地址（点分十进制）。
    pub(crate) dest: String,
    /// 前缀长度。
    pub(crate) prefix: u8,
    /// 下一跳（on-link 为 `0.0.0.0` 或空）。
    pub(crate) next_hop: String,
    /// 接口索引。
    pub(crate) ifindex: u32,
    /// 路由度量（RouteMetric）。
    pub(crate) metric: u32,
    /// 协议名（`Get-NetRoute` 字符串，如 `NetMgmt`/`Local`）。
    pub(crate) protocol: String,
}

/// 接口 DNS 投影（`Get-DnsClientServerAddress`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PsDnsIface {
    /// 接口索引。
    pub(crate) ifindex: u32,
    /// 接口别名。
    pub(crate) alias: String,
    /// 该接口的 DNS 服务器列表。
    pub(crate) servers: Vec<String>,
}

/// 一次预检运行共享的常驻子进程。
///
/// 生产路径经 [`ProbeChild::start`] 使用内嵌脚本；单测经
/// [`ProbeChild::start_with_script`] 注入假脚本（`nrpt_worker` `lifecycle_tests`
/// 同款接缝：参数是脚本本身，不是测试专用分支）。
pub(crate) struct ProbeChild {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<String>,
}

impl ProbeChild {
    /// 以内嵌生产脚本启动。
    pub(crate) fn start() -> Result<Self, String> {
        Self::start_with_script(SCRIPT)
    }

    /// 启动并等待 `{"ready":true}` 就绪通告。
    pub(crate) fn start_with_script(script: &str) -> Result<Self, String> {
        let powershell = std::env::var_os("SystemRoot")
            .filter(|root| !root.is_empty())
            .map(std::path::PathBuf::from)
            .map(|root| {
                root.join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe")
            })
            .ok_or_else(|| "无法启动预检 PowerShell，请检查 Windows 系统目录后重试。".to_owned())?;

        let mut child = Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode_powershell_script(script),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                "无法启动预检 PowerShell，请检查 Windows PowerShell 后重试。".to_owned()
            })?;
        let Some(stdin) = child.stdin.take() else {
            return Err("预检 PowerShell 启动失败（无 stdin），请重试。".to_owned());
        };
        let Some(stdout) = child.stdout.take() else {
            return Err("预检 PowerShell 启动失败（无 stdout），请重试。".to_owned());
        };
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if line.len() <= MAX_LINE_BYTES => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });

        let mut lease = Self {
            child,
            stdin: Some(stdin),
            messages,
        };
        match lease.messages.recv_timeout(READY_TIMEOUT) {
            Ok(line) if line == r#"{"ready":true}"# => Ok(lease),
            Ok(other) => {
                lease.close_stdin();
                Err(format!("预检 PowerShell 就绪应答无效：{other}"))
            }
            Err(_) => {
                lease.close_stdin();
                Err("预检 PowerShell 启动超时。".to_owned())
            }
        }
    }

    /// 写一行请求并等待其单行应答（原始行形态；调用方负责序列化/校验）。
    pub(crate) fn request_line(&mut self, request: &str) -> Result<String, String> {
        self.request_line_with_timeout(request, OP_TIMEOUT)
    }

    /// [`Self::request_line`] 的显式超时变体（`restart_dnscache` 等慢 op 用
    /// [`RESTART_OP_TIMEOUT`]；超时后本子进程可能残留迟到应答——后续请求的
    /// 应答会与其错位，调用方须弃用本子进程、换新进程续作）。
    pub(crate) fn request_line_with_timeout(
        &mut self,
        request: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let input = self
            .stdin
            .as_mut()
            .ok_or_else(|| "预检 PowerShell 已关闭。".to_owned())?;
        input
            .write_all(request.as_bytes())
            .and_then(|()| input.write_all(b"\n"))
            .and_then(|()| input.flush())
            .map_err(|_| "预检 PowerShell 通信失败。".to_owned())?;
        let response = self
            .messages
            .recv_timeout(timeout)
            .map_err(|_| "预检 PowerShell 应答超时。".to_owned())?;
        Ok(response)
    }

    /// 结束：`bye`（尽力）→ 关闭 stdin → 有界等待退出。
    pub(crate) fn shutdown(&mut self) {
        let bye = serde_json::to_string(&ProbeRequest::Bye).unwrap_or_default();
        let _ = self.request_line(&bye);
        self.close_stdin();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        // 只关 stdin：脚本的正常退出信号就是 EOF（与 nrpt_worker 同语义）。
        self.close_stdin();
    }
}

// ---------------------------------------------------------------------------
// 单元测试：真子进程/真管道 + 假脚本回声协议（`nrpt_worker` lifecycle_tests
// 同款接缝；不运行任何网络、NRPT、路由或注册表操作）。
// ---------------------------------------------------------------------------
