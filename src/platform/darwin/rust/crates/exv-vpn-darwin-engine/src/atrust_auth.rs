//! Engine 侧 aTrust 认证编排（M2：Underlay 与认证接入；设计 §4.4）。
//!
//! 本模块是 darwin engine 对 Common `browser_handoff` 六请求编排的薄组合：
//! 业务状态机、阶段错误、脱敏与认证结果全部在 Common；宿主只做选值——
//!
//! - **身份值来自信封字段**：`AuthClientIdentity::new(CLIENT_TYPE, <信封 platform>,
//!   <信封 language>)`——engine 生产代码**零 platform 硬编码**（M0 证据 §6：暂定值
//!   `Mac` 的唯一来源是 core 注入的信封字段；终局裁决在 M3 每流授权）。
//! - **resource_request 字节来自信封**（宿主产品选择，M0 定版；本模块不内置）。
//! - **出口注入**：调用方传入 [`crate::underlay::DarwinPhysicalUnderlay`]（或测试
//!   的回环 underlay）。
//!
//! ## 成功/失败边界（M2 钉死、M3 消费）
//!
//! - **成功边界** = [`authenticate_atrust_session`] 返回
//!   [`AuthenticatedAtrustSession`]（六请求编排走完：源复核→接收→票据→兑换→
//!   复核→资源目录）。M3 起调用方（`bootstrap_runtime` 的 aTrust 连接任务）把
//!   该值直接交给数据面组装（节点 TLS→L3 握手→run_packets——B4 消解，认证
//!   完成不再发布中间态终态事件）。
//! - **失败边界** = 任一阶段失败返回 [`AtrustConnectError`]；阶段经
//!   [`AtrustConnectError::reason`] 映射稳定 reason 码（进日志/trace，不含秘密），
//!   阶段→`VpnError`（code/stage）经 [`auth_stage_vpn_error`] 映射既有 Connecting
//!   阶段事件（认证全部发生在 `ConnectingControl` 阶段——隧道协商前）。
//! - 认证成功后的阶段推进与终态发布由调用方执行，本模块不发布事件。
//!
//! ## 密闭夹具边界（复审 E1 机制的 darwin 落地）
//!
//! 本模块的合成两级PKI仅服务回环夹具。既有生产验证器拒绝用例仍不注入测试根；
//! E3新增的真实Source isLogin=0消费用例，经dev依赖启用Common已有
//! `internal-test-trust`，只对该测试client按实例注入合成根，执行完整证书链和主机名验证。
//! 生产依赖与默认平台验证器不变，不提供禁用校验开关。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::fmt;
use std::sync::Arc;

use exv_vpn_atrust::Underlay;
use exv_vpn_atrust::auth::AuthClientOptions;
use exv_vpn_atrust::auth::browser_handoff::{
    self, AtrustAuthError, AuthStage, AuthenticatedAtrustSession,
};
use exv_vpn_atrust::auth::browser_session::BrowserSession;
use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_wire::generated as wire;

/// aTrust 协议的 clientType 字段（协议形状常量，与 platform 身份值无关；win32
/// 同款取值——M0 证据三参恒定 `clientType=SDPClient`）。
const CLIENT_TYPE: &str = "SDPClient";

/// aTrust 采纳身份的非秘密用户名占位（设计 §7：`PipelineIdentity{server: 网关,
/// username: "-atrust", protocol}`——判别子不得用 SID/会话秘密或其哈希；M2 的
/// aTrust 连接不达持有态，本占位用于 trace/身份比较面的一致形状）。
pub(crate) const ATRUST_IDENTITY_USERNAME_PLACEHOLDER: &str = "-atrust";

/// M3/M6 aTrust 连接的稳定 reason 码（进日志/trace；全部非秘密）。
pub(crate) mod reason {
    /// 认证编排成功（六请求完成，会话已验证）。
    pub const AUTH_COMPLETED: &str = "DARWIN_ATRUST_AUTH_COMPLETED";
    /// 数据面组装成功（L3 握手完成，`run_packets` worker 运行，双泵接管 utun）。
    /// M2 的 `DARWIN_ATRUST_DATA_PLANE_PENDING` 中间态已随 M3 数据面接线退役
    /// （B4 消解：Connected 由管线真实发布）。
    pub const DATA_PLANE_READY: &str = "DARWIN_ATRUST_DATA_PLANE_READY";
    /// 物理出口探测/构造失败（Underlay 建立前）。
    pub const EGRESS_FAILED: &str = "DARWIN_ATRUST_EGRESS_FAILED";
    /// 信封身份字段（platform/language）无法构造协议身份。
    pub const IDENTITY_INVALID: &str = "DARWIN_ATRUST_IDENTITY_INVALID";
    /// 信封 browser_session wire 无法还原为 Common BrowserSession。
    pub const SESSION_WIRE_INVALID: &str = "DARWIN_ATRUST_SESSION_WIRE_INVALID";
    /// R3（M6）：会话已失效——Source 复核 isLogin=0（非传输错误）。初始连接
    /// （缓存候选复核失败）与 R1 重拨（驻留材料复核失败）同判：直转终态链，
    /// 不耗重拨预算、不循环。
    pub const SESSION_INVALIDATED: &str = "DARWIN_ATRUST_SESSION_INVALIDATED";
}

/// 跨仓契约（M4 A4 兜底方案，M6 钉死）：Common `browser_handoff` 对 Source 阶段
/// isLogin=0 的 detail 字符串。Common 未提供结构化 `is_login=false` 变体——按治理
/// 已登记停线裁定请求（不触发停线），该字符串是 Source 阶段内区分「会话失效」与
/// 「传输/构造错误」的唯一判别面。Common 侧自有测试
/// `source_not_logged_in_stops_before_ticket` 以同一 Display 字符串断言（Common
/// 零改动侧的契约锁，本仓不触碰 Common）；darwin 侧以本常量与
/// [`source_session_invalidated`] 的密闭测试锁消费面。Common 若改动该文案，
/// R1 认证失效短路与 R3 终态映射会静默退化为普通认证失败——两侧测试同时变红
/// 即为契约漂移信号。
pub(crate) const SOURCE_NOT_AUTHENTICATED_CONTRACT: &str = "browser session is not authenticated";

/// 判定一次 Common 认证失败是否为「源会话已失效」（Source 阶段 isLogin=0——非
/// 传输错误）。R1 重拨的认证失效短路与 R3 终态映射共用本判别；匹配面是
/// [`AtrustAuthError`] 的 Display 文本（`"aTrust Source: {detail}"`——detail 字段
/// 在 Common 为私有，Display 是唯一公开投影）。
pub(crate) fn source_session_invalidated(auth: &AtrustAuthError) -> bool {
    std::error::Error::source(auth).is_none()
        && source_not_authenticated_detail(auth.stage, &auth.to_string())
}

/// 判别的纯函数面（stage + Display 文本）——密闭测试的注入点：`AtrustAuthError`
/// 无公开构造器（Common 只经 `authenticate_browser_session` 产出），测试以 Common
/// Display 形态的样本驱动同一判别逻辑。
fn source_not_authenticated_detail(stage: AuthStage, display: &str) -> bool {
    stage == AuthStage::Source && display.contains(SOURCE_NOT_AUTHENTICATED_CONTRACT)
}

/// Common `AuthStage` → 稳定 reason 码（typed 细节码，承 reason-id 纪律）。
#[must_use]
pub(crate) fn auth_stage_reason(stage: AuthStage) -> &'static str {
    match stage {
        AuthStage::Gateway => "DARWIN_ATRUST_AUTH_STAGE_GATEWAY",
        AuthStage::Source => "DARWIN_ATRUST_AUTH_STAGE_SOURCE",
        AuthStage::Receiver => "DARWIN_ATRUST_AUTH_STAGE_RECEIVER",
        AuthStage::Ticket => "DARWIN_ATRUST_AUTH_STAGE_TICKET",
        AuthStage::Exchange => "DARWIN_ATRUST_AUTH_STAGE_EXCHANGE",
        AuthStage::Verified => "DARWIN_ATRUST_AUTH_STAGE_VERIFIED",
        AuthStage::Resources => "DARWIN_ATRUST_AUTH_STAGE_RESOURCES",
    }
}

/// Common `AuthStage` → 既有 Connecting 阶段 `VpnError`（只填 code/stage；凭据、
/// Cookie 与主机名永不入错误）。认证全部发生在隧道协商前，stage 恒
/// `ConnectingControl`；网关/源会话问题属认证材料问题（Unauthorized），其余交换
/// 阶段保持协议通用码（EffectUnknown）。
#[must_use]
pub(crate) fn auth_stage_vpn_error(stage: AuthStage) -> wire::VpnError {
    use wire::{ErrorCode, ErrorStage};
    let code = match stage {
        AuthStage::Gateway | AuthStage::Source => ErrorCode::Unauthorized,
        AuthStage::Receiver
        | AuthStage::Ticket
        | AuthStage::Exchange
        | AuthStage::Verified
        | AuthStage::Resources => ErrorCode::EffectUnknown,
    };
    wire::VpnError {
        code: code as i32,
        stage: ErrorStage::ConnectingControl as i32,
        native: (code == ErrorCode::Unauthorized)
            .then(|| exv_vpn_darwin_ipc::atrust_auth_error::native(false)),
        ..Default::default()
    }
}

/// M2 aTrust 连接失败的 typed 分类；`Display` 只含稳定 reason 码（秘密零输出——
/// Common 错误的 detail 已脱敏，本类型不复制其文本，调用方日志经 reason 码关联）。
#[derive(Debug)]
pub(crate) enum AtrustConnectError {
    /// 信封身份字段（platform/language）非法（空/控制字符——构造
    /// `AuthClientIdentity` 失败）。
    IdentityInvalid,
    /// 信封 browser_session wire 无法还原（信封构造期已校验过；此处为消费期
    /// fail-closed 兜底）。
    SessionWireInvalid,
    /// Common 六请求编排失败（阶段见 `AtrustAuthError::stage`）。
    Auth(AtrustAuthError),
}

impl AtrustConnectError {
    /// 稳定 reason 码（Auth 变体按 Common 阶段细分）。
    #[must_use]
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::IdentityInvalid => reason::IDENTITY_INVALID,
            Self::SessionWireInvalid => reason::SESSION_WIRE_INVALID,
            Self::Auth(error) => auth_stage_reason(error.stage),
        }
    }

    /// 既有 Connecting 阶段事件的 `VpnError`（code/stage 两维）。
    #[must_use]
    pub(crate) fn vpn_error(&self) -> wire::VpnError {
        match self {
            Self::IdentityInvalid | Self::SessionWireInvalid => wire::VpnError {
                code: wire::ErrorCode::InvalidInput as i32,
                stage: wire::ErrorStage::ConnectingControl as i32,
                ..Default::default()
            },
            Self::Auth(error) => {
                let mut projected = auth_stage_vpn_error(error.stage);
                if let Some(code) = std::error::Error::source(error)
                    .and_then(|cause| cause.downcast_ref::<exv_vpn_atrust::auth::AuthHttpError>())
                    .and_then(exv_vpn_atrust::auth::AuthHttpError::raw_os_error)
                {
                    projected.native = Some(wire::RedactedNativeError {
                        category: wire::NativeErrorCategory::Transport as i32,
                        namespace: wire::NativeErrorNamespace::PosixErrno as i32,
                        code: i64::from(code),
                    });
                }
                projected
            }
        }
    }
}

impl fmt::Display for AtrustConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for AtrustConnectError {}

/// 由身份字段构造协议身份（显式注入、无默认、无硬编码——M0 暂定值的唯一消费链
/// 是 core→信封→本函数）。
fn identity_from_values(
    platform: &str,
    language: &str,
) -> Result<AuthClientIdentity, AtrustConnectError> {
    AuthClientIdentity::new(CLIENT_TYPE, platform, language)
        .map_err(|_| AtrustConnectError::IdentityInvalid)
}

/// 以材料件（网关/会话 wire/身份值/资源字节）执行 Common 六请求编排——M6 起
/// 唯一认证入口：初始连接与 R1 重拨都从材料件（apply 受理时捕获的
/// [`crate::bootstrap_runtime::AtrustMaterials`]）取值，走**同一条**认证路径
/// （isLogin 复核在 Source 阶段内部发生——失效判别见 [`source_session_invalidated`]）。
///
/// # Errors
///
/// 同 [`authenticate_atrust_session`]。
pub(crate) async fn authenticate_with_materials(
    gateway: &str,
    browser_session_wire: &[u8],
    platform: &str,
    language: &str,
    resource_request: &[u8],
    underlay: Arc<dyn Underlay>,
) -> Result<AuthenticatedAtrustSession, AtrustConnectError> {
    let identity = identity_from_values(platform, language)?;
    let browser = BrowserSession::decode(browser_session_wire)
        .map_err(|_| AtrustConnectError::SessionWireInvalid)?;
    let options = AuthClientOptions {
        user_agent: browser.user_agent().to_owned(),
        ..AuthClientOptions::new(identity)
    };
    browser_handoff::authenticate_browser_session(
        browser,
        gateway,
        underlay,
        options,
        resource_request,
    )
    .await
    .map_err(AtrustConnectError::Auth)
}

/// 仅在 Common 明确判定源会话失效时使用，普通 Unauthorized 不触发缓存遗忘。
pub(crate) fn session_invalidated_vpn_error() -> wire::VpnError {
    wire::VpnError {
        native: Some(exv_vpn_darwin_ipc::atrust_auth_error::native(true)),
        ..auth_stage_vpn_error(AuthStage::Source)
    }
}
