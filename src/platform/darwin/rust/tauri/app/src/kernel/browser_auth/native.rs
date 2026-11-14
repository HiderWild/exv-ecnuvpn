//! 请求级原生系统浏览器认证；保留已原生验证的CAS直达入口。
use std::future::Future;
use std::sync::Arc;

use exv_vpn_darwin_core::local_agent_host::{ExchangeFailure, TicketDelivery};

use super::super::connection_request::AuthPreparationAction;
mod preparation;
pub(super) const USE_EMBEDDED: &str = "DARWIN_AUTH_USE_EMBEDDED";
pub(super) const TRY_SAVED_LOGIN: &str = "DARWIN_AUTH_TRY_SAVED_LOGIN";
use super::{AuthArtifact, AuthContext, AuthMilestone, AuthOptions, AuthSource, CANCELLED};
use tauri::Manager as _;

/// 等待票据超时的呈现文案：事实 + 操作（Chrome 本地网络权限首遇包含在内——
/// 未授权时浏览器零请求出浏览器，票据永不到达，治理合规：不推测、给可执行操作）。
pub(super) const NATIVE_TICKET_TIMEOUT: &str = "等待浏览器登录超时：EXV 未收到学校门户送来的登录票据。请确认已在浏览器中完成学校登录；若浏览器曾弹出是否允许访问本地网络的询问，请选择允许，然后重新连接。";

/// 编排入口：原生系统浏览器认证全程（不进入页面编排器——没有可控页面）。
pub(super) async fn authenticate(
    app: &tauri::AppHandle,
    context: &AuthContext,
    request: &super::ConnectionRequest,
    options: &AuthOptions,
) -> Result<AuthArtifact, String> {
    let deadline = options
        .deadline
        .unwrap_or_else(|| tokio::time::Instant::now() + super::AUTH_BUDGET);
    let mut start_error = None;
    let mut session = loop {
        let prepared = preparation::prepare(app, request, deadline, start_error.take()).await?;
        match super::super::native_agent::prepare_system_auth(
            app,
            request.generation(),
            &context.gateway,
            &effective_user_agent(context),
            prepared,
        )
        .await
        {
            Ok(session) => break session,
            Err(error) => start_error = Some(error),
        }
    };
    let state = app.state::<super::super::client::CoreState>();
    let result = async {
        let state = app.state::<super::super::client::CoreState>();
        let mut actions = state.connection_requests.auth_preparation_actions(request.generation())?;
        actions.borrow_and_update();
        state.connection_requests.set_auth_preparation(request.generation(), super::super::connection_request::AuthPreparationPhase::Browser, false);
        request.begin_submit().map_err(|_| CANCELLED.to_string())?;
        if tokio::time::Instant::now() >= deadline { return Err(super::TIMEOUT.to_string()); }
        options.emit(AuthMilestone::Opening);
        let url = portal_login_url(context, request).await?;
        super::super::external_open::open_in_default_browser(app, &url).await
            .map_err(|error| format!("无法打开默认浏览器：{error}"))?;
        options.emit(AuthMilestone::SchoolLogin);
        let waiting = wait_for_ticket(session.wait.as_receiver(), deadline, request.cancelled(), &options);
        tokio::pin!(waiting);
        loop {
            tokio::select! { biased;
                result = &mut waiting => break result,
                changed = actions.changed() => {
                    if changed.is_err() { break Err(CANCELLED.into()); }
                    match *actions.borrow_and_update() {
                        Some(AuthPreparationAction::UseEmbedded) => break Err(USE_EMBEDDED.into()),
                        Some(AuthPreparationAction::TrySavedLogin) => break Err(TRY_SAVED_LOGIN.into()),
                        _ => {}
                    }
                }
            }
        }
    }.await;
    state.connection_requests.set_auth_preparation(
        request.generation(),
        super::super::connection_request::AuthPreparationPhase::Idle,
        false,
    );
    session
        .finish_result(
            result,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .await
}

/// 网关登录入口：casLogin 直达（E2E 实证修正——Agent 在场时门户首页走
/// check_environment 支路不显示登录表单；casLogin 直达 CAS→网关落会话→
/// shortcut 链 authCheck 用票据成功→SPA 加载→_startClient→ticketExchange
/// 产生无绑定新票→service/start 直送本 Agent）。
async fn portal_login_url(
    context: &AuthContext,
    _request: &super::ConnectionRequest,
) -> Result<String, String> {
    Ok(format!(
        "{}/passport/v1/public/casLogin?sfDomain={}",
        context.gateway,
        school_domain(context)
    ))
}

/// 学校统一认证域（aTrust 网关的 sfDomain 参数）。从网关 authConfig 的
/// `firstAuth` URL 可提取，此处用常量（ECNU 网关实证 sfDomain=ecnu.edu.cn）。
fn school_domain(_context: &AuthContext) -> String {
    "ecnu.edu.cn".to_owned()
}

/// 兑换 UA（§7.6 对齐配方的冻结侧）：连接冻结 UA 非空用冻结值；空则官方 aTrust UA。
pub(super) fn effective_user_agent(context: &AuthContext) -> String {
    if context.user_agent.trim().is_empty() {
        exv_vpn_atrust::auth::client_identity::ATRUST_OFFICIAL_USER_AGENT.to_owned()
    } else {
        context.user_agent.clone()
    }
}

/// 等待票据交付循环（单测直接驱动交付通道：成功/票据失效重试/超时/取消）。
async fn wait_for_ticket(
    rx: &mut tokio::sync::mpsc::Receiver<TicketDelivery>,
    deadline: tokio::time::Instant,
    cancel: impl Future<Output = ()> + Send,
    options: &AuthOptions,
) -> Result<AuthArtifact, String> {
    tokio::pin!(cancel);
    loop {
        tokio::select! { biased;
            _ = &mut cancel => return Err(CANCELLED.into()),
            _ = tokio::time::sleep_until(deadline) => return Err(NATIVE_TICKET_TIMEOUT.to_owned()),
            delivery = rx.recv() => match delivery {
                Some(TicketDelivery::AuthenticatedSession { session, username }) => {
                    options.emit(AuthMilestone::GatewayConfirming);
                    let mut artifact = AuthArtifact::new(session, AuthSource::SystemBrowser);
                    artifact.authenticated_username = username;
                    artifact.interactive = true;
                    return Ok(artifact);
                }
                Some(TicketDelivery::Session(session)) => {
                    options.emit(AuthMilestone::GatewayConfirming);
                    let mut artifact = AuthArtifact::new(session, AuthSource::SystemBrowser);
                    artifact.interactive = true;
                    return Ok(artifact);
                }
                // 75500304：票据已失效（无重取源）。如实提示；等待继续——浏览器内
                // 重新登录产生新票后自动再兑换。
                Some(TicketDelivery::Failed(ExchangeFailure::TicketInvalid)) => {
                    options.emit(AuthMilestone::TicketInvalid);
                    continue;
                }
                Some(TicketDelivery::Failed(other)) => {
                    eprintln!("aTrust 原生票据兑换失败（等待继续）：{other:?}");
                    continue;
                }
                None => return Err("本地登录票据通道已关闭，请重新连接。".into()),
            },
        }
    }
}
