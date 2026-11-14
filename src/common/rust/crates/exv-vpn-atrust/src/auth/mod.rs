//! 原生票据兑换与资源 HTTP 客户端；浏览器候选会话的完整交接编排见 [`browser_handoff`]，
//! Windows 生产路径由 `win32-engine/src/atrust_auth.rs` 的薄组合消费（经 `atrust_prepare.rs`
//! 进入运行时），Darwin 尚未接线。
//! 接口依据 docs/research/atrust/controlled-browser-debug-session.md §7.6。
//! 宿主注入 TLS 之下的裸 TCP 流；库执行平台证书验证，不提供默认拨号器。
//! 请求路径由宿主显式声明的 [`AuthClientIdentity`] 构造，Common 不提供平台默认值。

pub mod browser_handoff;
pub mod browser_auth_policy;
pub mod browser_session;
pub mod client_identity;
pub mod origin;
pub mod signing;
pub(crate) mod transport_error;

use self::client_identity::AuthClientIdentity;
use self::origin::GatewayOrigin;
use crate::{ConnectPurpose, OutboundTarget, Secret, Underlay};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Request};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use zeroize::{Zeroize, Zeroizing};

const MAX_COOKIE_BYTES: usize = 32 * 1024;

/// 认证原失败分类与有界脱敏传输原因；不含网关正文、Cookie 或票据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthHttpError {
    InvalidConfiguration,
    InvalidCookie,
    Timeout,
    Underlay(crate::UnderlayError),
    Tls {
        operation: &'static str,
        io_kind: Option<std::io::ErrorKind>,
        raw_os_error: Option<i32>,
        detail: String,
    },
    Http {
        operation: &'static str,
        io_kind: Option<std::io::ErrorKind>,
        raw_os_error: Option<i32>,
        detail: String,
    },
    HttpStatus {
        status: u16,
    },
    SizeLimit,
    Protocol,
    Rejected {
        code: i64,
    },
    MissingCsrf,
    MissingSignKey,
}

impl AuthHttpError {
    /// 仅返回原 I/O 对象实际携带的系统码，HTTP/业务码不进入该槽。
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Underlay(error) => error.raw_os_error(),
            Self::Tls { raw_os_error, .. } | Self::Http { raw_os_error, .. } => *raw_os_error,
            _ => None,
        }
    }
    /// 从原始 TLS I/O 错误提取系统码与有界脱敏原因，供宿主捕获点复用。
    #[must_use]
    pub fn tls_io(operation: &'static str, error: &std::io::Error) -> Self {
        Self::Tls {
            operation,
            io_kind: Some(error.kind()),
            raw_os_error: error.raw_os_error(),
            detail: transport_error::io_detail(error),
        }
    }
    /// 从 TLS 配置错误提取有界脱敏原因，供宿主捕获点复用。
    #[must_use]
    pub fn tls_config(operation: &'static str, error: &rustls::Error) -> Self {
        Self::Tls {
            operation,
            io_kind: None,
            raw_os_error: None,
            detail: transport_error::tls_detail(error),
        }
    }

    /// 输出既有传输构造已经脱敏的诊断字段；非传输错误不产生额外诊断。
    /// `Tls` 与 `Http` 的 `detail` 沿用各自捕获点的有界脱敏合同，保留安全的 TLS 类型名。
    #[must_use]
    pub fn transport_diagnostic(&self) -> Option<String> {
        match self {
            Self::Tls {
                operation,
                io_kind,
                raw_os_error,
                detail,
            }
            | Self::Http {
                operation,
                io_kind,
                raw_os_error,
                detail,
            } => Some(format!(
                "operation={operation} io_kind={io_kind:?} raw_os_error={raw_os_error:?} detail={detail}"
            )),
            _ => None,
        }
    }
}
impl std::fmt::Display for AuthHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration => f.write_str("invalid gateway endpoint or client options"),
            Self::InvalidCookie => f.write_str("invalid gateway cookie"),
            Self::Timeout => f.write_str("authentication request timed out"),
            Self::Underlay(error) => {
                write!(f, "underlay connection failed: {error}")?;
                if let Some(operation) = error.operation() {
                    write!(
                        f,
                        "; operation={operation}; io={:?}; os={:?}; {}",
                        error.io_kind(),
                        error.raw_os_error(),
                        error.detail().unwrap_or("")
                    )?;
                }
                Ok(())
            }
            Self::Tls {
                operation,
                io_kind,
                raw_os_error,
                detail,
            } => write!(
                f,
                "TLS verification or handshake failed; operation={operation}; io={io_kind:?}; os={raw_os_error:?}; {detail}"
            ),
            Self::Http {
                operation,
                io_kind,
                raw_os_error,
                detail,
            } => write!(
                f,
                "HTTP transport failed; operation={operation}; io={io_kind:?}; os={raw_os_error:?}; {detail}"
            ),
            Self::HttpStatus { status } => write!(f, "gateway returned HTTP status {status}"),
            Self::SizeLimit => f.write_str("response or request exceeds configured limit"),
            Self::Protocol => f.write_str("gateway returned malformed protocol response"),
            Self::Rejected { code } => write!(f, "gateway rejected request (code {code})"),
            Self::MissingCsrf => f.write_str("authConfig must provide CSRF token first"),
            Self::MissingSignKey => {
                f.write_str("device sign key unavailable for signed gateway request")
            }
        }
    }
}
impl std::error::Error for AuthHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Underlay(error) => Some(error),
            _ => None,
        }
    }
}

/// 所有 HTTP 操作共用的有限预算；超时覆盖拨号、TLS、响应头和响应正文。
/// 客户端身份由宿主显式提供，因此没有隐式默认构造；预算取值保持原默认。
#[derive(Debug, Clone)]
pub struct AuthClientOptions {
    pub user_agent: String,
    pub timeout: Duration,
    pub max_response_bytes: usize,
    pub identity: AuthClientIdentity,
    /// 按实例的测试信任注入：本 crate 单测（`cfg(test)`）直改字段；跨 crate 集成测试
    /// 经 `internal-test-trust` feature 暴露的 [`Self::set_test_trust`] 注入。常规
    /// （非测试、未启用该 feature 的）构建不存在该字段，也没有全局开关。注入的根集合
    /// 仍走完整链与主机名校验，只是信任锚来自测试夹具而非宿主系统。
    #[cfg(any(test, feature = "internal-test-trust"))]
    #[doc(hidden)]
    pub test_trust: Option<Arc<rustls::RootCertStore>>,
}
impl AuthClientOptions {
    /// 以宿主声明的身份构造选项；预算取原默认值：UA 为官方 aTrust UA
    /// （[`client_identity::ATRUST_OFFICIAL_USER_AGENT`]，不再使用 `EXV-aTrust/0.1` 指纹）、
    /// 30 秒、8 MiB。
    #[must_use]
    pub fn new(identity: AuthClientIdentity) -> Self {
        Self {
            user_agent: client_identity::ATRUST_OFFICIAL_USER_AGENT.into(),
            timeout: Duration::from_secs(30),
            max_response_bytes: 8 * 1024 * 1024,
            identity,
            #[cfg(any(test, feature = "internal-test-trust"))]
            test_trust: None,
        }
    }

    /// `internal-test-trust` feature 专用（跨 crate 集成测试按实例注入显式信任根，
    /// 如消费方 crate 对回环假网关自建 CA 的测试）：与本 crate 单测直改 `test_trust`
    /// 字段同一语义——注入根仍执行完整链与主机名校验，不提供禁用校验的开关。常规
    /// 构建未启用该 feature，本方法不存在。
    #[cfg(feature = "internal-test-trust")]
    #[doc(hidden)]
    pub fn set_test_trust(&mut self, roots: Arc<rustls::RootCertStore>) {
        self.test_trust = Some(roots);
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub is_login: bool,
    /// 网关 `authConfig.data.firstAuth`（登录入口路径数组）。仅未登录时用于初始导航；
    /// 附加字段，消费方仍只读 `is_login`，不改变既有成功/失败语义，也不改变任何 wire 形状。
    pub first_auth: Option<Vec<String>>,
}

/// 网关 authConfig 下发的 `antiMITMAttackData` 逐字段透传（规格 2.1 字段表）。
///
/// 全部字段由网关生成并原样回传给本机 agent（门户 JS `Ji()` 只透传，不加工），因此
/// 结构只做形状承载，不解释语义。字段按网关下发顺序声明，序列化结果与下发形态同序，
/// 供 S2 组装 `/v1/service/checkMITMAttack` 请求体直接复用。各字段可缺省（网关对
/// 未知设备可能不颁发设备数据，规格 4.3 实验 B 待判），缺省字段序列化时省略。
///
/// 其中公钥/挑战/票据等属 `error.rs` 规则 1 的敏感材料：`Debug` 只打印字段存在性，
/// `Drop` 零化全部字符串字段；`enable`/`antiMITMRequest` 是策略开关，非凭据，
/// 以不透明 JSON 值承载以容忍数字/字符串两种 wire 形态。
#[derive(Default, Deserialize, serde::Serialize)]
pub struct AntiMitmAttackData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<serde_json::Value>,
    #[serde(
        rename = "devicePubKeyMod",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub device_pub_key_mod: Option<String>,
    #[serde(
        rename = "devicePubKeyExp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub device_pub_key_exp: Option<String>,
    #[serde(rename = "rsaCert", default, skip_serializing_if = "Option::is_none")]
    pub rsa_cert: Option<String>,
    #[serde(
        rename = "sm2encCert",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sm2enc_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
    #[serde(
        rename = "encryptedChallenge",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub encrypted_challenge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(
        rename = "antiMITMRequest",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anti_mitm_request: Option<serde_json::Value>,
    #[serde(rename = "mitmSig", default, skip_serializing_if = "Option::is_none")]
    pub mitm_sig: Option<String>,
}

impl Drop for AntiMitmAttackData {
    fn drop(&mut self) {
        for field in [
            &mut self.device_pub_key_mod,
            &mut self.device_pub_key_exp,
            &mut self.rsa_cert,
            &mut self.sm2enc_cert,
            &mut self.challenge,
            &mut self.encrypted_challenge,
            &mut self.ticket,
            &mut self.mitm_sig,
        ]
        .into_iter()
        .flatten()
        {
            field.zeroize();
        }
    }
}

impl std::fmt::Debug for AntiMitmAttackData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只打印字段存在性；值属敏感材料（error.rs 规则 1）。
        f.debug_struct("AntiMitmAttackData")
            .field("enable", &self.enable.is_some())
            .field("device_pub_key_mod", &self.device_pub_key_mod.is_some())
            .field("device_pub_key_exp", &self.device_pub_key_exp.is_some())
            .field("rsa_cert", &self.rsa_cert.is_some())
            .field("sm2enc_cert", &self.sm2enc_cert.is_some())
            .field("challenge", &self.challenge.is_some())
            .field("encrypted_challenge", &self.encrypted_challenge.is_some())
            .field("ticket", &self.ticket.is_some())
            .field("anti_mitm_request", &self.anti_mitm_request.is_some())
            .field("mitm_sig", &self.mitm_sig.is_some())
            .finish()
    }
}

/// [`AuthClient::auth_config_detail`] 的返回：会话状态之外附带你我完成设备握手与
/// 票据核销所需的网关下发材料（规格 4.1）。`guid`/`unitid` 是设备/租户标识
/// （协议字段，非凭据）；`csrf` 为新构造副本，客户端内部另存一份供后续请求使用。
#[derive(Debug)]
pub struct AuthConfigDetail {
    pub status: AuthStatus,
    pub csrf: Option<Secret>,
    pub anti_mitm: Option<AntiMitmAttackData>,
    pub guid: Option<String>,
    pub unitid: Option<String>,
}

/// [`AuthClient::auth_check`] 的结果：网关业务码原样上抛。实测取值：`0`（推定成功）、
/// `75500001`（当前认证已超时/未登录，R1 §5.1）。`code=0` 即成功的语义未在真实网关
/// 对成功路径钉死，S6 验收复核。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthCheck {
    pub code: i64,
}

impl AuthCheck {
    /// 业务码是否为 0（推定成功；见类型文档的 S6 复核注记）。
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.code == 0
    }
}

/// [`AuthClient::exchange_sid_ticket`] 的返回：`sessionIdExchange` 响应 `data` 内的
/// 会话字段（规格 2.5：`sid` 以 JSON 体落回 agent/消费方侧，而非（仅）Set-Cookie）。
///
/// 官方样本键集：`sid`/`sid.sig`/`sid-legacy`/`sid-legacy.sig`/`straceid`/`straceid.sig`
/// （另有非凭据 `lang`，不在此承载）。全部键可缺省——真实网关的完整键集以 S6 验收为准。
/// 值属会话凭据（`error.rs` 规则 1）：`Debug` 只打印字段存在性；各字段由 [`Secret`]
/// 自身的 `Drop` 零化（本结构无需额外 Drop）。
#[derive(Default)]
pub struct SessionExchange {
    /// 本次票据兑换响应中的非秘密账户；未返回时保持缺值。
    pub authenticated_username: Option<String>,
    pub sid: Option<Secret>,
    pub sid_sig: Option<Secret>,
    pub sid_legacy: Option<Secret>,
    pub sid_legacy_sig: Option<Secret>,
    pub straceid: Option<Secret>,
    pub straceid_sig: Option<Secret>,
}

impl SessionExchange {
    /// 解析 `sessionIdExchange` 响应正文（`{"code":0,"data":{...}}` 信封）。
    ///
    /// S7 无头 CAS 链（宿主 crate 的 `atrust_cas_login`）以普通 Chrome UA +
    /// `SDPBrowserClient` 身份直接发 `sessionIdExchange`，不经 [`AuthClient`] 的
    /// 桌面会话；该入口把既有解析（业务码校验 + 键集提取 + 零化）开放为可复用函数，
    /// 不改变 [`AuthClient::exchange_sid_ticket`] 的任何行为。
    /// # Errors
    /// 业务码非 0 或 JSON 形状不符时返回脱敏分类错误。
    pub fn parse(raw: &[u8]) -> Result<Self, AuthHttpError> {
        parse_session_exchange(raw)
    }

    /// 是否持有任一会话字段（空响应等价于未取得会话）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sid.is_none()
            && self.sid_sig.is_none()
            && self.sid_legacy.is_none()
            && self.sid_legacy_sig.is_none()
            && self.straceid.is_none()
            && self.straceid_sig.is_none()
    }

    /// 渲染为 WebView 可注入的 Cookie 请求头形态（`sid=…; sid.sig=…; …`，按固定顺序，
    /// 缺省键跳过）。供 S2/S3 把 agent 侧会话可选注入内嵌窗口 cookie jar；结果由零化
    /// 容器持有，调用方负责不落日志。
    #[must_use]
    pub fn cookie_header(&self) -> Zeroizing<String> {
        let mut header = Zeroizing::new(String::new());
        for (name, value) in [
            ("sid", &self.sid),
            ("sid.sig", &self.sid_sig),
            ("sid-legacy", &self.sid_legacy),
            ("sid-legacy.sig", &self.sid_legacy_sig),
            ("straceid", &self.straceid),
            ("straceid.sig", &self.straceid_sig),
        ] {
            if let Some(value) = value {
                if !header.is_empty() {
                    header.push_str("; ");
                }
                header.push_str(name);
                header.push('=');
                header.push_str(value.expose());
            }
        }
        header
    }
}

impl std::fmt::Debug for SessionExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只打印字段存在性；值属会话凭据（error.rs 规则 1）。
        f.debug_struct("SessionExchange")
            .field("sid", &self.sid.is_some())
            .field("sid_sig", &self.sid_sig.is_some())
            .field("sid_legacy", &self.sid_legacy.is_some())
            .field("sid_legacy_sig", &self.sid_legacy_sig.is_some())
            .field("straceid", &self.straceid.is_some())
            .field("straceid_sig", &self.straceid_sig.is_some())
            .finish()
    }
}

/// 解析 `sessionIdExchange` 响应为 [`SessionExchange`]；提取为独立函数以便离线单测。
/// 中间结构逐字段零化，`SessionExchange` 自身的 `Drop` 负责后续持有期。
fn parse_session_exchange(raw: &[u8]) -> Result<SessionExchange, AuthHttpError> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(rename = "onlineInfo", default)]
        online_info: serde_json::Value,
        #[serde(default)]
        sid: Option<String>,
        #[serde(rename = "sid.sig", default)]
        sid_sig: Option<String>,
        #[serde(rename = "sid-legacy", default)]
        sid_legacy: Option<String>,
        #[serde(rename = "sid-legacy.sig", default)]
        sid_legacy_sig: Option<String>,
        #[serde(default)]
        straceid: Option<String>,
        #[serde(rename = "straceid.sig", default)]
        straceid_sig: Option<String>,
    }
    impl Drop for Wire {
        fn drop(&mut self) {
            for field in [
                &mut self.sid,
                &mut self.sid_sig,
                &mut self.sid_legacy,
                &mut self.sid_legacy_sig,
                &mut self.straceid,
                &mut self.straceid_sig,
            ]
            .into_iter()
            .flatten()
            {
                field.zeroize();
            }
        }
    }
    let mut wire: Wire = decode(raw)?;
    // `Wire` 实现 Drop（逐字段零化），不能按字段搬移；用 `take` 换出后由 `Secret`
    // 自身的 Drop 承接零化义务（换出的 String 即刻进入 Secret）。
    let into = |value: Option<String>| value.map(Secret::new);
    Ok(SessionExchange {
        authenticated_username: wire.online_info.get("username")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|username| !username.is_empty())
            .map(str::to_owned),
        sid: into(std::mem::take(&mut wire.sid)),
        sid_sig: into(std::mem::take(&mut wire.sid_sig)),
        sid_legacy: into(std::mem::take(&mut wire.sid_legacy)),
        sid_legacy_sig: into(std::mem::take(&mut wire.sid_legacy_sig)),
        straceid: into(std::mem::take(&mut wire.straceid)),
        straceid_sig: into(std::mem::take(&mut wire.straceid_sig)),
    })
}

/// `request()` 的签名头策略。既有端点（authConfig/ticketExchange/sessionIdExchange/
/// clientResource）保持 [`RequestSigning::None`] 不变：官方虽对 12-URI 白名单（规格 2.3）
/// 携带 `X-Request-Sig`，但网关对这些端点不强制（R1 实测无签名仍 code=0），为不改变
/// 既有生产 wire 行为，白名单补签留给 S2/S6 裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestSigning {
    /// 不加签名头。
    None,
    /// 环境上报族（endpointStrategy/reportEnv）：`X-Request-Sig` + `x-sdp-signature` +
    /// `x-sdp-random`；缺签名会被网关以 10000008 拒绝（R1 §5.2）。
    EnvReport,
}

/// 单一固定网关作用域会话；不跟随重定向，也不向其他源发送 Cookie。
pub struct AuthClient {
    host: String,
    authority: String,
    port: u16,
    /// `x-sdp-rid = base64(规范化网关主机名)`，与官方客户端一致，构造期固定。
    sdp_rid: String,
    /// 门户 `Referer`：`https://<网关>/portal/`；本客户端只访问该网关 origin。
    referer: String,
    underlay: Arc<dyn Underlay>,
    options: AuthClientOptions,
    cookies: BTreeMap<String, Secret>,
    csrf: Option<Secret>,
    /// 设备请求签名密钥（signKey，32 字节原始值）。由 authConfig 的
    /// `antiMITMAttackData` 派生（[`signing::derive_from_anti_mitm`]）或宿主经
    /// [`Self::install_sign_key`] 注入；按握手逐次更新，不落盘。
    sign_key: Option<Zeroizing<[u8; 32]>>,
}

impl AuthClient {
    /// 创建固定网关客户端。
    /// # Errors
    /// 非 HTTPS、带用户信息或路径的端点以及无效预算返回配置错误。
    pub fn new(
        endpoint: &str,
        underlay: Arc<dyn Underlay>,
        options: AuthClientOptions,
    ) -> Result<Self, AuthHttpError> {
        let origin =
            GatewayOrigin::parse(endpoint).map_err(|_| AuthHttpError::InvalidConfiguration)?;
        if options.timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 64 * 1024 * 1024
            || options.user_agent.len() > 4096
            || HeaderValue::from_str(&options.user_agent).is_err()
        {
            return Err(AuthHttpError::InvalidConfiguration);
        }
        let host = origin.host().to_owned();
        let port = origin.port();
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        // L1 协议身份：`x-sdp-rid = base64(网关主机名)`；主机名已由 `GatewayOrigin` 规范化为
        // 小写、IPv6 去方括号，与官方客户端一致。
        let sdp_rid = base64_standard(host.as_bytes());
        // 门户 Referer 指向同一网关 origin 的 `/portal/`；默认端口 443 不显式写出。
        let host_url = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        let referer = if port == 443 {
            format!("https://{host_url}/portal/")
        } else {
            format!("https://{host_url}:{port}/portal/")
        };
        Ok(Self {
            host,
            authority,
            port,
            sdp_rid,
            referer,
            underlay,
            options,
            cookies: BTreeMap::new(),
            csrf: None,
            sign_key: None,
        })
    }

    /// 仅供受信任宿主导入当前网关的 Cookie 请求头；原子替换，随后需 authConfig 验证会话。
    /// # Errors
    /// 非法 Cookie 字符或超限返回错误，原会话保持不变。
    pub fn import_gateway_cookies(&mut self, cookie_header: Secret) -> Result<(), AuthHttpError> {
        if cookie_header.len() > MAX_COOKIE_BYTES {
            return Err(AuthHttpError::SizeLimit);
        }
        let mut cookies = BTreeMap::new();
        for pair in cookie_header.expose().split(';') {
            let (name, value) = cookie_pair(pair.trim())?;
            cookies.insert(name.into(), Secret::new(value));
        }
        self.cookies = cookies;
        self.csrf = None;
        drop(cookie_header);
        Ok(())
    }

    #[must_use]
    pub fn sid(&self) -> Option<&Secret> {
        self.cookies.get("sid")
    }

    /// 导出网关会话 cookie header（`sid=...; sid.sig=...`）。
    ///
    /// 网关的 `sessionIdExchange` 以 Set-Cookie 交付会话（body 只含 `onlineInfo`），
    /// `SessionExchange` 的 body 解析结果可能为空。此方法从 cookie jar 构造 header，
    /// 仅当 `sid` 在位时返回 `Some`，保证调用方不会把空会话当成功。
    #[must_use]
    pub fn gateway_session_cookie_header(&self) -> Option<String> {
        self.cookies.get("sid").map(|_| {
            const SESSION_COOKIES: [&str; 6] = [
                "sid", "sid.sig", "sid-legacy", "sid-legacy.sig", "straceid", "straceid.sig",
            ];
            SESSION_COOKIES
                .iter()
                .filter_map(|&name| self.cookies.get(name).map(|value| format!("{name}={}", value.expose())))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }

    // 交接只消费本次兑换的原响应，不发查询。JSON 显式字段优先于同次 Set-Cookie；
    // 缺省字段沿用 Cookie，CSRF 与签名材料不受影响。先检查原 Cookie 表示约束再移动。
    fn admit_session_exchange(&mut self, exchange: SessionExchange) -> Result<(), AuthHttpError> {
        let fields = [
            ("sid", exchange.sid),
            ("sid.sig", exchange.sid_sig),
            ("sid-legacy", exchange.sid_legacy),
            ("sid-legacy.sig", exchange.sid_legacy_sig),
            ("straceid", exchange.straceid),
            ("straceid.sig", exchange.straceid_sig),
        ];
        for (name, value) in &fields {
            if let Some(value) = value {
                validate_cookie_parts(name, value.expose())?;
            }
        }
        let retained_bytes: usize = self
            .cookies
            .iter()
            .filter(|(name, _)| {
                !fields
                    .iter()
                    .any(|(key, value)| *key == name.as_str() && value.is_some())
            })
            .map(|(name, value)| name.len() + value.len() + 2)
            .sum();
        let incoming_bytes: usize = fields
            .iter()
            .filter_map(|(name, value)| value.as_ref().map(|value| name.len() + value.len() + 2))
            .sum();
        if retained_bytes + incoming_bytes > MAX_COOKIE_BYTES {
            return Err(AuthHttpError::SizeLimit);
        }
        for (name, value) in fields {
            if let Some(value) = value {
                self.cookies.insert(name.into(), value);
            }
        }
        Ok(())
    }

    /// 查询会话状态并更新 CSRF。
    /// # Errors
    /// 返回有界传输、HTTP 状态、协议形状或业务错误。
    pub async fn auth_config(&mut self, need_ticket: bool) -> Result<AuthStatus, AuthHttpError> {
        Ok(self.auth_config_detail(need_ticket).await?.status)
    }

    /// 同 [`Self::auth_config`]，但完整返回设备握手材料（`antiMITMAttackData`、
    /// `security.csrfToken`、`guid`、`unitid`），并在材料齐全时缓存派生的 signKey 供
    /// [`Self::endpoint_strategy`]/[`Self::report_env`] 签名。既有消费方继续使用
    /// [`Self::auth_config`]，行为不变。
    /// # Errors
    /// 同 [`Self::auth_config`]。
    pub async fn auth_config_detail(
        &mut self,
        need_ticket: bool,
    ) -> Result<AuthConfigDetail, AuthHttpError> {
        let path = self.options.identity.auth_config_path(need_ticket);
        let raw = self
            .request(
                Method::GET,
                &path,
                Zeroizing::new(Vec::new()),
                ConnectPurpose::AuthGateway,
                false,
                RequestSigning::None,
            )
            .await?;
        let detail = parse_auth_config_detail(&raw)?;
        self.csrf = detail.csrf.as_ref().map(|csrf| Secret::new(csrf.expose()));
        if let Some(sign_key) = detail
            .anti_mitm
            .as_ref()
            .and_then(signing::derive_from_anti_mitm)
        {
            self.sign_key = Some(Zeroizing::new(sign_key));
        }
        Ok(detail)
    }

    // 兑换后只获取资源消费所需材料；不读取登录状态或要求 SID/设备材料。
    // Cookie 由原 request 接纳，CSRF 按本响应替换，缺失留给资源消费点报告。
    async fn prepare_resource_materials(&mut self) -> Result<(), AuthHttpError> {
        let path = self.options.identity.auth_config_path(false);
        let raw = self
            .request(
                Method::GET,
                &path,
                Zeroizing::new(Vec::new()),
                ConnectPurpose::AuthGateway,
                false,
                RequestSigning::None,
            )
            .await?;
        self.csrf = parse_resource_csrf(&raw)?;
        Ok(())
    }

    /// 注入由握手材料派生的 signKey（例如 S2 从 `/v1/service/checkMITMAttack` 请求体
    /// 派生后复用同一客户端）；与 [`Self::auth_config_detail`] 的自动派生互为替代，
    /// 后写入者生效。
    pub fn install_sign_key(&mut self, sign_key: [u8; 32]) {
        self.sign_key = Some(Zeroizing::new(sign_key));
    }

    /// 签名密钥是否就绪（决定签名端点可否调用）。
    #[must_use]
    pub fn has_sign_key(&self) -> bool {
        self.sign_key.is_some()
    }

    /// 将一次性票据兑换为当前网关会话。
    ///
    /// 返回 [`SessionExchange`]：`sessionIdExchange` 以 JSON 体落回消费方的会话字段
    /// （规格 2.5，`sid`/`sid.sig` 等；官方浏览器 cookie jar 从无 `sid`）。S2 仿真 agent
    /// 据此在自身侧持有会话并可选注入 WebView。字段集合以 S6 真实网关验收为准，未知键
    /// 忽略、已知键可缺省。
    /// # Errors
    /// 缺少 CSRF 或网关拒绝、传输失败时返回错误。
    pub async fn exchange_sid_ticket(
        &mut self,
        ticket: Secret,
    ) -> Result<SessionExchange, AuthHttpError> {
        #[derive(serde::Serialize)]
        struct Ticket<'a> {
            #[serde(rename = "sidTicket")]
            sid_ticket: &'a str,
        }
        let body = Zeroizing::new(
            serde_json::to_vec(&Ticket {
                sid_ticket: ticket.expose(),
            })
            .map_err(|_| AuthHttpError::Protocol)?,
        );
        let path = self.options.identity.session_exchange_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                body,
                ConnectPurpose::AuthGateway,
                true,
                RequestSigning::None,
            )
            .await?;
        parse_session_exchange(&raw)
    }

    /// 已登录源会话生成票据，供受信任宿主立即交给另一个网关会话。
    /// # Errors
    /// 缺少 CSRF 或网关拒绝、传输失败时返回错误。
    #[allow(clippy::items_after_statements)]
    pub async fn ticket_exchange(&mut self) -> Result<Secret, AuthHttpError> {
        let path = self.options.identity.ticket_exchange_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                Zeroizing::new(b"{}".to_vec()),
                ConnectPurpose::AuthGateway,
                true,
                RequestSigning::None,
            )
            .await?;
        #[derive(Deserialize)]
        struct Ticket {
            #[serde(rename = "sidTicket")]
            sid_ticket: String,
        }
        impl Drop for Ticket {
            fn drop(&mut self) {
                self.sid_ticket.zeroize();
            }
        }
        let ticket: Ticket = decode(&raw)?;
        Ok(Secret::new(&ticket.sid_ticket))
    }

    /// 返回网关完整 JSON 正文，供资源解析器处理；调用方负责禁止日志输出。
    /// 请求必须包含 appList；同时校验顶层与 appList 业务码。
    /// # Errors
    /// 传输失败、顶层或 appList 拒绝、响应缺少 appList 时返回错误。
    pub async fn client_resource(
        &mut self,
        request_json: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AuthHttpError> {
        let path = self.options.identity.resource_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                Zeroizing::new(request_json.to_vec()),
                ConnectPurpose::ResourceGateway,
                true,
                RequestSigning::None,
            )
            .await?;
        validate_resource(&raw)?;
        Ok(raw)
    }

    /// 拉取环境检查策略（官方成功序列第 4 步，规格 §一/2.4）。
    ///
    /// **GET**，查询参数四元组 `timing`/`ticket`/`platform`/`clientType` 缺 `platform`
    /// 或 `clientType` 会被网关以 404 拒绝（路由不匹配，签名尚未参与）。`ticket` 恒为
    /// 本次 CAS 票据。携带环境上报族签名头（`X-Request-Sig`/`x-sdp-signature`/
    /// `x-sdp-random`）；空正文按 uri-only 签名（[`RequestSigning::EnvReport`] 语义不变）。
    /// 返回网关完整 JSON 正文（含 `data.env` 策略项与 `data.interval`），供 S2 驱动本地
    /// 环境检查；调用方负责禁止日志输出。
    ///
    /// 方法勘误（S7 阶段 A，2026-10-07 消融）：该路由是 **GET**——POST 得 404 HTML 壳，
    /// GET 才回 200 JSON。S1 起的 POST 使 S2 仿真的核销从未成功过（网关从未见过策略
    /// 请求的真实形状）；本修正让仿真核销真正可用，签名语义（uri-only、空正文）不变。
    /// # Errors
    /// 缺少 signKey、传输失败、网关拒绝或业务码非 0 时返回错误。
    pub async fn endpoint_strategy(
        &mut self,
        ticket: &Secret,
        timing: &str,
    ) -> Result<Zeroizing<Vec<u8>>, AuthHttpError> {
        let path = self
            .options
            .identity
            .endpoint_strategy_path(timing, ticket.expose());
        let raw = self
            .request(
                Method::GET,
                &path,
                Zeroizing::new(Vec::new()),
                ConnectPurpose::AuthGateway,
                false,
                RequestSigning::EnvReport,
            )
            .await?;
        decode::<serde::de::IgnoredAny>(&raw)?;
        Ok(raw)
    }

    /// 上报环境检查结果（官方成功序列第 5 步，规格 §一/2.3）。
    ///
    /// POST `/controller/v1/public/reportEnv?platform=&clientType=`，正文
    /// `{ticket, deviceId, env:{endpoint:<endpoint_env>}}`（R1 §5.2 实测形状；
    /// `endpoint_env` 由调用方按 endpointStrategy 下发的策略项组装）。携带环境上报族
    /// 签名头；无签名会被网关以 `10000008 interface sig verify failed` 拒绝。
    /// # Errors
    /// 缺少 signKey、传输失败、网关拒绝或业务码非 0 时返回错误。
    pub async fn report_env(
        &mut self,
        ticket: &Secret,
        device_id: &str,
        endpoint_env: &serde_json::Value,
    ) -> Result<(), AuthHttpError> {
        #[derive(serde::Serialize)]
        struct Env<'a> {
            endpoint: &'a serde_json::Value,
        }
        #[derive(serde::Serialize)]
        struct ReportEnv<'a> {
            ticket: &'a str,
            #[serde(rename = "deviceId")]
            device_id: &'a str,
            env: Env<'a>,
        }
        let body = Zeroizing::new(
            serde_json::to_vec(&ReportEnv {
                ticket: ticket.expose(),
                device_id,
                env: Env {
                    endpoint: endpoint_env,
                },
            })
            .map_err(|_| AuthHttpError::Protocol)?,
        );
        let path = self.options.identity.report_env_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                body,
                ConnectPurpose::AuthGateway,
                false,
                RequestSigning::EnvReport,
            )
            .await?;
        decode::<serde::de::IgnoredAny>(&raw)?;
        Ok(())
    }

    /// 认证状态检查（`nextService=auth/authCheck` 对应端点，R1 §5.1：官方默认 GET）。
    ///
    /// 该端点不在 `X-Request-Sig` 12-URI 白名单内、也不属环境上报族（规格 2.3），
    /// 因此不携带签名头。与 `authConfig` 不同，本端点在"未登录"时仍回 HTTP 200 加
    /// 非零业务码（实测 `75500001 当前认证已超时`），故不把非零码当传输错误，
    /// 由调用方按 [`AuthCheck`] 语义判断。
    /// # Errors
    /// 传输、HTTP 状态或响应不是携带 `code` 的 JSON 时返回错误。
    pub async fn auth_check(&mut self) -> Result<AuthCheck, AuthHttpError> {
        #[derive(Deserialize)]
        struct CodeOnly {
            code: i64,
        }
        let path = self.options.identity.auth_check_path();
        let raw = self
            .request(
                Method::GET,
                &path,
                Zeroizing::new(Vec::new()),
                ConnectPurpose::AuthGateway,
                false,
                RequestSigning::None,
            )
            .await?;
        let parsed: CodeOnly = serde_json::from_slice(&raw).map_err(|_| AuthHttpError::Protocol)?;
        Ok(AuthCheck { code: parsed.code })
    }

    async fn request(
        &mut self,
        method: Method,
        path: &str,
        body: Zeroizing<Vec<u8>>,
        purpose: ConnectPurpose,
        csrf_required: bool,
        signing: RequestSigning,
    ) -> Result<Zeroizing<Vec<u8>>, AuthHttpError> {
        if body.len() > self.options.max_response_bytes {
            return Err(AuthHttpError::SizeLimit);
        }
        // L1：`x-sdp-traceid` 每请求随机 8 字节十六进制。
        let trace_id = random_trace_id();
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", &self.authority)
            .header("user-agent", &self.options.user_agent)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            // L1 协议身份：base64(网关主机名) 与每请求 trace id。
            .header("x-sdp-rid", &self.sdp_rid)
            .header("x-sdp-traceid", &trace_id)
            // 门户 Referer：本客户端只访问该网关 origin，非网关/隧道请求不经此处，故统一设置。
            .header("referer", &self.referer);
        if signing == RequestSigning::EnvReport {
            // 环境上报族签名头（规格 2.3/A.4）：`X-Request-Sig` 与 `x-sdp-signature` 走同一
            // calRequestSign 构造（HMAC-SHA256(signKey, uri[||body])，无 nonce），两值相同；
            // 签名原文用与请求行完全一致的路径加查询串。请求体是 JSON（UTF-8），
            // 空正文按 uri-only 签（A.4 命中向量形态）。
            let sign_key = self
                .sign_key
                .as_deref()
                .ok_or(AuthHttpError::MissingSignKey)?;
            let body_text = if body.is_empty() {
                None
            } else {
                Some(std::str::from_utf8(&body).map_err(|_| AuthHttpError::Protocol)?)
            };
            let signature = signing::request_sig(sign_key, path, body_text);
            builder = builder
                .header("X-Request-Sig", sensitive_header(&signature)?)
                .header("x-sdp-signature", sensitive_header(&signature)?)
                // TODO(S6 消融实验 C 定稿): `x-sdp-random` 的请求侧构造规格未钉死——已证实的
                // 方向是网关→客户端（endpointStrategy/reportEnv 的响应必须带该头，客户端校验
                // 后推测回带/参与下一次签名）。此处按最可能形态（与 traceid 同分布的 16 位
                // 小写十六进制随机 nonce）每请求新生成；响应侧校验与回带语义留待 S2/S6 以
                // 真实网关钉死后再补，当前失败不改变请求行为。
                .header("x-sdp-random", sensitive_header(&random_trace_id())?);
        }
        if csrf_required {
            let csrf = self.csrf.as_ref().ok_or(AuthHttpError::MissingCsrf)?;
            builder = builder.header("x-csrf-token", sensitive_header(csrf.expose())?);
        }
        let mut cookie_header = Zeroizing::new(String::new());
        for (name, value) in &self.cookies {
            if !cookie_header.is_empty() {
                cookie_header.push_str("; ");
            }
            cookie_header.push_str(name);
            cookie_header.push('=');
            cookie_header.push_str(value.expose());
        }
        if !cookie_header.is_empty() {
            builder = builder.header("cookie", sensitive_header(&cookie_header)?);
        }
        // HTTP/TLS 实现会持有短期传输缓冲区；本层原始敏感材料和完整正文采用零化容器。
        let request = builder
            .body(Full::new(Bytes::copy_from_slice(&body)))
            .map_err(|_| AuthHttpError::Protocol)?;
        let result = tokio::time::timeout(self.options.timeout, async {
            let stream = self
                .underlay
                .connect(&OutboundTarget::new(&self.host, self.port, purpose))
                .await
                .map_err(AuthHttpError::Underlay)?;
            let config = client_config(&self.options)?;
            let server_name = rustls::pki_types::ServerName::try_from(self.host.clone())
                .map_err(|_| AuthHttpError::InvalidConfiguration)?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(server_name, stream)
                .await
                .map_err(|error| AuthHttpError::tls_io("tls.connect", &error))?;
            send_http(tls, request, self.options.max_response_bytes).await
        })
        .await
        .map_err(|_| AuthHttpError::Timeout)??;
        self.update_cookies(&result.0)?;
        Ok(result.1)
    }

    fn update_cookies(&mut self, headers: &HeaderMap) -> Result<(), AuthHttpError> {
        // 本流程使用同网关根路径会话 Cookie。跨域或非根路径 Cookie 不导入。
        // 当前不实现仅 Expires 的过期删除或浏览器完整 CookieJar 语义。
        for header in headers.get_all("set-cookie") {
            let value = header.to_str().map_err(|_| AuthHttpError::InvalidCookie)?;
            if value.len() > MAX_COOKIE_BYTES {
                return Err(AuthHttpError::SizeLimit);
            }
            let mut parts = value.split(';');
            let (name, value) = cookie_pair(parts.next().unwrap_or("").trim())?;
            let mut delete = value.is_empty();
            let mut applicable = true;
            for attribute in parts {
                let (key, val) = attribute
                    .trim()
                    .split_once('=')
                    .unwrap_or((attribute.trim(), ""));
                if key.eq_ignore_ascii_case("domain")
                    && !val.trim_start_matches('.').eq_ignore_ascii_case(&self.host)
                {
                    applicable = false;
                }
                if key.eq_ignore_ascii_case("path") && val != "/" {
                    applicable = false;
                }
                if key.eq_ignore_ascii_case("max-age")
                    && val.parse::<i64>().is_ok_and(|age| age <= 0)
                {
                    delete = true;
                }
            }
            if applicable {
                if delete {
                    self.cookies.remove(name);
                } else {
                    self.cookies.insert(name.into(), Secret::new(value));
                }
            }
            if self
                .cookies
                .iter()
                .map(|(k, v)| k.len() + v.len() + 2)
                .sum::<usize>()
                > MAX_COOKIE_BYTES
            {
                self.cookies.clear();
                return Err(AuthHttpError::SizeLimit);
            }
        }
        Ok(())
    }
}

// 独立材料形状避免 auth_config_detail 必需的 isLogin 变成兑换后复验。
fn parse_resource_csrf(raw: &[u8]) -> Result<Option<Secret>, AuthHttpError> {
    #[derive(Deserialize)]
    struct Security {
        #[serde(rename = "csrfToken")]
        csrf_token: Option<String>,
    }
    impl Drop for Security {
        fn drop(&mut self) {
            if let Some(value) = &mut self.csrf_token {
                value.zeroize();
            }
        }
    }
    #[derive(Deserialize)]
    struct Materials {
        security: Option<Security>,
    }
    let materials: Materials = decode(raw)?;
    Ok(materials
        .security
        .as_ref()
        .and_then(|security| security.csrf_token.as_ref())
        .map(|token| Secret::new(token.as_str())))
}

/// 解析 authConfig 响应为完整 [`AuthConfigDetail`]：会话状态、`security.csrfToken`、
/// `antiMITMAttackData`（可缺省）、`guid`/`unitid`。业务码非 0、JSON 形状不符时返回
/// 脱敏分类错误；提取为独立函数以便离线单测。
fn parse_auth_config_detail(raw: &[u8]) -> Result<AuthConfigDetail, AuthHttpError> {
    #[derive(Deserialize)]
    struct Security {
        #[serde(rename = "csrfToken")]
        csrf_token: Option<String>,
    }
    impl Drop for Security {
        fn drop(&mut self) {
            if let Some(value) = &mut self.csrf_token {
                value.zeroize();
            }
        }
    }
    #[derive(Deserialize)]
    struct Config {
        #[serde(rename = "isLogin")]
        is_login: u8,
        #[serde(rename = "firstAuth")]
        first_auth: Option<Vec<String>>,
        security: Option<Security>,
        #[serde(rename = "antiMITMAttackData")]
        anti_mitm: Option<AntiMitmAttackData>,
        guid: Option<String>,
        unitid: Option<String>,
    }
    let config: Config = decode(raw)?;
    let csrf = config
        .security
        .as_ref()
        .and_then(|s| s.csrf_token.as_ref())
        .map(|s| Secret::new(s.as_str()));
    Ok(AuthConfigDetail {
        status: AuthStatus {
            is_login: config.is_login == 1,
            first_auth: config.first_auth,
        },
        csrf,
        anti_mitm: config.anti_mitm,
        guid: config.guid,
        unitid: config.unitid,
    })
}

/// 生产构建唯一使用平台证书验证器；本 crate 单测（`cfg(test)`）与启用
/// `internal-test-trust` feature 的跨 crate 集成测试允许按实例注入显式根集合，
/// 用于本地合成 TLS 夹具。注入根仍执行完整链与主机名校验，不提供任何禁用校验的
/// 开关，也不使用全局或线程局部测试配置。
fn client_config(options: &AuthClientOptions) -> Result<rustls::ClientConfig, AuthHttpError> {
    // 常规构建里 `test_trust` 字段不存在；显式忽略该参数，避免“未使用”警告掩盖意图。
    #[cfg(not(any(test, feature = "internal-test-trust")))]
    let _ = options;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    #[cfg(any(test, feature = "internal-test-trust"))]
    if let Some(roots) = &options.test_trust {
        return Ok(rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| AuthHttpError::tls_config("tls.config", &error))?
            .with_root_certificates((**roots).clone())
            .with_no_client_auth());
    }
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|error| AuthHttpError::tls_config("tls.verifier", &error))?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| AuthHttpError::tls_config("tls.config", &error))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// 标准 base64（RFC 4648，含 `=` 填充）：`x-sdp-rid = base64(网关主机名)`。
fn base64_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map_or(0, u32::from);
        let third = chunk.get(2).copied().map_or(0, u32::from);
        let bits = (first << 16) | (second << 8) | third;
        encoded.push(char::from(ALPHABET[((bits >> 18) & 0x3f) as usize]));
        encoded.push(char::from(ALPHABET[((bits >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            encoded.push(char::from(ALPHABET[((bits >> 6) & 0x3f) as usize]));
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(char::from(ALPHABET[(bits & 0x3f) as usize]));
        } else {
            encoded.push('=');
        }
    }
    encoded
}

/// 生成官方形态的 `x-sdp-traceid`：8 字节随机数的 16 位小写十六进制。
/// 系统随机源不可用时回退到「纳秒时间戳 ^ 进程内递增计数」，仍保证每请求不同。
fn random_trace_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0u8; 8];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
            });
        bytes = (nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed)).to_le_bytes();
    }
    let mut trace_id = String::with_capacity(16);
    for byte in bytes {
        trace_id.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        trace_id.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    trace_id
}

fn sensitive_header(value: &str) -> Result<HeaderValue, AuthHttpError> {
    let mut header = HeaderValue::from_str(value).map_err(|_| AuthHttpError::Protocol)?;
    header.set_sensitive(true);
    Ok(header)
}
fn cookie_pair(value: &str) -> Result<(&str, &str), AuthHttpError> {
    let (name, value) = value.split_once('=').ok_or(AuthHttpError::InvalidCookie)?;
    validate_cookie_parts(name, value)?;
    Ok((name, value))
}
fn validate_cookie_parts(name: &str, value: &str) -> Result<(), AuthHttpError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !value.bytes().all(|b| {
            b == 0x21
                || (0x23..=0x2b).contains(&b)
                || (0x2d..=0x3a).contains(&b)
                || (0x3c..=0x5b).contains(&b)
                || (0x5d..=0x7e).contains(&b)
        })
    {
        return Err(AuthHttpError::InvalidCookie);
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<T, AuthHttpError> {
    #[derive(Deserialize)]
    struct Status {
        code: i64,
    }
    #[derive(Deserialize)]
    struct Envelope<T> {
        data: T,
    }
    let status: Status = serde_json::from_slice(raw).map_err(|_| AuthHttpError::Protocol)?;
    if status.code != 0 {
        return Err(AuthHttpError::Rejected { code: status.code });
    }
    let result: Envelope<T> = serde_json::from_slice(raw).map_err(|_| AuthHttpError::Protocol)?;
    Ok(result.data)
}

fn validate_resource(raw: &[u8]) -> Result<(), AuthHttpError> {
    #[derive(Deserialize)]
    struct AppList {
        code: i64,
    }
    #[derive(Deserialize)]
    struct Resources {
        #[serde(rename = "appList")]
        app_list: AppList,
    }
    let data: Resources = decode(raw)?;
    if data.app_list.code != 0 {
        return Err(AuthHttpError::Rejected {
            code: data.app_list.code,
        });
    }
    Ok(())
}

async fn send_http<S>(
    stream: S,
    request: Request<Full<Bytes>>,
    limit: usize,
) -> Result<(HeaderMap, Zeroizing<Vec<u8>>), AuthHttpError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(64 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|error| transport_error::http_failure("http.handshake", &error))?;
    // 不分离后台任务：超时/取消 drop 同时关闭 HTTP 驱动和注入流。
    let receive = async {
        let response = sender
            .send_request(request)
            .await
            .map_err(|error| transport_error::http_failure("http.request", &error))?;
        if !response.status().is_success() {
            return Err(AuthHttpError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        let (parts, mut body) = response.into_parts();
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(frame) = body.frame().await {
            let frame =
                frame.map_err(|error| transport_error::http_failure("http.body", &error))?;
            if let Some(data) = frame.data_ref() {
                if data.len() > limit.saturating_sub(bytes.len()) {
                    return Err(AuthHttpError::SizeLimit);
                }
                bytes.extend_from_slice(data);
            }
        }
        Ok((parts.headers, bytes))
    };
    tokio::pin!(connection);
    tokio::pin!(receive);
    tokio::select! { biased;
        result = &mut receive => result,
        result = &mut connection => { result.map_err(|error| transport_error::http_failure("http.driver", &error))?; receive.await }
    }
}

