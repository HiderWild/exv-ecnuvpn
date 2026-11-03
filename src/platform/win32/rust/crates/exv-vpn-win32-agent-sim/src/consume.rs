//! S2 真实核销：`/v1/service/reportEnvBeforeLogin` 与 `/v1/service/start` 的处理逻辑。
//!
//! 依据 `docs/research/atrust/2026-10-05-atrust-device-identity-and-signature-spec.md`
//! §一（官方成功序列）与《计划》§四 S2：本仿真 agent 不再吞掉门户 POST 来的 CAS 票据，
//! 而是按官方序列把它真正送到网关：
//!
//! ```text
//! reportEnvBeforeLogin（门户→agent，data:{ticket,timing,antiMITMAttackData}）
//!   → endpointStrategy（agent→网关，?timing=&ticket=&platform=Windows&clientType=SDPClient）
//!   → reportEnv       （agent→网关，票据核销点）
//! /v1/service/start（门户→agent，data:{sidTicket}）
//!   → sessionIdExchange（agent→网关）→ 会话字段（sid/sid.sig…）落本 sim 共享状态
//! ```
//!
//! 结构分层（可测性）：
//! * 信封解析（[`parse_pre_login`]/[`parse_start`]）与响应组装是纯函数；
//! * 网关 I/O 收敛在 [`GatewayConsumer`] trait 之后，测试注入假实现即可断言调用序列
//!   与失败上抛，无需真实网关；
//! * [`GatewayTicketConsumer`] 是真实实现：每次请求用桌面身份（`SDPClient`，与官方
//!   agent 相同）新建 `AuthClient`，signKey 优先从请求自带的 `antiMITMAttackData`
//!   派生（同会话材料，无额外往返），缺失时回退 `authConfig` 自动派生（S1）。
//!
//! 脱敏：票据/sidTicket/会话 Cookie 一律 [`Secret`] 承载，不进日志、不进错误消息；
//! 网关业务码与固定词汇的分类是协议层信息，可以出现在响应与日志里。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_atrust::auth::{
    AntiMitmAttackData, AuthClient, AuthClientOptions, AuthHttpError, SessionExchange, signing,
};
use exv_vpn_atrust::transport::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay, UnderlayStream,
};
use exv_vpn_atrust::{Secret, UnderlayError, UnderlayErrorKind};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroize;

use crate::contract;

/// `timing` 缺省值（官方常量）：成功序列 reportEnvBeforeLogin 的 `data.timing` 恒为
/// `pre-login`（规格 §一 第 3 步，L2 行 43）；endpointStrategy 路由按参数四元组匹配
/// （规格 2.4），空 `timing` 有 404 风险，缺省不得为空串（S2 评审修正 2）。
const DEFAULT_TIMING: &str = "pre-login";

/// 网关侧消费失败形态（脱敏：无票据、无 Cookie、无自由文本原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsumeError {
    /// 网关业务码非 0（如 `10000008` 签名校验失败）；码原样透传给门户。
    Gateway { code: i64 },
    /// 本地侧分类失败（信封不合规/配置/传输/超时/缺 signKey 等），无业务码。
    Local { class: &'static str },
}

/// `/v1/service/reportEnvBeforeLogin` 信封解析产物。
///
/// `gateway` 取自信封 `addr`（门户把网关 origin 原样告知本机 agent）；`ticket` 为
/// CAS 票据（敏感材料）；`guid` 是门户声明的设备标识，兼作 `reportEnv` 的
/// `deviceId`（选择理由：官方 agent 用自维护的 iddbase 设备 id，本仿真无该库，
/// 信封 `guid` 是同一门户会话已知的稳定设备标识，等价于浏览器路径自生成并持久化
/// `deviceId` 的地位）。
#[derive(Debug)]
pub struct PreLoginPlan {
    pub gateway: String,
    pub ticket: Secret,
    pub timing: String,
    pub guid: String,
    pub language: String,
    pub anti_mitm: Option<AntiMitmAttackData>,
}

/// `/v1/service/start` 信封解析产物（`data.sidTicket` 敏感材料）。
#[derive(Debug)]
pub struct StartRequest {
    pub gateway: String,
    /// 信封 `lang`（缺省/为空取 `zh-CN`），供 `sessionIdExchange` 路径透传（S2 评审修正 3）。
    pub language: String,
    pub sid_ticket: Secret,
}

/// 网关侧消费操作；真实实现 [`GatewayTicketConsumer`]，测试注入假实现。
///
/// 两个方法分别对应官方序列的两组网关调用；`consume_pre_login` 是票据核销点，
/// 一次门户请求恰好调用一次（内部不重试，杜绝单次票据双消费）。
pub trait GatewayConsumer: Send + Sync {
    /// 官方序列第 4-5 步：`endpointStrategy(timing,ticket)` → `reportEnv(ticket,deviceId,env)`。
    fn consume_pre_login<'a>(
        &'a self,
        plan: &'a PreLoginPlan,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConsumeError>> + Send + 'a>>;

    /// 官方序列第 7-8 步：`sessionIdExchange(sidTicket)` → 会话字段。`language` 取自
    /// `/v1/service/start` 信封 `lang`（缺省 `zh-CN`），透传到网关 query（S2 评审修正 3）。
    fn exchange_sid_ticket<'a>(
        &'a self,
        gateway: &'a str,
        language: &'a str,
        sid_ticket: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<SessionExchange, ConsumeError>> + Send + 'a>>;
}

/// `/v1/service/start` 成功后的会话落地快照（供 S3 可选注入 WebView cookie jar）。
pub struct SessionSnapshot {
    /// 会话所属网关 origin（注入时按域匹配）。
    pub gateway: String,
    /// `sid=…; sid.sig=…` 形态的 Cookie 请求头（`Secret` 承载，零化于 Drop）。
    pub cookie_header: Secret,
}

/// 会话共享状态：`/v1/service/start` 成功后保存，UI 层经 [`crate::server::AgentSim`]
/// 访问器读取。覆盖语义：新的成功交换整体替换旧会话；失败不改动既有状态。
#[derive(Default)]
pub struct SessionStore {
    inner: Mutex<Option<(String, Secret)>>,
}

impl SessionStore {
    /// 保存/覆盖会话（仅当 cookie 头非空；空会话字段不覆盖既有状态）。
    ///
    /// C4 起生产服务端是共享 `exv-vpn-atrust` 本地 Agent（票据消费经
    /// [`TicketConsumer`](exv_vpn_atrust::local_agent::TicketConsumer) 注入），
    /// 本方法公开给壳层消费者落地会话；语义与 agent-sim 服务端自用时一致。
    pub fn store(&self, gateway: &str, cookie_header: &str) {
        if cookie_header.is_empty() {
            return;
        }
        *self.inner.lock().expect("会话锁") =
            Some((gateway.to_owned(), Secret::new(cookie_header)));
    }

    /// 当前会话快照（无会话返回 `None`）。
    #[must_use]
    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        self.inner
            .lock()
            .expect("会话锁")
            .as_ref()
            .map(|(gateway, header)| SessionSnapshot {
                gateway: gateway.clone(),
                cookie_header: Secret::new(header.expose()),
            })
    }
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只打印是否已有会话；cookie 头是敏感材料。
        f.debug_struct("SessionStore")
            .field("has_session", &self.snapshot().is_some())
            .finish()
    }
}

/// 解析 `/v1/service/reportEnvBeforeLogin` 请求体（门户信封，utils.js `Pl` 构造）：
/// `{addr,type,guid,lang,token,data:{ticket,timing,antiMITMAttackData},sdpTraceId}`。
///
/// # Errors
/// 非法 JSON、缺 `addr`/`data.ticket`/非空 `guid` 或 `addr` 非 https 时返回
/// [`ConsumeError::Local`]（class=`envelope`），不触发任何网关调用——空 `guid` 会把
/// `deviceId=""` 送给 reportEnv（必然被网关拒绝、白白烧掉票据），故在解析期拒绝
/// （S2 评审修正 4）。
pub fn parse_pre_login(body: &[u8]) -> Result<PreLoginPlan, ConsumeError> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(default)]
        ticket: Option<String>,
        #[serde(default)]
        timing: Option<String>,
        #[serde(rename = "antiMITMAttackData", default)]
        anti_mitm: Option<AntiMitmAttackData>,
    }
    impl Drop for Data {
        fn drop(&mut self) {
            if let Some(ticket) = &mut self.ticket {
                ticket.zeroize();
            }
        }
    }
    #[derive(Deserialize)]
    struct Envelope {
        addr: Option<String>,
        #[serde(default)]
        guid: Option<String>,
        #[serde(default)]
        lang: Option<String>,
        data: Option<Data>,
    }
    let envelope: Envelope =
        serde_json::from_slice(body).map_err(|_| ConsumeError::Local { class: "envelope" })?;
    let mut data = envelope
        .data
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    let gateway = envelope
        .addr
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    if !gateway.starts_with("https://") || gateway.len() <= "https://".len() {
        return Err(ConsumeError::Local { class: "envelope" });
    }
    // guid 兼作 reportEnv 的 deviceId：缺失/为空都在解析期拒绝（见函数级注释）。
    let guid = envelope
        .guid
        .filter(|guid| !guid.is_empty())
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    let ticket = data
        .ticket
        .take()
        .filter(|ticket| !ticket.is_empty())
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    // `Data` 实现 Drop（票据零化），其余字段用 `take` 换出而非搬移。
    Ok(PreLoginPlan {
        gateway,
        ticket: Secret::new(ticket),
        // timing 缺省取官方常量（非空串），见 [`DEFAULT_TIMING`]。
        timing: data
            .timing
            .take()
            .unwrap_or_else(|| DEFAULT_TIMING.to_owned()),
        guid,
        language: envelope
            .lang
            .filter(|lang| !lang.is_empty())
            .unwrap_or_else(|| "zh-CN".to_owned()),
        anti_mitm: data.anti_mitm.take(),
    })
}

/// 解析 `/v1/service/start` 请求体：`{addr,…,data:{sidTicket}}`。
///
/// # Errors
/// 非法 JSON、缺 `addr`/`data.sidTicket` 时返回 [`ConsumeError::Local`]（class=`envelope`）。
pub fn parse_start(body: &[u8]) -> Result<StartRequest, ConsumeError> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "sidTicket")]
        sid_ticket: Option<String>,
    }
    impl Drop for Data {
        fn drop(&mut self) {
            if let Some(ticket) = &mut self.sid_ticket {
                ticket.zeroize();
            }
        }
    }
    #[derive(Deserialize)]
    struct Envelope {
        addr: Option<String>,
        #[serde(default)]
        lang: Option<String>,
        data: Option<Data>,
    }
    let envelope: Envelope =
        serde_json::from_slice(body).map_err(|_| ConsumeError::Local { class: "envelope" })?;
    let mut data = envelope
        .data
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    let gateway = envelope
        .addr
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    if !gateway.starts_with("https://") || gateway.len() <= "https://".len() {
        return Err(ConsumeError::Local { class: "envelope" });
    }
    let sid_ticket = data
        .sid_ticket
        .take()
        .filter(|ticket| !ticket.is_empty())
        .ok_or(ConsumeError::Local { class: "envelope" })?;
    Ok(StartRequest {
        gateway,
        // lang 缺省/为空取 zh-CN，与 reportEnvBeforeLogin 侧一致（S2 评审修正 3）。
        language: envelope
            .lang
            .filter(|lang| !lang.is_empty())
            .unwrap_or_else(|| "zh-CN".to_owned()),
        sid_ticket: Secret::new(sid_ticket),
    })
}

/// 处理 `/v1/service/reportEnvBeforeLogin`：成功（`code=0`）当且仅当
/// `endpointStrategy` 与 `reportEnv` 两次网关调用都真实成功；否则如实上抛
/// 网关业务码（或本地失败分类 + 官方 agent 内部错误码）。票据不进日志。
pub async fn handle_report_env_before_login(consumer: &dyn GatewayConsumer, body: &[u8]) -> Value {
    let plan = match parse_pre_login(body) {
        Ok(plan) => plan,
        Err(error) => {
            let response = consume_error_response(&error);
            tracing::warn!(
                target: "exv.agent_sim",
                code = %response["code"],
                class = ?error,
                "reportEnvBeforeLogin 信封解析失败（未触网）"
            );
            return response;
        }
    };
    // 2026-10-08 真机裁定：**不核销，立即诚实报错**（零网关调用）。
    // 证据链：门户 `source=auth_cas` 交接产生的事务绑定在浏览器上下文——仿真以
    // SDPClient 身份（签名 reportEnv）核销会被网关拒绝（75510008 族）且**烧掉
    // 单次票据**，从而毒化门户自有的浏览器路径回退（`Fi` catch → `Vi`），isLogin
    // 永不翻转（当日两轮真机冷链复现）；而仿真"早失败"（历史上的 strategy 404）
    // 恰使回退得以工作（S6 四连成 + Edge 机制抓包实证：页面 reportEnv
    // SDPBrowserClient 无签名核销成功）。此处把该成功配置从"偶然"固化为"设计"。
    tracing::info!(
        target: "exv.agent_sim",
        gateway = %plan.gateway,
        timing = %plan.timing,
        "reportEnvBeforeLogin 拒绝核销（浏览器事务归门户浏览器路径，零网关调用）"
    );
    let _ = (consumer, &plan);
    json!({"code": REFUSE_PRELOGIN_CODE, "message": "browser-bound transaction: consume via portal browser path"})
}

/// 处理 `/v1/service/start`：`sessionIdExchange` 成功后会话字段（`sid`/`sid.sig`…）
/// 落 [`SessionStore`]（S3 可选注入 WebView），失败如实上抛、不动既有会话。
pub async fn handle_service_start(
    consumer: &dyn GatewayConsumer,
    session: &SessionStore,
    body: &[u8],
) -> Value {
    let request = match parse_start(body) {
        Ok(request) => request,
        Err(error) => {
            let response = consume_error_response(&error);
            tracing::warn!(
                target: "exv.agent_sim",
                code = %response["code"],
                class = ?error,
                "service/start 信封解析失败（未触网）"
            );
            return response;
        }
    };
    match consumer
        .exchange_sid_ticket(&request.gateway, &request.language, &request.sid_ticket)
        .await
    {
        Ok(exchange) => {
            let header = exchange.cookie_header();
            if header.is_empty() {
                // 网关 code=0 但无会话字段：如实记录，不覆盖既有会话。
                tracing::warn!(
                    target: "exv.agent_sim",
                    gateway = %request.gateway,
                    "service/start 成功但网关未返回会话字段（S6 复核键集）"
                );
            } else {
                session.store(&request.gateway, header.as_str());
                tracing::info!(
                    target: "exv.agent_sim",
                    gateway = %request.gateway,
                    "service/start 会话已落地（cookie 不记录）"
                );
            }
            json!({"code": 0, "message": "OK", "data": {}})
        }
        Err(error) => {
            let response = consume_error_response(&error);
            tracing::warn!(
                target: "exv.agent_sim",
                gateway = %request.gateway,
                code = %response["code"],
                class = ?error,
                "service/start 会话交换失败"
            );
            response
        }
    }
}

/// 把 [`ConsumeError`] 组装为门户可消费的 agent 响应 JSON。
///
/// * 网关拒绝：透传网关业务码（如 `10000008`）；文案为固定描述——S1 的错误脱敏约定
///   不携带网关原文 message，码是诚实信号。
/// * 本地失败：官方 SFERRCODE `TRUSTCORE_BASIC_ERROR=11119000`（门户 utils.js 枚举，
///   映射文案 "Client inner request error"），文案只含固定分类词。
fn consume_error_response(error: &ConsumeError) -> Value {
    match error {
        ConsumeError::Gateway { code } => json!({
            "code": code,
            "message": format!("gateway rejected ticket consumption (code {code})"),
            "data": {},
        }),
        ConsumeError::Local { class } => json!({
            "code": contract::AGENT_INNER_ERROR_CODE,
            "message": format!("agent consume failed ({class})"),
            "data": {},
        }),
    }
}

/// 直连 TCP 出口：核销发生在认证早期，尚无任何 EXV 隧道/路由安装，系统默认路由即
/// 物理出口，普通 TCP 连接即可。与 `exv-vpn-win32-host` 探针的 `DirectUnderlay` 同一
/// 理由；本 crate 不依赖 host crate（依赖方向相反），本地持有等价实现。
pub struct DirectGatewayUnderlay;

impl Underlay for DirectGatewayUnderlay {
    fn resolve(&self, host: &str, port: u16) -> ResolveFuture<'_> {
        let endpoint = format!("{host}:{port}");
        Box::pin(async move {
            tokio::net::lookup_host(endpoint)
                .await
                .map(|addrs| addrs.map(|addr| addr.ip()).collect())
                .map_err(|_| {
                    UnderlayError::new(
                        UnderlayErrorKind::ResolveFailed,
                        ConnectPurpose::AuthGateway,
                    )
                })
        })
    }

    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        let host = target.host.clone();
        let port = target.port;
        let purpose = target.purpose;
        Box::pin(async move {
            tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map(|stream| Box::new(stream) as Box<dyn UnderlayStream>)
                .map_err(|_| UnderlayError::new(UnderlayErrorKind::ConnectFailed, purpose))
        })
    }
}

/// 真实网关消费者：桌面身份（`clientType=SDPClient`，与官方 agent 一致）。
///
/// signKey 获取顺序（计划 S2）：
///
/// 1. 优先从**请求自带**的 `antiMITMAttackData` 派生（`signing::derive_from_anti_mitm`
///    + `install_sign_key`），与官方 `checkMITMAttack` 同一材料源，无额外网关往返。
/// 2. 缺失（网关未对未知设备颁发设备数据，实验 B"空"分支）时回退
///    `auth_config_detail(false)` 自动拉取并派生（S1 内置缓存）。
/// 3. 仍无 signKey 时 `endpoint_strategy` 以 `MissingSignKey` 拒绝（拨号前失败，不烧票据）。
///
/// ECNU `enable=0` 时门户跳过 checkMITMAttack，握手材料仍随 reportEnvBeforeLogin 信封到达，
/// 故首选路径在无 checkMITMAttack 调用的场景下同样可用。
/// 真机裁定（2026-10-08）：`reportEnvBeforeLogin` 拒绝码——官方 `SFERRCODE.TRUSTCORE_BASIC_ERROR`
/// 同域固定值，告知门户此路径不可用、促其走浏览器路径回退核销。
pub use exv_vpn_atrust::local_agent::REFUSE_PRELOGIN_CODE;

pub struct GatewayTicketConsumer {
    underlay: Arc<dyn Underlay>,
    /// 测试信任根（仅 [`Self::with_underlay_and_test_trust`] 设置；生产构造恒 `None`，
    /// 且正式构建下注入点不存在，字段是惰性占位）。
    test_trust: Option<Arc<rustls::RootCertStore>>,
}

/// 真实网关消费者的每请求预算（S2 评审修正 5）：门户对 agent 请求整体预算 30 秒，
/// pre-login 缺 signKey 的回退最坏情况是 authConfig → endpointStrategy → reportEnv
/// 三次串行请求，8 秒/请求 ⇒ 最坏 ≈24 秒 < 30 秒（原 10 秒/请求恰好 30 秒吃满预算）。
/// 只收紧本消费者的预算，不改 `AuthClientOptions::new` 的全局默认（30 秒）。
const CONSUMER_REQUEST_BUDGET: Duration = Duration::from_secs(8);

impl GatewayTicketConsumer {
    /// 以直连出口构造（生产默认）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            underlay: Arc::new(DirectGatewayUnderlay),
            test_trust: None,
        }
    }

    /// 注入出口构造（宿主如需策略出口）。
    #[must_use]
    pub fn with_underlay(underlay: Arc<dyn Underlay>) -> Self {
        Self {
            underlay,
            test_trust: None,
        }
    }

    /// 仅测试（本 crate 单测/dev 依赖启用 `internal-test-trust` 时编译）：在注入出口的
    /// 基础上一并注入显式信任根，使真实 `AuthClient` 能信任回环假网关的自建 CA。
    /// 注入根仍走完整链与主机名校验；生产构建该构造不可达（`client_for` 内的注入点
    /// 在未启用 feature 时不存在）。
    #[cfg(any(test, feature = "internal-test-trust"))]
    #[must_use]
    pub fn with_underlay_and_test_trust(
        underlay: Arc<dyn Underlay>,
        test_trust: Arc<rustls::RootCertStore>,
    ) -> Self {
        Self {
            underlay,
            test_trust: Some(test_trust),
        }
    }

    /// 每次消费新建客户端：语言取信封（缺省 `zh-CN`），平台恒 `Windows`（本 crate 仅
    /// Win32）。预算见 [`CONSUMER_REQUEST_BUDGET`]。不用结构体更新语法改写选项——
    /// `test_trust` 字段跨 crate 不可见（按需经 [`Self::with_underlay_and_test_trust`]）。
    fn client_for(&self, gateway: &str, language: &str) -> Result<AuthClient, ConsumeError> {
        let identity = AuthClientIdentity::desktop("Windows", language).map_err(map_auth_error)?;
        let mut options = AuthClientOptions::new(identity);
        options.timeout = CONSUMER_REQUEST_BUDGET;
        if let Some(roots) = &self.test_trust {
            // 仅在 exv-vpn-atrust 启用 internal-test-trust（本 crate dev 依赖带入）时
            // 存在注入点；正式构建此分支编译为空操作。
            #[cfg(any(test, feature = "internal-test-trust"))]
            options.set_test_trust(Arc::clone(roots));
            #[cfg(not(any(test, feature = "internal-test-trust")))]
            let _ = roots;
        }
        AuthClient::new(gateway, Arc::clone(&self.underlay), options).map_err(map_auth_error)
    }
}

impl Default for GatewayTicketConsumer {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayConsumer for GatewayTicketConsumer {
    fn consume_pre_login<'a>(
        &'a self,
        plan: &'a PreLoginPlan,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConsumeError>> + Send + 'a>> {
        Box::pin(async move {
            let mut client = self.client_for(&plan.gateway, &plan.language)?;
            // signKey 首选：请求自带握手材料（同会话）。
            if let Some(key) = plan
                .anti_mitm
                .as_ref()
                .and_then(signing::derive_from_anti_mitm)
            {
                client.install_sign_key(key);
            }
            // 回退：authConfig 自动派生（拉一次配置；也顺带取得 CSRF 备用）。
            if !client.has_sign_key() {
                client
                    .auth_config_detail(false)
                    .await
                    .map_err(map_auth_error)?;
            }
            // 第 4 步：endpointStrategy（缺 signKey 在此之前即失败，不触网）。
            client
                .endpoint_strategy(&plan.ticket, &plan.timing)
                .await
                .map_err(map_auth_error)?;
            // 第 5 步：reportEnv（核销点）。env 载荷：官方 agent 按 endpointStrategy
            // 策略项执行本地检查后回填，本仿真无法复刻官方检查项，采用门户浏览器路径
            // 同等的最小诚实形状（R1 §5.2 SDPClient 样本）：{device_id, device:{type:"client"}}。
            let endpoint_env = json!({
                "device_id": plan.guid,
                "device": {"type": "client"},
            });
            client
                .report_env(&plan.ticket, &plan.guid, &endpoint_env)
                .await
                .map_err(map_auth_error)?;
            Ok(())
        })
    }

    fn exchange_sid_ticket<'a>(
        &'a self,
        gateway: &'a str,
        language: &'a str,
        sid_ticket: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<SessionExchange, ConsumeError>> + Send + 'a>> {
        Box::pin(async move {
            let mut client = self.client_for(gateway, language)?;
            // sessionIdExchange 需要 CSRF（S1 在拨号前强制）：先取 authConfig
            // （`needTicket=0`，只为 csrfToken；若网关顺带下发设备材料则缓存 signKey，无害）。
            client
                .auth_config_detail(false)
                .await
                .map_err(map_auth_error)?;
            // Secret 不 Clone：换一个等价副本交给客户端（两份都在 Drop 零化覆盖内）。
            let exchanged = client
                .exchange_sid_ticket(Secret::new(sid_ticket.expose()))
                .await
                .map_err(map_auth_error)?;
            if !exchanged.is_empty() {
                return Ok(exchanged);
            }
            // ECNU 实际网关 body 只有 onlineInfo，会话通过 Set-Cookie 交付。
            // 复用已修正的 Common 导出，不把 Secret 的脱敏 Display 当作线上值。
            let header = client
                .gateway_session_cookie_header()
                .ok_or(ConsumeError::Local {
                    class: "empty_session",
                })?;
            let header = Secret::new(header);
            Ok(session_from_gateway_cookie_header(header.expose()))
        })
    }
}

/// 将同网关 Cookie jar 导出的六项材料装入现有消费者契约；不改变正文交付路径。
fn session_from_gateway_cookie_header(header: &str) -> SessionExchange {
    let mut session = SessionExchange::default();
    for pair in header.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        let target = match name {
            "sid" => &mut session.sid,
            "sid.sig" => &mut session.sid_sig,
            "sid-legacy" => &mut session.sid_legacy,
            "sid-legacy.sig" => &mut session.sid_legacy_sig,
            "straceid" => &mut session.straceid,
            "straceid.sig" => &mut session.straceid_sig,
            _ => continue,
        };
        *target = Some(Secret::new(value));
    }
    session
}

/// S1 [`AuthHttpError`] → [`ConsumeError`]：拒绝码透传，其余映射为固定分类词
/// （与 host 探针 `ProbeError::from_auth` 的词汇一致，方便日志检索）。
fn map_auth_error(error: AuthHttpError) -> ConsumeError {
    match error {
        AuthHttpError::Rejected { code } => ConsumeError::Gateway { code },
        AuthHttpError::InvalidConfiguration => ConsumeError::Local { class: "config" },
        AuthHttpError::InvalidCookie => ConsumeError::Local { class: "cookie" },
        AuthHttpError::Timeout => ConsumeError::Local { class: "timeout" },
        AuthHttpError::Underlay(_) => ConsumeError::Local { class: "underlay" },
        AuthHttpError::Tls { .. } => ConsumeError::Local { class: "tls" },
        AuthHttpError::Http { .. } => ConsumeError::Local { class: "http" },
        AuthHttpError::HttpStatus { .. } => ConsumeError::Local {
            class: "http_status",
        },
        AuthHttpError::SizeLimit => ConsumeError::Local {
            class: "size_limit",
        },
        AuthHttpError::Protocol => ConsumeError::Local { class: "protocol" },
        AuthHttpError::MissingCsrf => ConsumeError::Local { class: "csrf" },
        AuthHttpError::MissingSignKey => ConsumeError::Local { class: "sign_key" },
    }
}
