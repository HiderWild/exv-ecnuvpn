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
    /// `code` 是页面分类等稳定机器码（前端据以分流「重试/手动介入/取消」）；普通提示为 `None`。
    /// `server_detail` 是**净化后的服务端消息摘录**（2026-10-07 终端页研究：失败页
    /// `#errorData` 文本 / 限流页 `err_info` 提示，经 `码 \x1f 摘录` 结构化后缀携带并
    /// 在构造时再次净化）——它到达前端后以独立行展示，让网关返回的真实原因成为
    /// 用户可见文案；摘录不含票据/凭据（来源与净化见 `exv_core::atrust_browser`）。
    BrowserLoginFailed {
        code: Option<&'static str>,
        message: &'static str,
        server_detail: Option<String>,
    },
    /// 本轮宿主/原生步骤失败；保留净化的实际阶段、端口与系统错误，省略认证URL。
    NativeBrowserLoginFailed {
        code: Option<&'static str>,
        message: String,
        server_detail: Option<String>,
    },
    /// engine 连接门已给出稳定 reason id；不传递自由文本诊断。
    PreflightBlocked(String),
    /// 内部错误。
    Internal(String),
}


fn native_auth_message(message: &str) -> String {
    let mut output = String::new();
    let mut remaining = message;
    loop {
        let next = ["https://", "http://", "atrust://"].iter()
            .filter_map(|scheme| remaining.find(*scheme).map(|index| (index, scheme.len())))
            .min_by_key(|(index, _)| *index);
        let Some((start, prefix_len)) = next else { output.push_str(remaining); break; };
        output.push_str(&remaining[..start]);
        output.push_str("[认证地址已省略]");
        let tail = &remaining[start..];
        let end = tail.char_indices().find(|(index, value)| *index >= prefix_len
            && (value.is_whitespace() || matches!(*value, '）' | '，' | '；' | '"' | '\'')))
            .map_or(tail.len(), |(index, _)| index);
        remaining = &tail[end..];
    }
    exv_core::browser_auth::page_facts::sanitize_server_detail(&output)
}

impl AppError {
    /// 页面分类保持既有提示；宿主与原生失败保留实际操作事实和错误码。
    pub fn browser_auth_failed(error: exv_core::atrust_browser::BrowserAuthError) -> Self {
        if let exv_core::atrust_browser::BrowserAuthError::Auth { error: auth, .. } = &error
            && auth.code == exv_core::browser_auth::AuthErrorCode::CredentialRequired
        {
            return Self::CredentialRequired {
                code: Some("auto_login_no_credentials".into()),
                message: "credential_required".into(),
            };
        }
        if let exv_core::atrust_browser::BrowserAuthError::Host(message) = &error {
            return Self::NativeBrowserLoginFailed {
                code: None, message: native_auth_message(message), server_detail: None,
            };
        }
        if let exv_core::atrust_browser::BrowserAuthError::Auth { error: auth, compatibility_message } = &error {
            use exv_core::browser_auth::{AuthErrorCode as Code, AuthOperation};
            let page = matches!(auth.code, Code::ClientInstallRequired | Code::FailedPage | Code::Stalled | Code::GatewayThrottled);
            if !page || auth.native_code.is_some() {
                let stage = match auth.operation {
                    AuthOperation::Open => "启动认证渠道", AuthOperation::Navigate => "打开认证页",
                    AuthOperation::Observe => "读取认证进度", AuthOperation::Probe => "检查网关登录状态",
                    AuthOperation::Reuse => "复用已保存会话", AuthOperation::Inject => "提交学校登录信息",
                    AuthOperation::Extract => "取得网关认证会话", AuthOperation::Present => "显示认证窗口",
                    AuthOperation::Wait => "等待认证结果", AuthOperation::Close => "关闭认证通道",
                };
                let mut message = if compatibility_message.starts_with(auth.code.machine_code()) {
                    format!("{stage}失败（错误码：{}）。请重试本次连接。", auth.code.machine_code())
                } else { native_auth_message(compatibility_message) };
                if let Some(native) = auth.native_code { message.push_str(&format!("（系统错误码：{native}）")); }
                return Self::NativeBrowserLoginFailed {
                    code: Some(auth.code.machine_code()), message,
                    server_detail: auth.server_detail.as_deref().map(native_auth_message).filter(|s| !s.is_empty()),
                };
            }
        }
        let detail =
            if let exv_core::atrust_browser::BrowserAuthError::Auth { error: auth, .. } = &error {
                auth.server_detail
                    .as_deref()
                    .map(exv_core::browser_auth::page_facts::sanitize_server_detail)
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
        let mut result = Self::browser_login_failed(error.legacy_message());
        if let Self::BrowserLoginFailed { server_detail, .. } = &mut result
            && server_detail.is_none()
        {
            *server_detail = detail;
        }
        result
    }

    /// 浏览器认证路径只向界面交付已知固定提示；未知原文不跨过此边界。
    ///
    /// 页面分类结果以稳定机器码上报：`detail` 为机器码，或「机器码 + \x1f 分隔符 +
    /// 服务端消息摘录」的结构化形态（摘录只对已知机器码放行，且构造时再次净化；
    /// 未知码的任何后缀一律丢弃，不让自由文本冒充机器码）。
    pub fn browser_login_failed(detail: impl AsRef<str>) -> Self {
        let detail = detail.as_ref();
        const CODED_MESSAGES: &[(&str, &str)] = &[
            (
                exv_core::atrust_browser::BROWSER_AUTH_CLIENT_INSTALL_REQUIRED,
                "网关要求使用官方客户端完成认证；请重试或改用手动登录",
            ),
            (
                exv_core::atrust_browser::BROWSER_AUTH_FAILED_PAGE,
                "认证未通过；请重试或改用手动登录",
            ),
            (
                exv_core::atrust_browser::BROWSER_AUTH_STALLED,
                "认证长时间没有进展；请重试或改用手动登录",
            ),
            (
                exv_core::atrust_browser::BROWSER_AUTH_GATEWAY_THROTTLED,
                "网关提示登录受限（如短时间重复登录或并发登录冲突）；请稍候片刻再重试，或改用手动登录",
            ),
        ];
        // 结构化切分：摘录经共享净化（剥控制字符+截断），空摘录归一为 `None`。
        let (code_part, server_detail) =
            match detail.split_once(exv_core::atrust_browser::SERVER_DETAIL_SEPARATOR) {
                Some((code, detail_text)) => (
                    code,
                    Some(exv_core::browser_auth::page_facts::sanitize_server_detail(
                        detail_text,
                    ))
                    .filter(|text| !text.is_empty()),
                ),
                None => (detail, None),
            };
        if let Some((code, message)) = CODED_MESSAGES
            .iter()
            .copied()
            .find(|(candidate, _)| *candidate == code_part)
        {
            return Self::BrowserLoginFailed {
                code: Some(code),
                message,
                server_detail,
            };
        }
        const MESSAGES: &[&str] = &[
            "浏览器登录状态不可用",
            "正在等待浏览器登录，请完成或取消当前登录",
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
            .find(|message| *message == code_part)
            .unwrap_or("浏览器登录未完成，请重试；若持续出现请反馈应用日志。");
        // 未知机器码（或不带码的原文）不携带任何服务端摘录——白名单纪律。
        Self::BrowserLoginFailed {
            code: None,
            message,
            server_detail: None,
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (kind, code, message, server_detail) = match self {
            Self::NotWired(message) => ("not_wired", None, message.as_str(), None),
            Self::CoreUnreachable(message) => ("core_unreachable", None, message.as_str(), None),
            Self::CoreWakeFailed(message) => ("core_wake_failed", None, message.as_str(), None),
            Self::ServiceNotRunning(message) => {
                ("service_not_running", None, message.as_str(), None)
            }
            Self::ServiceConnectFailed(message) => {
                ("service_connect_failed", None, message.as_str(), None)
            }
            Self::CompatibilityModeUnavailable(message) => (
                "compatibility_mode_unavailable",
                None,
                message.as_str(),
                None,
            ),
            Self::ProtocolUnavailable(message) => {
                ("protocol_unavailable", None, message.as_str(), None)
            }
            Self::CredentialRequired { code, message } => (
                "credential_required",
                code.as_deref(),
                message.as_str(),
                None,
            ),
            Self::BrowserLoginFailed {
                code,
                message,
                server_detail,
            } => (
                "browser_login_failed",
                *code,
                *message,
                server_detail.as_deref(),
            ),
            Self::NativeBrowserLoginFailed { code, message, server_detail } => (
                "browser_login_failed", *code, message.as_str(), server_detail.as_deref(),
            ),
            Self::PreflightBlocked(reasons) => (
                "preflight_blocked",
                Some(reasons.as_str()),
                "连接前检查未通过",
                None,
            ),
            Self::Internal(message) => ("internal", None, message.as_str(), None),
        };
        let field_count =
            2 + usize::from(code.is_some()) + usize::from(server_detail.is_some());
        let mut output = serializer.serialize_struct("AppError", field_count)?;
        output.serialize_field("kind", kind)?;
        if let Some(code) = code {
            output.serialize_field("code", code)?;
        }
        output.serialize_field("message", message)?;
        if let Some(server_detail) = server_detail {
            output.serialize_field("server_detail", server_detail)?;
        }
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
            Self::BrowserLoginFailed {
                message,
                server_detail,
                ..
            } => match server_detail {
                Some(server_detail) => {
                    // 摘录已净化（无控制字符/已截断），进入日志不构成注入面。
                    write!(f, "browser login failed: {message}（网关返回：{server_detail}）")
                }
                None => write!(f, "browser login failed: {message}"),
            },
            Self::NativeBrowserLoginFailed { message, server_detail, .. } => {
                write!(f, "browser login failed: {message}")?;
                if let Some(detail) = server_detail { write!(f, "（网关返回：{detail}）")?; }
                Ok(())
            }
            Self::PreflightBlocked(reasons) => write!(f, "preflight blocked: {reasons}"),
            Self::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for AppError {}

