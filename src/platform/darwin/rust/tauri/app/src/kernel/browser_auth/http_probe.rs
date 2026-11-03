//! HTTP/CAS 仅复用学校 SSO；一次调用后由统一编排器决定交互回退。
//! 不读取网关候选缓存，不接收账号密码，也不向学校提交登录表单。
use super::*;
use exv_vpn_darwin_core::{
    atrust_auth_storage::{self as storage, SsoStorageError},
    atrust_cas_login::{
        CAS_REQUEST_TIMEOUT, CAS_TOTAL_BUDGET, CasLoginClient, CasLoginOptions, CasOutcome,
    },
    atrust_http_underlay::HostAuthUnderlay,
    sso_session_jar::{SSO_HOST, SsoSessionJar},
};
use std::{path::PathBuf, sync::Arc, time::Instant};

pub(super) struct HttpChannel {
    config_dir: PathBuf,
}
impl HttpChannel {
    pub(super) fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}
impl ChannelTrait for HttpChannel {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            can_render: false,
            can_extract: true,
            can_inject: false,
            surface: PresentationSurface::NoSurface,
        }
    }
    fn navigate<'a>(&'a mut self, _: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn observe_page(&mut self, sequence: u64) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move { Ok(page_facts::parse_sample(None, sequence, None, None)) })
    }
    fn probe_session<'a>(&'a mut self, _: &'a AuthContext) -> ChannelFuture<'a, SessionStatus> {
        Box::pin(async { Ok(SessionStatus::NotObserved) })
    }
    fn extract<'a>(&'a mut self, _: &'a AuthContext) -> ChannelFuture<'a, Option<BrowserSession>> {
        Box::pin(async { Ok(None) })
    }
    fn reuse_session<'a>(
        &'a mut self,
        context: &'a AuthContext,
        budget: Duration,
    ) -> ChannelFuture<'a, ReuseOutcome> {
        Box::pin(async move {
            let started = Instant::now();
            if budget.is_zero() {
                return Ok(interactive("budget_exhausted"));
            }
            let mut jar = match plan_jar(storage::load_sso_jar(&self.config_dir, SSO_HOST)) {
                Ok(jar) => jar,
                Err(reason) => return Ok(interactive(reason)),
            };
            let Some(options) = cas_options(budget.saturating_sub(started.elapsed())) else {
                return Ok(interactive("budget_exhausted"));
            };
            // CAS 使用普通 Chrome UA；候选保留完全相同的 UA，不沿用 Cisco 默认身份。
            let user_agent = options.user_agent.clone();
            let mut client =
                match CasLoginClient::new(&context.gateway, Arc::new(HostAuthUnderlay), options) {
                    Ok(client) => client,
                    Err(_) => return Ok(interactive("cas_configuration")),
                };
            let outcome = client.login(&mut jar).await;
            // CAS 轮换合并在同一 jar，保留原 deviceId；损坏档案从不走到此处。
            if storage::save_sso_jar(&self.config_dir, SSO_HOST, &jar).is_err() {
                eprintln!("aTrust HTTP复用 sso_save_failed");
            }
            Ok(map_outcome(outcome, &context.gateway, &user_agent))
        })
    }
    fn cleanup(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn cas_options(remaining: Duration) -> Option<CasLoginOptions> {
    if remaining.is_zero() {
        return None;
    }
    let budget = remaining.min(CAS_TOTAL_BUDGET);
    Some(CasLoginOptions {
        budget,
        timeout: budget.min(CAS_REQUEST_TIMEOUT),
        ..CasLoginOptions::default()
    })
}

fn plan_jar(
    loaded: Result<Option<SsoSessionJar>, SsoStorageError>,
) -> Result<SsoSessionJar, &'static str> {
    match loaded {
        Ok(Some(jar)) if jar.has_cookie("SESSION") || jar.has_cookie("SOURCEID_TGC") => Ok(jar),
        Ok(Some(_)) => Err("sso_insufficient"),
        Ok(None) => Err("sso_missing"),
        Err(_) => Err("sso_unreadable"),
    }
}

fn interactive(reason: &'static str) -> ReuseOutcome {
    eprintln!("aTrust HTTP复用 interactive reason={reason}");
    ReuseOutcome::Interactive(reason)
}

fn map_outcome(outcome: CasOutcome, gateway: &str, user_agent: &str) -> ReuseOutcome {
    match outcome {
        CasOutcome::Ticket {
            sid_via_auth_check: Some(session),
            ..
        } if !session.is_empty() => {
            let header = session.cookie_header();
            match BrowserSession::new(
                gateway.to_owned(),
                header.to_string(),
                user_agent.to_owned(),
            ) {
                Ok(candidate) => ReuseOutcome::Ready(candidate),
                Err(_) => interactive("cas_session_invalid"),
            }
        }
        CasOutcome::Ticket { .. } => interactive("cas_session_missing"),
        CasOutcome::FormRequired => interactive("cas_form_required"),
        CasOutcome::NetError(_) => interactive("cas_unavailable"),
    }
}
