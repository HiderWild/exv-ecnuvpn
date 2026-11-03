//! WKWebView 原语；手动登录窗用网关级持久 data store（S4b，macOS>=14），不另设认证循环或提交预算。
use super::native_eval::{current_url, evaluate_json};
use super::page_facts::{
    PAGE_STARTUP_SCRIPT, PageObservation, SessionStatus, page_sample_script, parse_native_sample,
    parse_startup_sample,
};
use super::school_auto_login::{CredentialDelivery, InjectionOutcome, build_fill_script};
use super::*;
use exv_vpn_atrust::auth::browser_session::{CookieSessionError, session_from_cookie_pairs};
use exv_vpn_darwin_core::atrust_cas_facts::{apply_set_cookie_pair, cookie_header};
use exv_vpn_darwin_core::atrust_http_underlay::HostAuthUnderlay;
use exv_vpn_darwin_core::atrust_session_probe::{PROBE_TIMEOUT, probe_auth_config};
use exv_vpn_darwin_core::sso_session_jar::SSO_HOST;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use zeroize::{Zeroize, Zeroizing};

pub(super) struct EmbeddedChannel {
    app: AppHandle,
    window: WebviewWindow,
    label: String,
    closed: bool,
    destroyed: Arc<AtomicBool>,
    destroyed_event: Arc<tokio::sync::Notify>,
    started: std::time::Instant,
    last_startup: Option<String>,
    startup_navigation: Arc<AtomicU64>,
    round: u64,
    generation: u64,
    intent: AuthIntent,
}
impl EmbeddedChannel {
    pub(super) fn open(
        app: &AppHandle,
        context: &AuthContext,
        presentation: PresentationIntent,
        owner: u64,
        round: u64,
        intent: AuthIntent,
    ) -> Result<Self, String> {
        let label = format!("atrust-auth-{}", random_label_suffix());
        let navigation_label = label.clone();
        let startup_navigation = Arc::new(AtomicU64::new(0));
        let navigation_events = startup_navigation.clone();
        let visible = presentation == PresentationIntent::Visible;
        let destroyed = Arc::new(AtomicBool::new(false));
        let destroyed_event = Arc::new(tokio::sync::Notify::new());
        let builder = WebviewWindowBuilder::new(
            app,
            label.clone(),
            WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .title("学校统一身份认证 — 请在学校页面完成登录")
        .inner_size(540., 700.)
        .min_inner_size(380., 480.)
        .resizable(true)
        .center()
        .visible(visible)
        .initialization_script(PAGE_STARTUP_SCRIPT)
        .initialization_script_for_all_frames(include_str!("owned_portal_guard.js"))
        .initialization_script(if intent.fresh() {
            super::username_memory_script(super::external::profile_root().ok().and_then(|root| super::remembered_username(&root, &context.gateway)).as_deref())
        } else { String::new() })
        .focused(visible)
        .on_new_window(|url, _| {
            if owned_navigation_allowed(&url) {
                tauri::webview::NewWindowResponse::Allow
            } else {
                tauri::webview::NewWindowResponse::Deny
            }
        })
        .on_navigation(owned_navigation_allowed)
        .on_page_load(move |_, payload| {
            // 仅约束新增启动诊断；同 URL 完整重载也会使在途采样失效。
            let navigation = navigation_events.fetch_add(1, Ordering::AcqRel) + 1;
            eprintln!("aTrust embedded.navigation channel=wkwebview owner={} window={} navigation={} event={:?}", owner, navigation_label, navigation, payload.event());
        })
        // 沿用 HTTP/CAS 已有的普通浏览器兼容身份；原生 WK 默认 UA 不含 Safari 版本，
        // 实际门户仍将其拒绝。探针和候选从 navigator.userAgent 读取同一身份。
        .user_agent(&exv_vpn_darwin_core::atrust_cas_login::CasLoginOptions::default().user_agent);
        // 手动窗持久 store（S4b）：macOS>=14 传网关级 data_store_identifier，
        // 由 WKWebsiteDataStore(forIdentifier:) 与 Safari 默认库隔离并跨次登录保留；
        // macOS 13 及以下或版本未知时降级原 incognito——wry 0.54.4 在 macOS<14
        // 收到 identifier 会静默落入与 Safari 共享的 defaultDataStore，比 incognito 更差。
        let data_store_identifier = intent
            .fresh()
            .then(|| manual_data_store_identifier(&context.gateway, macos_major_version()))
            .flatten();
        let builder = match data_store_identifier {
            Some(identifier) => {
                eprintln!(
                    "aTrust embedded.data_store channel=wkwebview store=persistent key={}",
                    &gateway_hash(&context.gateway)[..16]
                );
                builder.incognito(false).data_store_identifier(identifier)
            }
            None => {
                if intent.fresh() {
                    eprintln!(
                        "aTrust embedded.data_store channel=wkwebview store=incognito reason=macos_below_14_or_unknown"
                    );
                }
                builder.incognito(intent.fresh())
            }
        };
        let window = builder
            .build()
            .map_err(|_| "无法打开内嵌登录窗口".to_owned())?;
        let closed = destroyed.clone();
        let closed_event = destroyed_event.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                closed.store(true, Ordering::Release);
                closed_event.notify_waiters();
            }
        });
        Ok(Self {
            app: app.clone(),
            window,
            label,
            closed: false,
            destroyed,
            destroyed_event,
            started: std::time::Instant::now(),
            last_startup: None,
            startup_navigation,
            round,
            generation: owner,
            intent,
        })
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.closed || self.app.get_webview_window(&self.label).is_none() {
            Err(CANCELLED.to_owned())
        } else {
            Ok(())
        }
    }

    fn close(&mut self) -> Result<(), String> {
        if !self.closed && !self.destroyed.load(Ordering::Acquire) {
            self.window
                .close()
                .map_err(|_| "无法关闭本轮内嵌认证窗口".to_owned())?;
            self.closed = true;
        }
        Ok(())
    }

    async fn user_agent(&self) -> Result<String, String> {
        self.ensure_open()?;
        let value = Zeroizing::new(
            evaluate_json(
                &self.window,
                Zeroizing::new("JSON.stringify(navigator.userAgent)".to_owned()),
            )
            .await?,
        );
        self.ensure_open()?;
        serde_json::from_str::<String>(&value)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "内嵌浏览器未返回有效标识".to_owned())
    }
}

/// 当前系统主版本（sysctl `kern.osproductversion`；`None` = 读不到，按低版本保守处理）。
fn macos_major_version() -> Option<u32> {
    crate::notification::os_major_version()
}

/// 手动内嵌窗持久 store 决策（S4b，纯函数）：仅 macOS>=14 返回网关级
/// identifier（sha256(gateway) 前 16 字节——与 `manual-persistent/<sha>` 哈希
/// 同源，同一网关跨次登录稳定复用同一 WKWebsiteDataStore）；13 及以下或版本
/// 未知返回 None（降级 incognito，禁止传 identifier——wry 0.54.4 在 macOS<14
/// 会静默落入与 Safari 共享的 defaultDataStore）。
fn manual_data_store_identifier(gateway: &str, macos_major: Option<u32>) -> Option<[u8; 16]> {
    if !macos_major.is_some_and(|version| version >= 14) {
        return None;
    }
    let digest = Sha256::digest(gateway.as_bytes());
    let mut identifier = [0_u8; 16];
    identifier.copy_from_slice(&digest[..16]);
    Some(identifier)
}
impl Drop for EmbeddedChannel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
impl ChannelTrait for EmbeddedChannel {
    fn closed(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let destroyed = self.destroyed.clone();
        let event = self.destroyed_event.clone();
        Box::pin(async move {
            loop {
                let notified = event.notified();
                if destroyed.load(Ordering::Acquire) {
                    return;
                }
                notified.await;
            }
        })
    }

    fn capabilities(&self) -> ChannelCapabilities {
        embedded_capabilities(self.intent)
    }
    fn supports_in_place_restart(&self) -> bool {
        // 窗口仍在（未被用户关闭/销毁）才允许原地重启；否则走重建。
        self.ensure_open().is_ok()
    }
    fn begin_round(&mut self, round: u64) {
        self.round = round;
    }
    fn first_navigation<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, FirstNavigation> {
        Box::pin(async move {
            self.ensure_open()?;
            // 预探针（S4b，对齐 win32 建窗前预探针；差异：本窗已按原时序建立，探针
            // 在首导航前执行，空窗期由探针时延构成并受总期限约束）。空 cookie 直连
            // 冻结网关；明确未登录且入口合法时直达学校登录页，跳过 authConfig
            // JSON 闪现，其余情形（失败/已登录/入口非法）保持 authConfig 原语义。
            let started = std::time::Instant::now();
            let probe = probe_auth_config(
                &context.gateway,
                "",
                &exv_vpn_darwin_core::atrust_cas_login::CasLoginOptions::default().user_agent,
                PROBE_TIMEOUT,
                Arc::new(HostAuthUnderlay),
            )
            .await;
            let direct = direct_login_target(false, &probe, &context.gateway);
            eprintln!(
                "aTrust embedded.pre_navigation_probe channel=wkwebview window={} round={} result={} direct={} elapsed_ms={}",
                self.label,
                self.round,
                match &probe {
                    Ok(state) if state.is_login => "logged_in",
                    Ok(_) => "not_logged_in",
                    Err(_) => "unavailable",
                },
                direct.is_some(),
                started.elapsed().as_millis()
            );
            Ok(match direct {
                Some(url) => FirstNavigation {
                    url,
                    school_login: true,
                },
                None => FirstNavigation {
                    url: format!("{}{}", context.gateway, context.auth_path),
                    school_login: false,
                },
            })
        })
    }
    fn navigate<'a>(&'a mut self, url: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            self.window
                .navigate(url.parse().map_err(|_| "学校认证入口无效".to_owned())?)
                .map_err(|_| "无法打开学校认证入口".to_owned())
        })
    }
    fn observe_page(&mut self, sequence: u64) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move {
            self.ensure_open()?;
            let before_navigation = self.startup_navigation.load(Ordering::Acquire);
            let before = current_url(&self.window).await?;
            let sample = evaluate_json(&self.window, Zeroizing::new(page_sample_script(sequence)))
                .await
                .ok()
                .map(Zeroizing::new);
            self.ensure_open()?;
            let after = current_url(&self.window).await?;
            let diagnostic = parse_startup_sample(
                sample.as_ref().map(|value| value.as_str()),
                sequence,
                before.as_ref().map(|value| value.as_str()),
                after.as_ref().map(|value| value.as_str()),
                (
                    before_navigation,
                    self.startup_navigation.load(Ordering::Acquire),
                ),
            );
            if sequence == 1 || sequence % 10 == 0 || self.last_startup != diagnostic {
                eprintln!(
                    "aTrust embedded.page_startup channel=wkwebview identity=browser_compatible window={} round={} navigation={} sample={} {} elapsed_ms={}",
                    self.label,
                    self.round,
                    before_navigation,
                    sequence,
                    diagnostic.as_deref().unwrap_or("result=unavailable"),
                    self.started.elapsed().as_millis()
                );
                crate::kernel::auth_diagnostics::startup(&self.app, self.generation, self.round, before_navigation, sequence, diagnostic.as_deref());
                self.last_startup = diagnostic.clone();
            }
            let startup = diagnostic
                .as_ref()
                .and_then(|_| sample.as_ref())
                .and_then(|sample| {
                    let sample: serde_json::Value = serde_json::from_str(sample).ok()?;
                    exv_vpn_atrust::auth::browser_auth_policy::StartupFacts::parse(
                        sample.get("startup")?.as_str()?,
                    )
                });
            let mut observation = parse_native_sample(
                sample.as_ref().map(|value| value.as_str()),
                sequence,
                before.as_ref().map(|value| value.as_str()),
                after.as_ref().map(|value| value.as_str()),
                (
                    before_navigation,
                    self.startup_navigation.load(Ordering::Acquire),
                ),
            );
            observation.startup = startup;
            Ok(observation)
        })
    }
    fn probe_session<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, SessionStatus> {
        Box::pin(async move {
            self.ensure_open()?;
            // 网关位置来自冻结上下文，学校页面 origin 不参与探针资格判定。
            let result = async {
                let cookies =
                    read_cookies(self.window.clone(), gateway_cookie_url(context)?).await?;
                let header = gateway_cookie_header(cookies);
                let user_agent = self.user_agent().await?;
                probe_auth_config(
                    &context.gateway,
                    &header,
                    &user_agent,
                    Duration::from_secs(6),
                    Arc::new(HostAuthUnderlay),
                )
                .await
                .map_err(|error| match error.http_status {
                    Some(status) => format!("固定网关会话探针失败 code={} http_status={status}", error.code),
                    None => format!("固定网关会话探针失败 code={}", error.code),
                })
            }
            .await;
            self.ensure_open()?;
            eprintln!(
                "aTrust embedded.host_probe channel=wkwebview window={} round={} result={} elapsed_ms={}",
                self.label,
                self.round,
                match &result {
                    Ok(probe) if probe.is_login => "logged_in",
                    Ok(_) => "logged_out",
                    Err(_) => "unavailable",
                },
                self.started.elapsed().as_millis()
            );
            result.map(|probe| {
                if probe.is_login {
                    SessionStatus::LoggedIn
                } else {
                    SessionStatus::LoggedOut(probe.first_auth)
                }
            })
        })
    }
    fn present(&mut self, intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async move {
            self.ensure_open()?;
            match intent {
                PresentationIntent::Hidden => {
                    self.window
                        .hide()
                        .map_err(|_| "无法隐藏学校认证窗口".to_owned())?;
                    Ok(PresentationOutcome::Hidden)
                }
                PresentationIntent::Visible => {
                    self.window
                        .show()
                        .map_err(|_| "无法显示学校认证窗口".to_owned())?;
                    // show 已成功，聚焦失败不改变可见事实，也不能因此关闭真实故障现场。
                    let _ = self.window.set_focus();
                    Ok(PresentationOutcome::Visible)
                }
            }
        })
    }
    fn inject_once(&mut self, delivery: CredentialDelivery) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async move {
            // 意图随窗口冻结；其他消费者也只在自动登录时取得填交能力。
            if !self.capabilities().can_inject {
                return Ok(InjectionOutcome::Failed);
            }
            self.ensure_open()?;
            let fill = build_fill_script(delivery)?;
            let mut script = Zeroizing::new(String::from("JSON.stringify("));
            script.push_str(&fill);
            script.push(')');
            let result = evaluate_json(&self.window, script).await;
            self.ensure_open()?;
            Ok(match result {
                Ok(value) => {
                    let value = Zeroizing::new(value);
                    InjectionOutcome::parse(&value)
                }
                Err(_) => InjectionOutcome::Failed,
            })
        })
    }
    fn observed_username(&mut self) -> ChannelFuture<'_, Option<String>> {
        Box::pin(async move {
            let cookies = read_cookies(self.window.clone(), sso_cookie_url()?).await?;
            Ok(cookies
                .iter()
                .find(|(key, _)| key == super::USERNAME_COOKIE)
                .and_then(|(_, value)| super::observed_username(value)))
        })
    }
    fn collect_sso(&mut self) -> ChannelFuture<'_, Vec<(String, String)>> {
        Box::pin(async move {
            self.ensure_open()?;
            let cookies = read_cookies(self.window.clone(), sso_cookie_url()?).await?;
            self.ensure_open()?;
            Ok(cookies)
        })
    }
    fn extract<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, Option<BrowserSession>> {
        Box::pin(async move {
            self.ensure_open()?;
            let user_agent = self.user_agent().await?;
            let cookies = read_cookies(self.window.clone(), gateway_cookie_url(context)?).await?;
            self.ensure_open()?;
            match session_from_cookie_pairs(&context.gateway, &user_agent, cookies) {
                Ok(session) => Ok(Some(session)),
                Err(CookieSessionError::MissingSession) => Ok(None),
                Err(_) => Err("内嵌浏览器会话字段无效".into()),
            }
        })
    }
    fn cleanup(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.close()?;
            // 新一轮只能在本轮原生窗口已释放后建立；外层沿用三秒收尾预算。
            self.closed().await;
            Ok(())
        })
    }
}

/// Cookie 按实际 authConfig 消费路径筛选，不按当前页面或网关根路径筛选。
fn gateway_cookie_url(context: &AuthContext) -> Result<tauri::Url, String> {
    format!("{}{}", context.gateway, context.auth_path)
        .parse()
        .map_err(|_| "冻结网关认证探针地址无效".to_owned())
}

fn sso_cookie_url() -> Result<tauri::Url, String> {
    format!("https://{SSO_HOST}/login")
        .parse()
        .map_err(|_| "学校 SSO 地址无效".to_owned())
}

/// 原生 API 包含 HttpOnly Cookie；仅供本通道所持窗口的数据域导出。
async fn read_cookies(
    window: WebviewWindow,
    url: tauri::Url,
) -> Result<Vec<(String, String)>, String> {
    tokio::task::spawn_blocking(move || {
        window
            // 本机 wry 的 cookies_for_url 使用精确域比较，会漏掉父域 Cookie。
            // 从原生数据域读取（含 HttpOnly），再按冻结 URL 的适用范围筛选。
            .cookies()
            .map(|cookies| {
                cookies
                    .into_iter()
                    .filter(|cookie| cookie_applies_to_url(cookie, &url))
                    .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
                    .collect()
            })
            .map_err(|_| "无法读取内嵌登录会话".to_owned())
    })
    .await
    .map_err(|_| "无法读取内嵌登录会话".to_owned())?
}

fn cookie_applies_to_url(cookie: &tauri::webview::Cookie<'_>, url: &tauri::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let Some(domain) = cookie.domain() else {
        // 全量原生导出需要域归属；没有来源域时不能把 Cookie 归给当前页面。
        return false;
    };
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    let domain_matches = host == domain
        || (host.parse::<std::net::IpAddr>().is_err()
            && host
                .strip_suffix(&domain)
                .is_some_and(|prefix| prefix.ends_with('.')));
    if domain.is_empty() || !domain_matches {
        return false;
    }
    if cookie.secure() == Some(true) && url.scheme() != "https" {
        return false;
    }
    let path = cookie.path().unwrap_or("/");
    let request_path = url.path();
    request_path == path
        || request_path
            .strip_prefix(path)
            .is_some_and(|suffix| path.ends_with('/') || suffix.starts_with('/'))
}

fn gateway_cookie_header(cookies: Vec<(String, String)>) -> Zeroizing<String> {
    let mut jar = BTreeMap::new();
    for (name, mut value) in cookies {
        let _ = apply_set_cookie_pair(&mut jar, &name, &value);
        value.zeroize();
    }
    cookie_header(&jar).unwrap_or_else(|| Zeroizing::new(String::new()))
}

fn embedded_capabilities(intent: AuthIntent) -> ChannelCapabilities {
    ChannelCapabilities {
        can_render: true,
        can_extract: true,
        can_inject: intent == AuthIntent::Automatic,
        surface: PresentationSurface::Controlled,
    }
}

fn random_label_suffix() -> String {
    let mut seed = [0_u8; 8];
    getrandom::fill(&mut seed).expect("system randomness is available");
    seed.iter().map(|byte| format!("{byte:02x}")).collect()
}

// 与doc-start请求拦截使用同一实证官方SDK范围，保留学校及其他本机应用导航。
fn owned_navigation_allowed(url: &tauri::Url) -> bool {
    let official = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost.sangfor.com.cn")
    ) && matches!(
        url.port(),
        Some(54630 | 54641 | 54652 | 54631 | 54642 | 54653)
    ) && matches!(
        url.path(),
        "/v1/detect"
            | "/v1/service/init"
            | "/v1/service/start"
            | "/v1/service/online"
            | "/v1/service/offline"
            | "/v1/service/logout"
            | "/v1/service/status"
            | "/v1/service/events"
            | "/v1/service/checkMITMAttack"
            | "/v1/service/reportEnv"
            | "/v1/service/reportEnvBeforeLogin"
            | "/v1/service/PopupPhishingRiskConfirm"
            | "/v1/update/check"
            | "/v1/update/status"
            | "/v1/update/start"
            | "/v1/base/setinfo"
            | "/v1/base/getter"
            | "/v1/base/setter"
            | "/v1/base/printLog"
    );
    url.scheme() == "https" && !official
}
