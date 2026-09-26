//! Command 层错误类型。serde 序列化，随 invoke() 返回值传到前端。

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
    /// 服务已安装但未运行（R5：connect 路由 `service_not_running|` 前缀 → 弹恢复 modal）。
    ServiceNotRunning(String),
    /// 服务 mode 连接失败（R5：connect 路由 `service_connect_failed|` 前缀 → 弹恢复 modal）。
    ServiceConnectFailed(String),
    /// 兼容模式需要新版 engine 能力；提示更新或修复本地 EXV 服务。
    CompatibilityModeUnavailable(String),
    /// 本地连接凭据缺失或不可用；`code` 是供前端恢复流分流的稳定类别。
    CredentialRequired {
        code: Option<String>,
        message: String,
    },
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
            Self::ServiceNotRunning(message) => ("service_not_running", None, message.as_str()),
            Self::ServiceConnectFailed(message) => {
                ("service_connect_failed", None, message.as_str())
            }
            Self::CompatibilityModeUnavailable(message) => {
                ("compatibility_mode_unavailable", None, message.as_str())
            }
            Self::CredentialRequired { code, message } => {
                ("credential_required", code.as_deref(), message.as_str())
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
            Self::ServiceNotRunning(m) => write!(f, "service not running: {m}"),
            Self::ServiceConnectFailed(m) => write!(f, "service connect failed: {m}"),
            Self::CompatibilityModeUnavailable(m) => {
                write!(f, "compatibility mode unavailable: {m}")
            }
            Self::CredentialRequired { code, message } => {
                write!(f, "credential required ({code:?}): {message}")
            }
            Self::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for AppError {}
