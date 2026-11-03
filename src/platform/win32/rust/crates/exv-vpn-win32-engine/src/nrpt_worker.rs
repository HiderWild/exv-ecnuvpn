//! Engine 生命周期内的精确 NRPT owner worker。
//!
//! PowerShell 子进程持有 stdin 管道；正常关闭和 Engine 崩溃都会让脚本观察到 EOF，
//! 随后只删除本次 owner 标记且内容仍匹配的规则。
//!
//! 等效接纳（2026-10-08 计划阶段 1）：`accept_forms` 是 Rust 侧
//! （`preflight::determine_nrpt_acceptance` → `classify::nrpt_equivalence_decision`，
//! 单一指纹规则实现）判定的「外来规则等效提供」命名空间形式清单。worker 对这些
//! 形式跳过自有添加——不 Add、不写 owner 标记，teardown（finally 清理）天然不碰
//! 它们、也不期待它们消失（防死锁约束：清理只认本次 owner GUID，幂等）。
//! **全部形式都被接纳时调用方不启动 worker**（`start` 返回 `Ok(None)` 之前由
//! `tunnel_runtime` 短路），等效接纳路径不引入任何「等待 worker/PS 进程退出」
//! 的新依赖。

use std::net::IpAddr;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

const READY_TIMEOUT: Duration = Duration::from_secs(8);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const SCRIPT: &str = include_str!("nrpt_worker.ps1");

#[derive(Serialize)]
struct WorkerRequest<'a> {
    domains: &'a [String],
    servers: &'a [String],
    /// 等效接纳的命名空间形式（小写字面量；Rust 判定的单一实现产物）。
    /// 空集不序列化——与旧脚本/旧引擎双向兼容（未知字段/缺省字段各自安全）。
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    accept_forms: &'a [String],
}

fn validate_inputs(domains: &[String], servers: &[String]) -> Result<Option<Vec<String>>, String> {
    if domains.is_empty() {
        return Ok(None);
    }
    if servers.is_empty() {
        return Err("服务端未下发校园 DNS 解析器；请检查校园网络配置。".to_owned());
    }

    let mut normalized_domains = Vec::with_capacity(domains.len());
    for domain in domains {
        let canonical = exv_vpn_win32_config::parse_campus_dns_domains(domain)
            .ok()
            .filter(|parsed| parsed.len() == 1)
            .and_then(|mut parsed| parsed.pop())
            .ok_or_else(|| "校园 DNS 域名或解析器格式无效，请检查设置。".to_owned())?;
        if !normalized_domains.contains(&canonical) {
            normalized_domains.push(canonical);
        }
    }

    for server in servers {
        let parsed = server
            .parse::<IpAddr>()
            .map_err(|_| "校园 DNS 域名或解析器格式无效，请检查设置。".to_owned())?;
        if parsed.is_unspecified() || parsed.is_multicast() {
            return Err("校园 DNS 域名或解析器格式无效，请检查设置。".to_owned());
        }
    }
    Ok(Some(normalized_domains))
}

/// 校验 accept_forms 是展开形式集（带点+不带点）的子集；返回去重后的清单。
/// 展开规则与 `preflight::classify::expand_campus_forms` 同一约定（防两表漂移：
/// Rust 侧两处展开必须一致；脚本侧再按字面量复核）。
fn validate_accept_forms(
    domains: &[String],
    accept_forms: &[String],
) -> Result<Vec<String>, String> {
    let mut forms = Vec::with_capacity(accept_forms.len());
    for form in accept_forms {
        let canonical = form.trim().to_ascii_lowercase();
        let is_known = domains.iter().any(|domain| {
            let lower = domain.trim().to_ascii_lowercase();
            // 已知形式 = 该域的不带点/带点两种（与 expand_campus_forms 同一约定）。
            canonical == lower || canonical == format!(".{lower}")
        });
        if !is_known || forms.contains(&canonical) {
            return Err("校园 DNS 等效接纳清单无效，请重试。".to_owned());
        }
        forms.push(canonical);
    }
    Ok(forms)
}

fn request_line(
    domains: &[String],
    servers: &[String],
    accept_forms: &[String],
) -> Result<String, String> {
    serde_json::to_string(&WorkerRequest {
        domains,
        servers,
        accept_forms,
    })
    .map_err(|_| "校园 DNS worker 请求无法编码。".to_owned())
}

/// PowerShell 脚本的 UTF-16LE base64 `EncodedCommand` 编码（crate 内共享：
/// `nrpt_worker` 与 `preflight` 的常驻子进程同款启动形态）。
pub(crate) fn encode_powershell_script(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    STANDARD.encode(bytes)
}

#[cfg(windows)]
mod windows_worker {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// 持有一次 Engine 会话的 NRPT worker；Drop 仅关闭 stdin，不强杀清理进程。
    pub(crate) struct NrptWorkerLease {
        child: Child,
        stdin: Option<ChildStdin>,
        messages: Receiver<String>,
        cleanup_confirmed: bool,
        /// 本次会话的 owner 标记（`EXV-NRPT:<guid>`，ready 应答回带；旧脚本应答
        /// 不携带则为 None）。T4 起记入连接痕迹供预检 NRPT 域认领。
        owner: Option<String>,
        /// 启动应答是否为 conflict（随 ready 解析点记录，转投 `NrptStartError`）。
        conflict: bool,
    }

    pub(crate) struct NrptStartError {
        pub(crate) message: String,
        pub(crate) native_code: Option<u32>,
        /// 是否为「校园 DNS 域名已有精确规则」conflict（受理门兜底认领清理的机器
        /// 判别位，计划 v6.1 §8.2 触发模型补洞——字符串匹配脆弱，判别位随
        /// `ReadyLine::Failed("conflict")` 解析点产生）。
        pub(crate) conflict: bool,
        /// 稳定失败原因 id（阶段② reason 透传的六类细分判别位；随
        /// `tunnel_runtime` 的 campus_dns_start 失败上抛透传到 wire tag 8）。
        /// 当前取值：apply/verify 类挂 `platform/route_apply`（DNS 规则应用属
        /// T-05 路由/地址/DNS/代理 apply 语域，且无 native 码、不与 native 分档
        /// 冲突）；conflict 类挂空——D8 阶段2 静默接管落地后，conflict 进入接管
        /// 路径不再直抛，残余失败由接管分支写入
        /// `platform/nrpt_takeover_failed` / `platform/nrpt_foreign_rebuild`（D8
        /// 所有，本通道纯消费）；input/无效应答/超时维持空（T-22 泛化）。
        pub(crate) reason: &'static str,
        // Box：lease 持子进程句柄/通道（>128B），装箱把 Err 变体压回常规尺寸
        //（错误返回值按值穿多层调用，热路径不应携带大对象）。
        pub(crate) worker: Option<Box<NrptWorkerLease>>,
    }

    impl From<String> for NrptStartError {
        fn from(message: String) -> Self {
            Self {
                message,
                native_code: None,
                conflict: false,
                reason: "",
                worker: None,
            }
        }
    }

    impl NrptStartError {
        /// 是否为 conflict（受理门按 EXV-NRPT: 前缀认领清理后重试一次的判别位）。
        pub(crate) fn is_conflict(&self) -> bool {
            self.conflict
        }
    }

    impl NrptWorkerLease {
        /// 空域名不启动 worker；非空域名必须同时具有已选校园 DNS。
        ///
        /// `accept_forms`：等效接纳的命名空间形式（Rust 判定产物；空 = 原行为）。
        /// **调用方必须先做「全部形式都被接纳」短路**（`tunnel_runtime`）——
        /// 全免时不调用本函数，等效接纳路径不启动 worker、不引入新的进程
        /// 生命周期依赖（防死锁约束）。
        pub(crate) fn start(
            domains: &[String],
            dns_servers: &[String],
            accept_forms: &[String],
        ) -> Result<Option<Self>, NrptStartError> {
            Self::start_with_script(domains, dns_servers, accept_forms, SCRIPT)
        }

        /// 带脚本注入的启动（`lifecycle_tests`/`contract_tests` 的接缝：参数是
        /// 脚本本身，不是测试专用分支；生产恒用内嵌 [`SCRIPT`]）。
        pub(crate) fn start_with_script(
            domains: &[String],
            dns_servers: &[String],
            accept_forms: &[String],
            script: &str,
        ) -> Result<Option<Self>, NrptStartError> {
            let Some(domains) = validate_inputs(domains, dns_servers)? else {
                return Ok(None);
            };
            let servers = normalize_servers(dns_servers)?;
            let accepted =
                validate_accept_forms(&domains, accept_forms).map_err(NrptStartError::from)?;
            let request = request_line(&domains, &servers, &accepted)?;
            let powershell = std::env::var_os("SystemRoot")
                .filter(|root| !root.is_empty())
                .map(std::path::PathBuf::from)
                .map(|root| {
                    root.join("System32")
                        .join("WindowsPowerShell")
                        .join("v1.0")
                        .join("powershell.exe")
                })
                .ok_or_else(|| {
                    "无法启动校园 DNS 规则，请检查 Windows 系统目录后重试。".to_owned()
                })?;

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
                .map_err(|error| {
                    NrptStartError {
                        message: format!("无法启动校园 DNS 规则，请检查 Windows PowerShell 后重试：{error}"),
                        native_code: error.raw_os_error().and_then(|code| u32::try_from(code).ok()),
                        conflict: false,
                        reason: "",
                        worker: None,
                    }
                })?;

            let Some(stdin) = child.stdin.take() else {
                close_stdin_and_detach(&mut child);
                return Err("校园 DNS 规则 worker 启动失败，请重试。".to_owned().into());
            };
            let Some(stdout) = child.stdout.take() else {
                drop(stdin);
                return Err("校园 DNS 规则 worker 启动失败，请重试。".to_owned().into());
            };
            let (sender, messages) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) if line.len() <= 65536 => {
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
                cleanup_confirmed: false,
                owner: None,
                conflict: false,
            };
            let input = lease.stdin.as_mut().expect("worker stdin");
            let write_result = input
                .write_all(request.as_bytes())
                .and_then(|()| input.write_all(b"\n"))
                .and_then(|()| input.flush());
            if let Err(error) = write_result {
                lease.stdin.take();
                return Err(NrptStartError {
                    message: format!("校园 DNS 规则通信失败；正在清理，请重试：{error}"),
                    native_code: error.raw_os_error().and_then(|code| u32::try_from(code).ok()),
                    conflict: false,
                    reason: "",
                    worker: Some(Box::new(lease)),
                });
            }
            let first = lease.messages.recv_timeout(READY_TIMEOUT);
            // ready 解析（JSON）：`{"ready":true,"owner":"EXV-NRPT:<guid>"}`（现行
            // 脚本回带 owner）；旧脚本应答 `{"ready":true}` / `{"ready":false,…}`
            // 同为合法 JSON——兼容期两种形态都接受，owner 缺失记 None。
            // 解析结果同时给出（失败文案, 稳定 reason id）——六类细分判别位见
            // `NrptStartError.reason` 字段注记。
            let parsed = first.as_deref().ok().map(parse_ready_line);
            let failure = first.as_deref().ok().and_then(parse_operation_failure);
            let (message, reason) = match parsed {
                Some(ReadyLine::Ready { owner }) => {
                    lease.owner = owner;
                    return Ok(Some(lease));
                }
                Some(ReadyLine::Failed("conflict")) => {
                    lease.conflict = true;
                    // conflict 直抛维持空 reason：D8 接管落地后此分支进接管路径，
                    // 残余由接管分支挂 nrpt_takeover_failed/nrpt_foreign_rebuild。
                    (
                        "校园 DNS 域名已有精确规则，未覆盖现有设置；请调整校园域名配置。",
                        "",
                    )
                }
                Some(ReadyLine::Failed("input")) => (
                    "校园 DNS 配置无效，请检查校园域名和解析器设置。",
                    "",
                ),
                Some(ReadyLine::Failed("verify")) => (
                    "校园 DNS 规则生效检查失败；正在清理，请重试。",
                    "platform/route_apply",
                ),
                Some(ReadyLine::Failed("apply")) => (
                    "无法应用校园 DNS 规则；正在清理，请检查系统权限后重试。",
                    "platform/route_apply",
                ),
                Some(ReadyLine::Failed(_)) | Some(ReadyLine::Invalid) => {
                    ("校园 DNS 规则启动响应无效；正在清理，请重试。", "")
                }
                None => ("校园 DNS 规则启动超时；正在清理，请重试。", ""),
            };
            lease.stdin.take();
            let native_code = failure.as_ref().and_then(|failure| failure.native_code);
            Err(NrptStartError {
                message: failure.as_ref().map_or_else(|| message.to_owned(), |failure| format!("{message}；{failure}")),
                native_code,
                conflict: lease.conflict,
                reason: if native_code.is_some() { "" } else { reason },
                worker: Some(Box::new(lease)),
            })
        }

        /// 本次会话的 owner 标记（`EXV-NRPT:<guid>`；只读暴露，T4 起记入连接痕迹）。
        /// `None` = 旧脚本应答未携带（或空域名未启动 worker）。
        pub(crate) fn owner_guid(&self) -> Option<&str> {
            self.owner.as_deref()
        }

        /// 关闭持有管道并等待脚本确认本次 owner 规则清理。
        pub(crate) fn shutdown(&mut self) -> Result<(), String> {
            self.stdin.take();
            if self.cleanup_confirmed {
                return if wait_for_exit(&mut self.child, Duration::from_millis(250)) {
                    Ok(())
                } else {
                    Err("校园 DNS 清理已确认，worker 正在退出；请稍后重试。".to_owned())
                };
            }
            let deadline = Instant::now() + CLEANUP_TIMEOUT;
            let mut cleanup_complete = false;
            let mut first_error = None;
            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self
                    .messages
                    .recv_timeout(remaining.min(Duration::from_millis(100)))
                {
                    Ok(line) if line == r#"{"cleanup":"complete"}"# => {
                        cleanup_complete = true;
                        self.cleanup_confirmed = true;
                    }
                    Ok(line) if serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|value| value["cleanup"].as_str().map(str::to_owned)).as_deref() == Some("incomplete") => {
                        if first_error.is_none() {
                            first_error = parse_operation_failure(&line).map(|failure| failure.to_string());
                        }
                        // Worker 持有未完成的规则并继续重试；调用方保留 lease。
                    }
                    Ok(_) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if cleanup_complete && self.child.try_wait().ok().flatten().is_some() {
                    return first_error.map_or(Ok(()), Err);
                }
            }
            if cleanup_complete && wait_for_exit(&mut self.child, Duration::from_millis(250)) {
                first_error.map_or(Ok(()), Err)
            } else {
                Err(first_error.map_or_else(|| "校园 DNS 规则清理确认超时；worker 仍会在结束前继续清理。".to_owned(), |error| format!("校园 DNS 规则清理确认超时；{error}")))
            }
        }
    }

    impl Drop for NrptWorkerLease {
        fn drop(&mut self) {
            self.stdin.take();
        }
    }

    /// ready 应答行的解析结局。
    #[derive(Debug, serde::Deserialize)]
    struct OperationFailure {
        operation: String,
        message: String,
        native_code: Option<u32>,
        hresult: Option<i64>,
        exception_type: Option<String>,
        error_id: Option<String>,
    }

    impl std::fmt::Display for OperationFailure {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "NRPT {}：{}；native_code={:?}；hresult={:?}；exception_type={}；error_id={}",
                self.operation, self.message, self.native_code, self.hresult,
                self.exception_type.as_deref().unwrap_or(""), self.error_id.as_deref().unwrap_or(""))
        }
    }

    fn parse_operation_failure(line: &str) -> Option<OperationFailure> {
        #[derive(serde::Deserialize)]
        struct Response { failure: Option<OperationFailure> }
        serde_json::from_str::<Response>(line).ok()?.failure
    }

    /// ready 应答行的解析结局。
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ReadyLine {
        /// ready=true；owner 标记存在时携带（`EXV-NRPT:<guid>` 前缀校验）。
        Ready { owner: Option<String> },
        /// ready=false 且 error 字段可读（conflict/input/verify/apply/其他）。
        Failed(&'static str),
        /// 非 JSON / 结构不符 / ready 字段非布尔——按无效应答处理。
        Invalid,
    }

    /// 解析 ready 应答行（单行 JSON）。owner 仅接受 `EXV-NRPT:` 前缀且长度受控的
    /// 标记（子进程产物仍按不可信输入校验，防把任意长文本灌进连接痕迹）。
    fn parse_ready_line(line: &str) -> ReadyLine {
        #[derive(serde::Deserialize)]
        struct ReadyJson {
            ready: bool,
            owner: Option<String>,
            error: Option<String>,
        }
        let Ok(parsed) = serde_json::from_str::<ReadyJson>(line) else {
            return ReadyLine::Invalid;
        };
        if parsed.ready {
            let owner = parsed
                .owner
                .filter(|marker| {
                    marker.starts_with("EXV-NRPT:") && marker.len() <= 64 && marker.is_ascii()
                })
                .filter(|marker| {
                    marker["EXV-NRPT:".len()..]
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || b == b'-')
                });
            return ReadyLine::Ready { owner };
        }
        // 失败码集合封闭（脚本侧只产生这四种）；未知 error 按 Invalid 归并。
        let kind = match parsed.error.as_deref() {
            Some("conflict") => "conflict",
            Some("input") => "input",
            Some("verify") => "verify",
            Some("apply") => "apply",
            _ => return ReadyLine::Invalid,
        };
        ReadyLine::Failed(kind)
    }

    fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                _ => return false,
            }
        }
    }

    fn close_stdin_and_detach(child: &mut Child) {
        child.stdin.take();
        let _ = child;
    }

    fn normalize_servers(servers: &[String]) -> Result<Vec<String>, String> {
        let mut normalized = Vec::with_capacity(servers.len());
        for server in servers {
            let parsed = server
                .parse::<IpAddr>()
                .map_err(|_| "校园 DNS 域名或解析器格式无效，请检查设置。".to_owned())?;
            let canonical = parsed.to_string();
            if !normalized.contains(&canonical) {
                normalized.push(canonical);
            }
        }
        Ok(normalized)
    }
}

#[cfg(windows)]
pub(crate) use windows_worker::{NrptStartError, NrptWorkerLease};

