//! 自建 CA 的**当前用户** Root 存储安装/卸载（`certutil -user Root`）。
//!
//! 纪律（规格 §3、任务硬约束）：
//! * 只操作 `-user Root`（当前用户 Root），**绝不**触碰机器存储；
//! * 不改系统代理、hosts、scheme；
//! * 卸载按记录的 SHA-1 指纹执行，可一键回退。
//!
//! # 已知交互边界（实测）
//!
//! 向 Root 存储装入 CA 会触发 Windows「安全警告」确认对话框（这是系统的根证书保护机制，
//! 非本模块缺陷）：
//! * `certutil -addstore -user Root` 会等待用户确认，非交互进程会阻塞；
//! * `CertAddCertificateContextToStore`（CryptoAPI）同样弹窗；
//! * 直接写 `HKCU\...\SystemCertificates\Root\Certificates` 注册表被系统回收、不生效。
//!
//! 因此**首次安装需要用户点一次确认**；确认后指纹已在用户 Root，后续启动
//! [`is_trusted`] 命中即不再弹窗。卸载（`-delstore`）不弹窗。
//!
//! 命令执行经 [`CommandRunner`] 抽象，安装/卸载编排可用假执行器离线单测。

use std::path::Path;
use std::process::Command;

use crate::cert::{CertMaterial, ca_pem_path};

/// 存储操作错误。
#[derive(Debug)]
pub enum TrustError {
    /// 命令无法启动或返回非零。
    Command(String),
    /// 证书目录中没有可安装的 CA PEM。
    MissingCa(String),
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(message) => write!(f, "证书存储命令失败: {message}"),
            Self::MissingCa(message) => write!(f, "缺少待安装的 CA 证书: {message}"),
        }
    }
}

impl std::error::Error for TrustError {}

/// 一次命令执行的结果。
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    /// 退出码是否成功。
    pub success: bool,
    /// 原始进程退出码。
    pub code: Option<i32>,
    /// 标准输出（诊断用，不落敏感正文）。
    pub stdout: String,
    /// 标准错误。
    pub stderr: String,
}

/// 命令执行器（可替换，便于离线测试）。
pub trait CommandRunner: Send + Sync {
    /// 执行 `program`，参数为 `args`。
    ///
    /// # Errors
    /// 无法启动进程时返回 [`TrustError::Command`]。
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutcome, TrustError>;
}

/// 真实系统命令执行器。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutcome, TrustError> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| TrustError::Command(format!("{program}: {e}")))?;
        decode_command_output(
            &format!("{program} {}", args.join(" ")),
            output.status.success(),
            output.status.code(),
            &output.stdout,
            &output.stderr,
        )
    }
}

fn decode_utf16le(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xfe, 0xff]) {
        return Err("收到 UTF-16BE BOM，预期 UTF-16LE".to_owned());
    }
    let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
    if bytes.len() % 2 != 0 {
        return Err(format!("UTF-16LE 字节数为奇数: {}", bytes.len()));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|error| format!("UTF-16LE 代理项无效: {error}"))
}

fn decode_command_output(
    step: &str,
    success: bool,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<CommandOutcome, TrustError> {
    let stdout = decode_utf16le(stdout);
    let stderr = decode_utf16le(stderr);
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Ok(CommandOutcome {
            success,
            code,
            stdout,
            stderr,
        }),
        (stdout, stderr) => {
            let describe = |stream: &str, result: Result<String, String>| match result {
                Ok(text) => format!("{stream}: {}", text.trim()),
                Err(error) => format!("{stream} 解码失败: {error}"),
            };
            Err(TrustError::Command(format!(
                "{step} 退出码 {code:?}，命令成功={success}: {}; {}",
                describe("stdout", stdout),
                describe("stderr", stderr),
            )))
        }
    }
}

/// 信任存储抽象。
pub trait TrustStore: Send + Sync {
    /// 指纹（十六进制）是否已在当前用户 Root 中受信。
    ///
    /// # Errors
    /// 命令执行失败时返回 [`TrustError`]。
    fn is_trusted(&self, sha1_hex: &str) -> Result<bool, TrustError>;
    /// 把 CA PEM 文件装入当前用户 Root（首次可能弹一次系统确认框）。
    ///
    /// # Errors
    /// 命令执行失败时返回 [`TrustError`]。
    fn install(&self, ca_pem: &Path) -> Result<(), TrustError>;
    /// 按 SHA-1 指纹从当前用户 Root 删除 CA（幂等，不弹窗）。
    ///
    /// # Errors
    /// 命令执行失败时返回 [`TrustError`]。
    fn uninstall(&self, sha1_hex: &str) -> Result<(), TrustError>;
}

/// 基于 `certutil` 的实现（只操作 `-user Root`）。
#[derive(Debug, Default, Clone, Copy)]
pub struct CertutilTrustStore<R = SystemCommandRunner> {
    runner: R,
}

impl<R: CommandRunner> CertutilTrustStore<R> {
    /// 用给定执行器构造。
    pub fn new(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: CommandRunner> TrustStore for CertutilTrustStore<R> {
    fn is_trusted(&self, sha1_hex: &str) -> Result<bool, TrustError> {
        // `certutil -Unicode -store -user Root <sha1>` 按哈希过滤：命中才打印该证书（退出码 0）。
        // 未命中接受 NTE_NOT_FOUND、CRYPT_E_NOT_FOUND 完整 HRESULT 及 legacy 17；
        // legacy 17 不是完整 HRESULT。故命中判据 = 退出码成功
        // **且** 输出包含该 SHA-1（十六进制为 ASCII，与本地化标签无关）。
        let outcome = self.runner.run(
            "certutil",
            &["-Unicode", "-store", "-user", "Root", sha1_hex],
        )?;
        if !outcome.success
            && !matches!(
                outcome.code.map(|code| code as u32),
                Some(0x8009_0011 | 0x8009_2004 | 17)
            )
        {
            return Err(command_failure(
                "certutil -Unicode -store -user Root",
                &outcome,
            ));
        }
        Ok(outcome.success
            && outcome
                .stdout
                .to_ascii_lowercase()
                .contains(&sha1_hex.to_ascii_lowercase()))
    }

    fn install(&self, ca_pem: &Path) -> Result<(), TrustError> {
        if !ca_pem.is_file() {
            return Err(TrustError::MissingCa(ca_pem.display().to_string()));
        }
        let path = ca_pem.to_string_lossy().into_owned();
        let outcome = self.runner.run(
            "certutil",
            &["-Unicode", "-addstore", "-user", "Root", &path],
        )?;
        if outcome.success {
            Ok(())
        } else {
            Err(command_failure(
                "certutil -Unicode -addstore -user Root",
                &outcome,
            ))
        }
    }

    fn uninstall(&self, sha1_hex: &str) -> Result<(), TrustError> {
        let outcome = self.runner.run(
            "certutil",
            &["-Unicode", "-delstore", "-user", "Root", sha1_hex],
        )?;
        if outcome.success {
            Ok(())
        } else {
            Err(command_failure(
                "certutil -Unicode -delstore -user Root",
                &outcome,
            ))
        }
    }
}

fn command_failure(step: &str, outcome: &CommandOutcome) -> TrustError {
    TrustError::Command(format!(
        "{step} 退出码 {:?}: {} {}",
        outcome.code,
        outcome.stdout.trim(),
        outcome.stderr.trim()
    ))
}

/// 幂等确保 CA 已装入用户 Root；返回是否本次真正执行了安装。
///
/// # Errors
/// 命令执行失败或 CA PEM 缺失时返回 [`TrustError`]。
pub fn ensure_trusted(
    store: &dyn TrustStore,
    dir: &Path,
    material: &CertMaterial,
) -> Result<bool, TrustError> {
    if store.is_trusted(&material.ca_sha1_hex)? {
        return Ok(false);
    }
    store.install(&ca_pem_path(dir))?;
    Ok(true)
}

/// 一键卸载：按持久化指纹从用户 Root 删除 CA，返回被删除的指纹（不存在则 `None`）。
///
/// # Errors
/// 命令执行失败时返回 [`TrustError`]。
pub fn uninstall_trusted_ca(
    store: &dyn TrustStore,
    dir: &Path,
) -> Result<Option<String>, TrustError> {
    let Some(sha1) = crate::cert::persisted_sha1(dir) else {
        return Ok(None);
    };
    store.uninstall(&sha1)?;
    Ok(Some(sha1))
}
