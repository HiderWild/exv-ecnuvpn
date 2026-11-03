//! X4 HTTP 有限原语：缓存复核、单轮既有 CAS 协议复用及会话收尾。
//! 无页面、无学校凭据；静默回退和认证期限由统一 AuthOrchestrator 管理。
use super::*;
use crate::atrust_cas_login::{
    CAS_REQUEST_TIMEOUT, CAS_TOTAL_BUDGET, CasLoginClient, CasLoginOptions, CasOutcome,
};
use crate::atrust_session_probe::{DirectUnderlay, PROBE_TIMEOUT, ProbeError, SessionProbe};
use crate::sso_session_jar::SsoSessionJar;
use exv_vpn_atrust::auth::browser_session::{CookieSessionError, session_from_cookie_pairs};
use std::{path::PathBuf, sync::Arc};
use tokio::time::Instant;

const HTTP_BUDGET: Duration = Duration::from_secs(10);

/// 仅持有当前网关的缓存和 HTTP 运行材料，不接 CredentialProvider 或学校账号。
pub struct HttpProbeAdapter {
    profile: PathBuf,
    gateway: String,
    cached: Option<BrowserSession>,
    cache_error: Option<AuthError>,
    deadline: Option<Instant>,
    completed: bool,
}

impl HttpProbeAdapter {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            gateway: String::new(),
            cached: None,
            cache_error: None,
            deadline: None,
            completed: false,
        }
    }
    fn remaining(&self) -> Duration {
        self.deadline.map_or(Duration::ZERO, |d| {
            d.saturating_duration_since(Instant::now())
        })
    }
}

pub(crate) fn probe_status(probe: Result<SessionProbe, ProbeError>) -> Result<SessionStatus, AuthError> {
    match probe {
        Ok(state) if state.code_ok && state.is_login => Ok(SessionStatus::LoggedIn),
        Ok(state) if state.code_ok => Ok(SessionStatus::LoggedOut {
            first_auth: state.first_auth,
        }),
        Ok(_) => Ok(SessionStatus::NotYetObserved),
        Err(error) => {
            log::info!(target: "exv.atrust_auth", "http.cache_probe result=error reason={}", error.label());
            let mut failure =
                AuthError::at(AuthErrorCode::SessionProbeFailed, AuthOperation::Probe);
            failure.native_code = error.http_status.map(i64::from);
            Err(failure)
        }
    }
}

enum JarPlan {
    Run(SsoSessionJar),
    Interactive(InteractiveReason),
}
fn plan_jar_run(jar: Result<Option<SsoSessionJar>, String>) -> Result<JarPlan, AuthError> {
    match jar {
        Ok(Some(jar)) if jar.has_cookie("SESSION") && jar.has_cookie("SOURCEID_TGC") => {
            Ok(JarPlan::Run(jar))
        }
        Ok(Some(_)) => Ok(JarPlan::Interactive(InteractiveReason::JarInsufficient)),
        Ok(None) => Ok(JarPlan::Interactive(InteractiveReason::JarMissing)),
        Err(_) => Err(AuthError::at(
            AuthErrorCode::SessionStorageFailed,
            AuthOperation::Reuse,
        )),
    }
}
fn budget_allows_cas(remaining: Duration) -> bool {
    remaining >= CAS_REQUEST_TIMEOUT
}

fn cas_session(gateway: &str, outcome: CasOutcome) -> Result<ReuseOutcome, AuthError> {
    match outcome {
        CasOutcome::FormRequired => Ok(ReuseOutcome::Interactive(InteractiveReason::FormRequired)),
        CasOutcome::NetError(error) => {
            log::info!(target: "exv.atrust_auth", "http.cas result=error reason={}", error.label());
            let mut failure =
                AuthError::at(AuthErrorCode::SessionProbeFailed, AuthOperation::Reuse);
            failure.native_code = error.http_status.map(i64::from);
            Err(failure)
        }
        CasOutcome::Ticket {
            sid_via_auth_check: Some(exchange),
            ..
        } if !exchange.is_empty() => {
            let header = exchange.cookie_header();
            let pairs = header
                .split(';')
                .filter_map(|pair| pair.trim().split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
            let session =
                session_from_cookie_pairs(gateway, &CasLoginOptions::default().user_agent, pairs)
                    .map_err(|error| {
                    AuthError::at(
                        match error {
                            CookieSessionError::MissingSession => AuthErrorCode::SessionMissing,
                            _ => AuthErrorCode::SessionInvalid,
                        },
                        AuthOperation::Extract,
                    )
                })?;
            Ok(ReuseOutcome::Ready(SessionArtifact {
                session,
                source: SessionSource::HttpCasReuse,
            }))
        }
        CasOutcome::Ticket { .. } => Err(AuthError::at(
            AuthErrorCode::SessionMissing,
            AuthOperation::Extract,
        )),
    }
}

impl ChannelTrait for HttpProbeAdapter {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            injection: InjectionCapability::Unavailable,
            can_render: false,
            visibility: VisibilityCapability::NoSurface,
            can_read_cookiejar: true,
            can_write_cookiejar: true,
            observation: ObservationFidelity::NoPage,
            probe_scope: ProbeScope::IndependentHttp,
        }
    }
    fn open(&mut self, request: ChannelRequest) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.gateway = request.gateway_origin;
            self.deadline = Some(Instant::now() + HTTP_BUDGET);
            self.completed = false;
            match crate::atrust_session_cache::load(&self.profile, &self.gateway) {
                Ok(cached) => {
                    self.cached = cached;
                    self.cache_error = None;
                }
                Err(_) => {
                    self.cached = None;
                    self.cache_error = Some(AuthError::at(
                        AuthErrorCode::SessionStorageFailed,
                        AuthOperation::Open,
                    ));
                }
            }
            Ok(())
        })
    }
    fn navigate(&mut self, _url: String) -> ChannelFuture<'_, ()> {
        Box::pin(async {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Navigate,
            ))
        })
    }
    fn observe(&mut self) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async {
            Ok(PageObservation {
                sequence: 0,
                url: Observation::Unavailable,
                origin: Observation::Unavailable,
                dom: Observation::Unavailable,
                startup: None,
                coherent: false,
            })
        })
    }
    fn probe_session(&mut self) -> ChannelFuture<'_, SessionStatus> {
        Box::pin(async move {
            if let Some(error) = self.cache_error.take() {
                return Err(error);
            }
            let Some(cached) = &self.cached else {
                return Ok(SessionStatus::NotYetObserved);
            };
            let probe = tokio::time::timeout(
                PROBE_TIMEOUT.min(self.remaining()),
                crate::atrust_session_probe::probe_auth_config(
                    &self.gateway,
                    cached.cookie_header(),
                    cached.user_agent(),
                ),
            )
            .await
            .unwrap_or(Err(ProbeError {
                code: "timeout",
                http_status: None,
            }));
            let status = probe_status(probe)?;
            if matches!(status, SessionStatus::LoggedOut { .. }) {
                self.cached = None;
            }
            Ok(status)
        })
    }
    fn reuse_session(&mut self, budget: Duration) -> ChannelFuture<'_, ReuseOutcome> {
        Box::pin(async move {
            let mut jar = match plan_jar_run(crate::sso_session_jar::load(&self.profile))? {
                JarPlan::Run(jar) => jar,
                JarPlan::Interactive(reason) => return Ok(ReuseOutcome::Interactive(reason)),
            };
            let remaining = budget.min(self.remaining());
            if !budget_allows_cas(remaining) {
                return Ok(ReuseOutcome::Interactive(InteractiveReason::BudgetExpired));
            }
            let options = CasLoginOptions {
                budget: CAS_TOTAL_BUDGET.min(remaining),
                ..CasLoginOptions::default()
            };
            // 每次复用构造新协议客户端；不存在通道自己的认证循环或重试。
            let mut client =
                match CasLoginClient::new(&self.gateway, Arc::new(DirectUnderlay), options) {
                    Ok(client) => client,
                    Err(_) => {
                        return Ok(ReuseOutcome::Interactive(
                            InteractiveReason::CasConfigurationInvalid,
                        ));
                    }
                };
            let outcome = client.login(&mut jar).await;
            // SSO 轮换与 deviceId 沿用同档案事实，失败不删除整个档案。
            if crate::sso_session_jar::save(&self.profile, &jar).is_err() {
                log::warn!(target: "exv.atrust_auth", "{}", AuthError::at(AuthErrorCode::SessionStorageFailed, AuthOperation::Reuse).diagnostic_log_line());
            }
            cas_session(&self.gateway, outcome)
        })
    }
    fn inject_once(
        &mut self,
        _credentials: CredentialDelivery,
    ) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Inject,
            ))
        })
    }
    fn extract_session(&mut self) -> ChannelFuture<'_, Option<SessionArtifact>> {
        Box::pin(async move {
            Ok(self.cached.take().map(|session| SessionArtifact {
                session,
                source: SessionSource::GatewayCache,
            }))
        })
    }
    fn complete_session<'a>(&'a mut self, artifact: &'a SessionArtifact) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if !self.completed {
                self.completed = true;
                if crate::atrust_session_cache::save(&self.profile, &artifact.session).is_err() {
                    log::warn!(target: "exv.atrust_auth", "{}", AuthError::at(AuthErrorCode::SessionStorageFailed, AuthOperation::Extract).diagnostic_log_line());
                }
            }
            Ok(())
        })
    }
    fn present(&mut self, _intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async { Ok(PresentationOutcome::NoSurface) })
    }
    fn wait_for_change(&mut self, _max_wait: Duration) -> ChannelFuture<'_, ()> {
        Box::pin(async {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Wait,
            ))
        })
    }
    fn close(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.cached = None;
            self.cache_error = None;
            self.deadline = None;
            Ok(())
        })
    }
}
