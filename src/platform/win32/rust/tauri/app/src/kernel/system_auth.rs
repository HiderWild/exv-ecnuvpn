//! 系统浏览器 UI 接线；阶段、计数与快照继续使用统一端口及既有事件。
//!
//! C4 原生通道：系统浏览器模式不再走 CDP 受控浏览器，改为「打开默认浏览器
//! （Windows `cmd /C start`，零控制）→ 本地 Agent 截获 `service/start` 票据 →
//! 立即兑换 → 会话快照建连」。CDP 阶梯（B1-B3 基础设施）保留在自动登录模式的
//! Chromium 选择上，不再从本入口进入。内嵌/自动登录/HTTP 探测各线保持不变。
use exv_core::atrust_browser::{BrowserAuthError, LoginAction, PLAIN_CHROME_USER_AGENT};
use exv_core::browser_auth::*;
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::sync::watch;

mod preparation;

struct Dependencies {
    app: AppHandle,
    generation: u64,
    user_agent: String,
    profile: std::path::PathBuf,
    intent: AuthIntent,
}
impl AuthEventSink for Dependencies {
    fn milestone(&self, milestone: AuthMilestone) {
        let stage = match milestone {
            AuthMilestone::Opening => "opening",
            AuthMilestone::SchoolLogin => "school_login",
            AuthMilestone::Submitting => "submitting",
            AuthMilestone::GatewayConfirming => "gateway_confirming",
            AuthMilestone::AwaitingManual => "awaiting_manual",
        };
        super::commands::emit_browser_login_stage(&self.app, self.generation, stage);
    }
    fn form_login_reached(&self) {
        super::embedded_auth::record_form_login_once(&self.app);
    }
}
impl SessionCacheInvalidator for Dependencies {
    fn forget<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if self.intent.allows_reuse() {
                exv_core::atrust_session_cache::forget(&self.profile, gateway);
            }
            Ok(())
        })
    }
}
impl SessionSnapshotProvider for Dependencies {
    fn snapshot<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, Option<SessionArtifact>> {
        Box::pin(async move {
            if !self.intent.allows_reuse() {
                return Ok(None);
            }
            let snapshot = super::local_agent::session_snapshot(&self.app);
            let session = snapshot.as_ref().and_then(|s| {
                super::embedded_auth::build_session_from_snapshot(
                    gateway,
                    &self.user_agent,
                    &s.gateway,
                    s.cookie_header.expose(),
                )
            });
            Ok(session.map(|session| SessionArtifact {
                session,
                source: SessionSource::AgentSnapshot,
            }))
        })
    }
}

pub(super) async fn acquire(
    app: &AppHandle,
    server: &str,
    profile: &Path,
    generation: u64,
    action: &mut watch::Receiver<LoginAction>,
    attempt: &AuthAttempt,
    intent: AuthIntent,
    interaction: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<exv_vpn_win32_ipc::browser_session::BrowserSession, BrowserAuthError> {
    let gateway = super::embedded_auth::normalize_gateway(server)?;
    let dependencies = Dependencies {
        app: app.clone(),
        generation,
        user_agent: PLAIN_CHROME_USER_AGENT.to_owned(),
        profile: profile.to_path_buf(),
        intent,
    };
    // CDP 入口在选择/启动浏览器后才装配 System 通道；这里薄装配同一编排器的 HTTP 段，
    // 让有效候选在选择浏览器前完成。用户明确切换渠道时沿原入口，不再重复 HTTP。
    if intent.allows_reuse() && *action.borrow() != LoginAction::UseSystemBrowser {
        let mut http = HttpProbeAdapter::new(profile.to_path_buf());
        let orchestrator = AuthOrchestrator {
            config: AuthConfig::new(
                gateway.clone(),
                PLAIN_CHROME_USER_AGENT.into(),
                exv_core::atrust_browser::auth_config_path(false),
            ),
            events: &dependencies,
            snapshots: &dependencies,
            credentials: None,
            cache: &dependencies,
            attempt,
        };
        let signal = |action| match action {
            LoginAction::Cancel => Some(AuthControl::Cancel),
            LoginAction::UseSystemBrowser => Some(AuthControl::UseSystemBrowser),
            _ => None,
        };
        let (sender, mut receiver) = watch::channel(signal(*action.borrow()));
        let mut channels: [&mut dyn ChannelTrait; 1] = [&mut http];
        let acquisition = orchestrator.acquire(&mut channels, &mut receiver);
        tokio::pin!(acquisition);
        let mut alive = true;
        let result = loop {
            tokio::select! { biased;
                changed = action.changed(), if alive => {
                    if changed.is_err() { alive = false; sender.send_replace(Some(AuthControl::Cancel)); }
                    else { sender.send_replace(signal(*action.borrow_and_update())); }
                }
                result = &mut acquisition => break result,
            }
        };
        match result {
            Ok(AcquisitionOutcome::Ready(artifact)) => return Ok(artifact.session),
            Ok(
                AcquisitionOutcome::Interactive(_)
                | AcquisitionOutcome::Control(AuthControl::UseSystemBrowser),
            ) => {}
            Ok(AcquisitionOutcome::Control(control)) => {
                return Err(BrowserAuthError::control(control, "浏览器登录已取消"));
            }
            Err(error) => {
                let message = error.legacy_message();
                return Err(BrowserAuthError::auth(error, message));
            }
        }
    }
    // C4 原生通道：本次请求本地 Agent（票据经 `/v1/service/start` 到达即兑换）+
    // 打开默认浏览器 + 等待兑换结果（会话快照建连）。
    let result = native_system_browser_acquire(app, &dependencies, &gateway, action, attempt).await;
    // 此入口已实际打开系统浏览器认证页；仅成功返回才影响成功收束点。
    if result.is_ok() {
        interaction.store(true, std::sync::atomic::Ordering::Release);
    }
    result
}

/// casLogin 兜底入口路径（ECNU 实证形状）；authConfig 探针取得 `firstAuth` 时以
/// 网关下发为准（`native_login_url`）。
const DEFAULT_CAS_LOGIN_PATH: &str = "/passport/v1/public/casLogin?sfDomain=ecnu.edu.cn";

/// 原生登录入口 URL：优先网关 `authConfig` 下发的 `firstAuth`（权威），探针失败/
/// 缺失时回退网关域派生（`vpn.ecnu.edu.cn` → `sfDomain=ecnu.edu.cn`；无可去标签
/// 时用 ECNU 常量）。
///
/// 纯函数（探针在 [`native_system_browser_acquire`] 内做）；与内嵌线共用
/// [`super::embedded_auth::login_url`] 的入口形状校验，不另立第二份实现。
fn native_login_url(gateway: &str, first_auth: Option<&str>) -> Result<String, String> {
    let path = match first_auth.filter(|auth| !auth.is_empty()) {
        Some(auth) => auth.to_owned(),
        None => {
            let host = gateway
                .trim_start_matches("https://")
                .split(['/', ':'])
                .next()
                .unwrap_or_default();
            match host.split_once('.') {
                Some((_, domain)) if !domain.is_empty() => {
                    format!("/passport/v1/public/casLogin?sfDomain={domain}")
                }
                _ => DEFAULT_CAS_LOGIN_PATH.to_owned(),
            }
        }
    };
    super::embedded_auth::login_url(gateway, &path)
}

/// 用系统默认浏览器打开认证页（Windows `cmd /C start`；零控制、零 CDP——与
/// `commands::open_external` 同一机制，此处独立于命令层供编排直接调用）。
fn open_default_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|child| {
            drop(child);
        })
}

/// C4 原生系统浏览器通道：本地 Agent 在线 → 打开默认浏览器 → 等待票据兑换。
///
/// 等待语义：门户对同一登录可能重试多张票据（R2 实证 5 次到达），单次失败不提前
/// 终止；期限到按最近一次失败如实分类（`75500304` → 「登录票据已失效」，见
/// `local_agent::TICKET_INVALID_CODE`）。浏览器页面在 EXV 竞速成功后的终态
/// （authCheck「操作异常」）只影响浏览器侧，不阻断本通道（知识库 §R2 第四轮）。
async fn native_system_browser_acquire(
    app: &AppHandle,
    dependencies: &Dependencies,
    gateway: &str,
    action: &mut watch::Receiver<LoginAction>,
    attempt: &AuthAttempt,
) -> Result<exv_vpn_win32_ipc::browser_session::BrowserSession, BrowserAuthError> {
    let mut start_error = None;
    let session = loop {
        let prepared = preparation::prepare(
            app,
            dependencies.generation,
            gateway,
            action,
            attempt,
            start_error.take(),
        )
        .await?;
        match super::local_agent::prepare_system_auth(
            app,
            dependencies.generation,
            gateway,
            prepared,
        )
        .await
        {
            Ok(session) => break session,
            Err(error) => start_error = Some(error),
        }
    };
    let requests = &app.state::<super::client::CoreState>().connection_requests;
    requests.set_auth_preparation(
        dependencies.generation,
        super::connection_request::AuthPreparationPhase::Browser,
        false,
    );
    let exchange = session.exchange.clone();
    let mut notify = exchange.subscribe();
    let result = async {
    // 2. 认证入口：一次 authConfig 探针取 firstAuth（探针失败不阻断——回退派生
    //    入口，入口问题在浏览器里如实呈现）。
    let first_auth = tokio::select! { biased;
        () = async {
            loop {
                if matches!(*action.borrow(), LoginAction::Cancel | LoginAction::Embedded) { break; }
                if action.changed().await.is_err() { break; }
            }
        } => {
            return if *action.borrow() == LoginAction::Embedded { Err(BrowserAuthError::FallBackEmbedded) }
                else { Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")) };
        }
        probe = tokio::time::timeout_at(
            attempt.deadline(Duration::from_secs(300)).min(tokio::time::Instant::now() + exv_core::atrust_session_probe::PROBE_TIMEOUT),
            exv_core::atrust_session_probe::probe_auth_config(gateway, "", PLAIN_CHROME_USER_AGENT),
        ) => match probe { Ok(Ok(probe)) => probe.first_auth, _ => None },
    };
    if *action.borrow() == LoginAction::Cancel { return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")); }
    if attempt.deadline(Duration::from_secs(300)) <= tokio::time::Instant::now() { return Err(BrowserAuthError::Host("准备系统浏览器登录超过本次认证期限".into())); }
    let url = native_login_url(gateway, first_auth.as_deref()).map_err(|_| {
        BrowserAuthError::auth(
            AuthError::at(AuthErrorCode::LoginEntryInvalid, AuthOperation::Open),
            "网关返回了无效的认证入口".into(),
        )
    })?;

    // 3. 打开默认浏览器（用户在浏览器完成 CAS 认证；EXV 零控制）。
    dependencies.milestone(AuthMilestone::Opening);
    if let Err(error) = open_default_browser(&url) {
        log::warn!(target: "exv.local_agent", "agent.browser_open_failed os_code={:?} error={error}", error.raw_os_error());
        return Err(BrowserAuthError::auth(
            AuthError::at(AuthErrorCode::ChannelOpenFailed, AuthOperation::Open),
            format!("无法打开系统默认浏览器：{error}"),
        ));
    }

    // 4. 等待票据兑换（同一预算口径：与既有系统浏览器线一致 300 秒上限）。
    //    watch 发送端被丢弃后 changed() 立即返回 Err——用存活标志停用对应 select
    //    臂，避免立即完成的分支把 select 退化成忙转。
    let deadline = attempt.deadline(Duration::from_secs(300));
    let mut notify_alive = true;
    loop {
        if exchange.exchanged_for_gateway() {
            return finish_native_login(&exchange, gateway);
        }
        tokio::select! { biased;
            () = tokio::time::sleep_until(deadline) => break,
            changed = action.changed() => {
                match changed {
                    Err(_) => return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")),
                    Ok(()) if *action.borrow_and_update() == LoginAction::Cancel => {
                        return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消"));
                    }
                    Ok(()) if *action.borrow() == LoginAction::Embedded => return Err(BrowserAuthError::FallBackEmbedded),
                    Ok(()) => {}
                }
                // UseSystemBrowser 在本通道是既成事实（已在系统浏览器），继续等待。
            }
            changed = notify.changed(), if notify_alive => {
                if changed.is_err() {
                    notify_alive = false;
                }
                // 一次兑换终结：成功在循环顶判读；失败记录留在 exchange 供期限分类。
            }
        }
    }
    // 期限到：终结可能与期限同时刻落地，先做最终判读，再按最近一次失败如实分类
    //（单次失败不提前终止——门户会重发新票据，R2 实证 5 次到达）。
    if exchange.exchanged_for_gateway() {
        return finish_native_login(&exchange, gateway);
    }
    if exchange.last_failure_code() == Some(super::local_agent::TICKET_INVALID_CODE) {
        return Err(BrowserAuthError::auth(
            AuthError {
                code: AuthErrorCode::FailedPage,
                operation: AuthOperation::Extract,
                native_code: Some(super::local_agent::TICKET_INVALID_CODE),
                server_detail: None,
                failure_kind: None,
            },
            "登录票据已失效，请重新在浏览器中登录".into(),
        ));
    }
    Err(BrowserAuthError::auth(
        AuthError::at(AuthErrorCode::AcquisitionTimeout, AuthOperation::Wait),
        "等待系统浏览器登录票据超过本次认证期限，请重试".into(),
    ))
    }.await;
    requests.set_auth_preparation(
        dependencies.generation,
        super::connection_request::AuthPreparationPhase::Idle,
        false,
    );
    finish_session(session, result).await
}

pub(super) async fn finish_session(
    session: super::local_agent::SystemAuthSession,
    result: Result<exv_vpn_win32_ipc::browser_session::BrowserSession, BrowserAuthError>,
) -> Result<exv_vpn_win32_ipc::browser_session::BrowserSession, BrowserAuthError> {
    let closed = session
        .finish(std::time::Instant::now() + Duration::from_secs(5))
        .await;
    match (result, closed) {
        (Ok(session), Ok(())) => Ok(session),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(BrowserAuthError::Host(format!(
            "结束本次本地登录服务失败：{error}"
        ))),
        (Err(original), Err(cleanup)) => {
            let append =
                |message: String| format!("{message}；结束本次本地登录服务失败：{cleanup}");
            Err(match original {
                BrowserAuthError::Auth {
                    error,
                    compatibility_message,
                } => BrowserAuthError::Auth {
                    error,
                    compatibility_message: append(compatibility_message),
                },
                BrowserAuthError::Control {
                    control,
                    compatibility_message,
                } => BrowserAuthError::Control {
                    control,
                    compatibility_message: append(compatibility_message),
                },
                BrowserAuthError::Host(message) => BrowserAuthError::Host(append(message)),
                BrowserAuthError::FallBackEmbedded => BrowserAuthError::Host(format!(
                    "切换内嵌浏览器前结束本次本地登录服务失败：{cleanup}"
                )),
            })
        }
    }
}

/// 本轮兑换成功的收束：会话快照 → 同源校验 + 既有 Cookie 解析构造会话。
/// 兑换成功但快照无法构造会话（不同源/材料不合法）时如实按无效会话失败。
fn finish_native_login(
    exchange: &super::local_agent::TicketExchange,
    gateway: &str,
) -> Result<exv_vpn_win32_ipc::browser_session::BrowserSession, BrowserAuthError> {
    let snapshot = exchange.snapshot();
    let session = snapshot.as_ref().and_then(|snapshot| {
        super::embedded_auth::build_session_from_snapshot(
            gateway,
            PLAIN_CHROME_USER_AGENT,
            &snapshot.gateway,
            snapshot.cookie_header.expose(),
        )
    });
    match session {
        Some(session) => {
            tracing::info!(
                target: "exv.local_agent",
                "原生系统浏览器登录完成（本地 Agent 票据兑换成功）"
            );
            Ok(session)
        }
        None => Err(BrowserAuthError::auth(
            AuthError::at(AuthErrorCode::SessionInvalid, AuthOperation::Extract),
            "登录会话无效，请重试".into(),
        )),
    }
}
