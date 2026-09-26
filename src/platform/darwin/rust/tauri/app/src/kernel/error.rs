//! Command 层错误类型。serde 序列化，随 invoke() 返回值传到前端。
//!
//! P4 v2 起 darwin core 发出 win32 既有服务路由前缀
//! （`service_not_running|`/`service_connect_failed|`），`ServiceNotRunning`/
//! `ServiceConnectFailed` 变体与 win32 形状一致（kind 字符串同源），前端
//! `ServiceConnectFailureModal` 恢复链在 darwin 同样激活。

use std::fmt;

use serde::ser::SerializeStruct;

/// UI Command 层错误。P4-a 阶段仅有骨架错误（not_wired）；
/// P4-b 真实接线后扩展 gRPC/通道错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    /// Command 已定义但尚未接线到 core（P4-b 前占位）。
    NotWired(String),
    /// core 进程不可达（未启动/通道断开）。
    CoreUnreachable(String),
    /// 本地连接凭据缺失或不可用；`code` 是供前端恢复流分流的稳定类别
    ///（darwin core 的 `DARWIN_CORE_CONNECT_CREDENTIALS_MISSING` 稳定码）。
    CredentialRequired {
        code: Option<String>,
        message: String,
    },
    /// 服务已安装但未就绪（P4 v2：connect 路由 `service_not_running|` 前缀 → 弹
    /// 恢复 modal；win32 R5 同款语义）。
    ServiceNotRunning(String),
    /// 服务未安装（P4 v2 状态拆分：darwin 无一次性连接形态，未装即无法连接——
    /// 不弹恢复 modal，toast 给出安装指引；win32 无此态）。
    ServiceNotInstalled(String),
    /// 服务形态连接失败（P4 v2：`service_connect_failed|` 前缀 → 弹恢复 modal）。
    ServiceConnectFailed(String),
    /// 内部错误。
    Internal(String),
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (kind, code, message) = match self {
            Self::NotWired(message) => ("not_wired", None, message.as_str()),
            Self::CoreUnreachable(message) => ("core_unreachable", None, message.as_str()),
            Self::CredentialRequired { code, message } => {
                ("credential_required", code.as_deref(), message.as_str())
            }
            Self::ServiceNotRunning(message) => ("service_not_running", None, message.as_str()),
            Self::ServiceNotInstalled(message) => ("service_not_installed", None, message.as_str()),
            Self::ServiceConnectFailed(message) => {
                ("service_connect_failed", None, message.as_str())
            }
            Self::Internal(message) => ("internal", None, message.as_str()),
        };
        let mut output =
            serializer.serialize_struct("AppError", if code.is_some() { 3 } else { 2 })?;
        output.serialize_field("kind", kind)?;
        if let Some(code) = code {
            output.serialize_field("code", code)?;
        }
        output.serialize_field("message", message)?;
        output.end()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWired(m) => write!(f, "not wired to core: {m}"),
            Self::CoreUnreachable(m) => write!(f, "core unreachable: {m}"),
            Self::CredentialRequired { code, message } => {
                write!(f, "credential required ({code:?}): {message}")
            }
            Self::ServiceNotRunning(m) => write!(f, "service not running: {m}"),
            Self::ServiceNotInstalled(m) => write!(f, "service not installed: {m}"),
            Self::ServiceConnectFailed(m) => write!(f, "service connect failed: {m}"),
            Self::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for AppError {}
