//! Darwin 局部页面策略；只解释已观察事实，未知页面继续等待。
use super::page_facts::{Observation, PageObservation, SessionStatus};
use super::page_facts::{
    extract_err_info_hint, sanitize_server_detail, text_is_failed_like, url_is_auth_config_path,
    url_marks_client_install, url_marks_client_shortcut, page_result_kind,
};
use std::time::Duration;
use exv_vpn_atrust::auth::browser_auth_policy::{classify_page_result, AuthFailureKind, PageResultFacts};

/// 已确认学校页面不计跨域停滞；学校表单交给人工或一次填交处理。
const SCHOOL_LOGIN_ORIGIN: &str = "https://sso.ecnu.edu.cn";
/// 当前批准的局部停滞默认上限。
pub const PAGE_STALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyErrorCode {
    ClientInstallRequired,
    FailedPage,
    Stalled,
    GatewayThrottled,
}

/// 仅含净化后的服务端摘录；最终用户因果文案由连接边界生成。
pub struct PolicyError {
    pub code: PolicyErrorCode,
    pub detail: Option<String>,
    pub failure_kind: Option<AuthFailureKind>,
}

/// 当前尝试的局部策略。敏感 URL 仅用于 latch 与进展比较，不提供 Debug 或日志。
#[derive(Default)]
pub struct PagePolicy {
    handover_url: Option<String>,
    last_url: Option<String>,
    last_sample: Option<Duration>,
    gateway_active: bool,
    cross_active: bool,
    frozen: bool,
    gateway_elapsed: Duration,
    cross_elapsed: Duration,
    promise_active: bool,
    promise_elapsed: Duration,
    remembered_install: bool,
    gateway_recovery: bool,
    gateway_stall_from_page: bool,
    install_recovery_needed: bool,
}

impl PagePolicy {
    /// 内嵌线会先尝试脱离安装页，系统旧线保持即时分类；只配置已有业务差异。
    pub fn with_gateway_recovery(enabled: bool) -> Self {
        Self::with_gateway_policy(enabled, enabled)
    }
    /// 观测事实计时与安装恢复分别声明；Unavailable 本身始终不是未登录。
    pub fn with_gateway_policy(recovery: bool, stall_from_page: bool) -> Self {
        Self {
            gateway_recovery: recovery,
            gateway_stall_from_page: stall_from_page,
            ..Self::default()
        }
    }
    pub fn install_recovery_needed(&self) -> bool {
        self.install_recovery_needed
    }
    /// 返回当前交接保护事实；未知页面、未采到及不可达不构成失败。
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        page: &PageObservation,
        status: &SessionStatus,
        gateway: &str,
        auth_path: &str,
        started: bool,
        now: Duration,
        stall: Duration,
    ) -> Result<bool, PolicyError> {
        let url = match &page.url {
            Observation::Observed(s) => Some(s.as_str()),
            _ => None,
        };
        let origin = match &page.origin {
            Observation::Observed(s) => Some(s.as_str()),
            _ => None,
        };
        let at_gateway = origin == Some(gateway);
        self.install_recovery_needed = false;
        if let Some(url) = url {
            if started
                && at_gateway
                && let Some(_) = exv_vpn_darwin_core::atrust_cas_facts::extract_cas_handover(url)
            {
                self.handover_url = Some(url.to_owned());
            } else if self.handover_url.as_deref().is_some_and(|old| old != url) {
                self.handover_url = None;
            }
        }
        let latched = url.is_some() && self.handover_url.as_deref() == url;
        let dom = if page.coherent && url.is_some() {
            match &page.dom {
                Observation::Observed(d) => Some(d),
                _ => None,
            }
        } else {
            None
        };
        let failed_text = dom.and_then(|d| {
            d.short_body_text.as_deref().filter(|t| {
                d.body_char_count <= 60 && d.body_has_failure_phrase && text_is_failed_like(t)
            })
        });
        let error_text = dom
            .and_then(|d| d.error_container_text.as_deref())
            .filter(|s| !s.trim().is_empty());
        let auth_config = url.is_some_and(|u| url_is_auth_config_path(u, auth_path));
        let detail = url.and_then(extract_err_info_hint).or_else(|| {
            (!auth_config).then(|| error_text.or(failed_text).map(sanitize_server_detail)).flatten()
        });
        if let Some(kind) = classify_page_result(&PageResultFacts {
            coherent: page.coherent,
            result: url.map(page_result_kind).unwrap_or_default(),
            detail: detail.as_deref(),
            dom_failure: !auth_config && (error_text.is_some() || failed_text.is_some()),
            startup: page.startup,
        }) {
            let code = if kind == AuthFailureKind::GatewayThrottled { PolicyErrorCode::GatewayThrottled } else { PolicyErrorCode::FailedPage };
            return Err(PolicyError { code, detail, failure_kind: Some(kind) });
        }
        let install = url.is_some_and(|u| url_marks_client_install(u) || url_marks_client_shortcut(u))
            || dom.is_some_and(|d| d.client_install_marker || d.shortcut_page || (d.download_page && at_gateway));
        if install && started {
            if latched {
                self.remembered_install = true;
            } else if self.gateway_recovery && at_gateway {
                if !auth_config {
                    self.remembered_install = true;
                    self.install_recovery_needed = true;
                }
            } else {
                return Err(PolicyError { code: PolicyErrorCode::ClientInstallRequired, detail: None, failure_kind: None });
            }
        }
        let gateway_active = started
            && at_gateway
            && url.is_some()
            && (matches!(status, SessionStatus::LoggedOut(_))
                || (self.gateway_stall_from_page && !matches!(status, SessionStatus::LoggedIn)));
        let cross_active = started
            && origin.is_some()
            && !at_gateway
            && origin != Some(SCHOOL_LOGIN_ORIGIN)
            && url.is_some();
        let progress =
            url.is_some() && self.last_url.as_deref().is_some_and(|old| Some(old) != url);
        let delta = self
            .last_sample
            .map_or(Duration::ZERO, |last| now.saturating_sub(last));
        // 仅当前 CAS 交接的未处理拒绝计局部停滞；处理器接管或导航进展后重新计时。
        let promise_active = latched && page.startup.is_some_and(|startup| startup.promise_rejected);
        if !promise_active || progress {
            self.promise_elapsed = Duration::ZERO;
        } else if self.promise_active {
            self.promise_elapsed += delta;
        }
        self.promise_active = promise_active;
        // 网关时钟在交接期间冻结，冻结边界两侧的区间也不累计。
        if !gateway_active {
            self.gateway_elapsed = Duration::ZERO;
        } else if self.gateway_active && !self.frozen {
            self.gateway_elapsed += delta;
        }
        if !cross_active || progress {
            self.cross_elapsed = Duration::ZERO;
        } else if self.cross_active && !self.frozen && !latched {
            self.cross_elapsed += delta;
        }
        self.gateway_active = gateway_active;
        self.cross_active = cross_active;
        self.frozen = latched;
        self.last_sample = Some(now);
        if let Some(url) = url {
            self.last_url = Some(url.to_owned());
        }
        if promise_active && self.promise_elapsed >= stall {
            return Err(PolicyError { code: PolicyErrorCode::Stalled, detail: None, failure_kind: None });
        }
        if !latched
            && ((gateway_active && self.gateway_elapsed >= stall)
                || (cross_active && self.cross_elapsed >= stall))
        {
            return Err(PolicyError {
                code: if self.remembered_install {
                    PolicyErrorCode::ClientInstallRequired
                } else {
                    PolicyErrorCode::Stalled
                },
                detail: None,
                failure_kind: None,
            });
        }
        Ok(latched)
    }
}
