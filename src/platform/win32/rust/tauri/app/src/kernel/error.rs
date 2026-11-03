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
    /// core 业务唤醒失败（T9 §2.9：确认死亡后重拉失败，或子进程仍在运行但控制管道
    /// 不可达）。前端据此弹出「尝试重启程序 / 仅退出」强报错模态。
    CoreWakeFailed(String),
    /// 服务已安装但未运行（R5：connect 路由 `service_not_running|` 前缀 → 弹恢复 modal）。
    ServiceNotRunning(String),
    /// 服务 mode 连接失败（R5：connect 路由 `service_connect_failed|` 前缀 → 弹恢复 modal）。
    ServiceConnectFailed(String),
    /// 兼容模式需要新版 engine 能力；提示更新或修复本地 EXV 服务。
    CompatibilityModeUnavailable(String),
    /// 所选协议在当前版本不可用（T02：aTrust 后端尚未接入）；提示改用 Cisco 或更新版本。
    ProtocolUnavailable(String),
    /// 本地连接凭据缺失或不可用；`code` 是供前端恢复流分流的稳定类别。
    CredentialRequired {
        code: Option<String>,
        message: String,
    },
    /// 仅由固定认证提示构造；不携带浏览器、系统或上游响应原文。
    BrowserLoginFailed(&'static str),
    /// 内部错误。
    Internal(String),
}

impl AppError {
    /// 浏览器认证路径只向界面交付已知固定提示；未知原文不跨过此边界。
    pub fn browser_login_failed(detail: impl AsRef<str>) -> Self {
        const MESSAGES: &[&str] = &[
            "浏览器登录状态不可用",
            "正在等待浏览器登录，请完成或取消当前登录",
            "无法读取系统默认浏览器设置",
            "系统默认浏览器目前不受支持；请将 HTTPS 默认浏览器设为 Microsoft Edge 或 Chrome 后重试",
            "系统默认浏览器已识别，但未找到对应的 Edge 或 Chrome 程序",
            "无法定位 EXV 浏览器配置目录",
            "无法创建 EXV 浏览器配置目录",
            "无法准备浏览器调试入口",
            "无法启动系统默认浏览器",
            "无法检查浏览器状态",
            "系统默认浏览器启动后已退出",
            "等待浏览器调试入口超时",
            "无法连接浏览器调试入口",
            "浏览器未返回标识",
            "无法绑定登录标签页",
            "浏览器调试连接已关闭",
            "浏览器调试连接读取失败",
            "浏览器响应格式错误",
            "浏览器命令执行失败",
            "浏览器已退出",
            "浏览器命令超时",
            "浏览器登录已取消",
            "取消浏览器登录超时；当前认证窗口仍在关闭，请稍后重试",
            "连接提交尚未结束，未确认停止",
            "浏览器登录超时，请重试",
            "网关未提供浏览器认证入口",
            "网关浏览器认证入口不是有效的本站路径",
            "无法读取网关认证入口",
            "网关地址编码失败",
            "浏览器 Cookie 响应格式错误",
            "浏览器会话字段无效",
            "浏览器尚未取得网关会话",
            "浏览器会话格式无效",
            "aTrust 网关地址无效",
            "aTrust 认证入口无效",
            "无法定位 aTrust 内嵌浏览器数据目录",
            "无法准备 aTrust 内嵌浏览器数据目录",
            "无法打开 aTrust 内嵌登录窗口",
            "登录已取消",
            "内嵌登录超时，请重试",
            "登录窗口已关闭",
            "无法读取网关认证状态",
            "学校已确认登录，但未取得网关会话，请重试登录",
            "学校认证入口无效",
            "无法打开学校认证入口",
            "无法读取内嵌浏览器标识",
            "网关返回了无效的认证入口",
            "无法读取内嵌登录会话",
            "内嵌浏览器会话格式无效",
            "无法编码浏览器登录会话",
            "会话缓存过大",
            "无法保护或恢复当前用户的 aTrust 会话",
            "会话缓存编码失败",
            "无法创建会话缓存目录",
            "无法保存加密会话",
        ];
        let message = MESSAGES
            .iter()
            .copied()
            .find(|message| *message == detail.as_ref())
            .unwrap_or("浏览器登录未完成，请重试；若持续出现请反馈应用日志。");
        Self::BrowserLoginFailed(message)
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (kind, code, message) = match self {
            Self::NotWired(message) => ("not_wired", None, message.as_str()),
            Self::CoreUnreachable(message) => ("core_unreachable", None, message.as_str()),
            Self::CoreWakeFailed(message) => ("core_wake_failed", None, message.as_str()),
            Self::ServiceNotRunning(message) => ("service_not_running", None, message.as_str()),
            Self::ServiceConnectFailed(message) => {
                ("service_connect_failed", None, message.as_str())
            }
            Self::CompatibilityModeUnavailable(message) => {
                ("compatibility_mode_unavailable", None, message.as_str())
            }
            Self::ProtocolUnavailable(message) => ("protocol_unavailable", None, message.as_str()),
            Self::CredentialRequired { code, message } => {
                ("credential_required", code.as_deref(), message.as_str())
            }
            Self::BrowserLoginFailed(message) => ("browser_login_failed", None, *message),
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
            Self::CoreWakeFailed(m) => write!(f, "core wake failed: {m}"),
            Self::ServiceNotRunning(m) => write!(f, "service not running: {m}"),
            Self::ServiceConnectFailed(m) => write!(f, "service connect failed: {m}"),
            Self::CompatibilityModeUnavailable(m) => {
                write!(f, "compatibility mode unavailable: {m}")
            }
            Self::ProtocolUnavailable(m) => write!(f, "protocol unavailable: {m}"),
            Self::CredentialRequired { code, message } => {
                write!(f, "credential required ({code:?}): {message}")
            }
            Self::BrowserLoginFailed(m) => write!(f, "browser login failed: {m}"),
            Self::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for AppError {}
