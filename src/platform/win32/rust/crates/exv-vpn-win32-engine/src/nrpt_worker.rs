//! Engine 生命周期内的精确 NRPT owner worker。
//!
//! PowerShell 子进程持有 stdin 管道；正常关闭和 Engine 崩溃都会让脚本观察到 EOF，
//! 随后只删除本次 owner 标记且内容仍匹配的规则。

use std::net::IpAddr;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;

const READY_TIMEOUT: Duration = Duration::from_secs(8);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const SCRIPT: &str = include_str!("nrpt_worker.ps1");

#[derive(Serialize)]
struct WorkerRequest<'a> {
    domains: &'a [String],
    servers: &'a [String],
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

fn request_line(domains: &[String], servers: &[String]) -> Result<String, String> {
    serde_json::to_string(&WorkerRequest { domains, servers })
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
        /// 是否为「校园 DNS 域名已有精确规则」conflict（受理门兜底认领清理的机器
        /// 判别位，计划 v6.1 §8.2 触发模型补洞——字符串匹配脆弱，判别位随
        /// `ReadyLine::Failed("conflict")` 解析点产生）。
        pub(crate) conflict: bool,
        // Box：lease 持子进程句柄/通道（>128B），装箱把 Err 变体压回常规尺寸
        //（错误返回值按值穿过多层调用，热路径不应携带大对象）。
        pub(crate) worker: Option<Box<NrptWorkerLease>>,
    }

    impl From<String> for NrptStartError {
        fn from(message: String) -> Self {
            Self {
                message,
                conflict: false,
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
        pub(crate) fn start(
            domains: &[String],
            dns_servers: &[String],
        ) -> Result<Option<Self>, NrptStartError> {
            Self::start_with_script(domains, dns_servers, SCRIPT)
        }

        fn start_with_script(
            domains: &[String],
            dns_servers: &[String],
            script: &str,
        ) -> Result<Option<Self>, NrptStartError> {
            let Some(domains) = validate_inputs(domains, dns_servers)? else {
                return Ok(None);
            };
            let servers = normalize_servers(dns_servers)?;
            let request = request_line(&domains, &servers)?;
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
                .map_err(|_| {
                    "无法启动校园 DNS 规则，请检查 Windows PowerShell 后重试。".to_owned()
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
                        Ok(line) if line.len() <= 256 => {
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
            if write_result.is_err() {
                lease.stdin.take();
                return Err(NrptStartError {
                    message: "校园 DNS 规则通信失败；正在清理，请重试。".to_owned(),
                    conflict: false,
                    worker: Some(Box::new(lease)),
                });
            }
            let first = lease.messages.recv_timeout(READY_TIMEOUT);
            // ready 解析（JSON）：`{"ready":true,"owner":"EXV-NRPT:<guid>"}`（现行
            // 脚本回带 owner）；旧脚本应答 `{"ready":true}` / `{"ready":false,…}`
            // 同为合法 JSON——兼容期两种形态都接受，owner 缺失记 None。
            let message = match first.as_deref().ok().map(parse_ready_line) {
                Some(ReadyLine::Ready { owner }) => {
                    lease.owner = owner;
                    return Ok(Some(lease));
                }
                Some(ReadyLine::Failed("conflict")) => {
                    lease.conflict = true;
                    "校园 DNS 域名已有精确规则，未覆盖现有设置；请调整校园域名配置。"
                }
                Some(ReadyLine::Failed("input")) => {
                    "校园 DNS 配置无效，请检查校园域名和解析器设置。"
                }
                Some(ReadyLine::Failed("verify")) => {
                    "校园 DNS 规则生效检查失败；正在清理，请重试。"
                }
                Some(ReadyLine::Failed("apply")) => {
                    "无法应用校园 DNS 规则；正在清理，请检查系统权限后重试。"
                }
                Some(ReadyLine::Failed(_)) | Some(ReadyLine::Invalid) => {
                    "校园 DNS 规则启动响应无效；正在清理，请重试。"
                }
                None => "校园 DNS 规则启动超时；正在清理，请重试。",
            };
            lease.stdin.take();
            Err(NrptStartError {
                message: message.to_owned(),
                conflict: lease.conflict,
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
                    Ok(line) if line == r#"{"cleanup":"incomplete"}"# => {
                        // Worker 持有未完成的规则并继续重试；调用方保留 lease。
                    }
                    Ok(_) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if cleanup_complete && self.child.try_wait().ok().flatten().is_some() {
                    return Ok(());
                }
            }
            if cleanup_complete && wait_for_exit(&mut self.child, Duration::from_millis(250)) {
                Ok(())
            } else {
                Err("校园 DNS 规则清理确认超时；worker 仍会在结束前继续清理。".to_owned())
            }
        }
    }

    impl Drop for NrptWorkerLease {
        fn drop(&mut self) {
            self.stdin.take();
        }
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
pub(crate) use windows_worker::NrptWorkerLease;
