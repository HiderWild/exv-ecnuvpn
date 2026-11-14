//! 用户新建 headless 连接的缓存准备；仍由 Core 配置锁和原候选回执裁决。
use super::*;
use crate::atrust_session_probe::{PROBE_TIMEOUT, ProbeError};
use exv_vpn_darwin_ipc::browser_session::BrowserSession;

pub(super) struct CachedProbeInput {
    config: DarwinUiConfig,
    gateway: String,
    cookie: zeroize::Zeroizing<String>,
    user_agent: String,
}
impl CachedProbeInput {
    pub(super) fn capture(config: &DarwinUiConfig, candidate: &BrowserSession) -> Self {
        Self {
            config: config.clone(),
            gateway: candidate.gateway().to_owned(),
            cookie: zeroize::Zeroizing::new(candidate.cookie_header().to_owned()),
            user_agent: candidate.user_agent().to_owned(),
        }
    }
}
impl LiveConnectionProjection {
    pub(super) async fn prepare_cached_headless(
        &self,
        input: CachedProbeInput,
        receipt: Option<&atrust_session_cache::CandidateReceipt>,
        generation: u64,
        auth_deadline: Option<AuthDeadline>,
    ) -> Result<(), Status> {
        // 无版本凭证时不能以旧候选的结果启动新认证或提交连接。
        let receipt = receipt
            .ok_or_else(|| Status::aborted("DARWIN_CORE_ATRUST_CACHE_VERSION_UNAVAILABLE"))?;
        let probe = async {
            #[cfg(test)]
            if let Some(probe) = &self.cached_session_probe {
                return probe().await;
            }
            crate::atrust_session_probe::probe_auth_config(
                &input.gateway,
                &input.cookie,
                &input.user_agent,
                PROBE_TIMEOUT,
                Arc::new(crate::atrust_http_underlay::HostAuthUnderlay),
            )
            .await
        };
        let cancelled = async {
            while generation == self.reconnect_generation.current() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        // 不持配置锁、会话锁或来源锁等待网络；本地超时是未知结果，不能遗忘缓存。
        let total_expired = async {
            match auth_deadline {
                Some(deadline) => {
                    tokio::time::sleep_until(
                        deadline.tokio_deadline().map_err(auth_deadline_status)?,
                    )
                    .await;
                    Err::<(), Status>(auth_deadline_status(AuthDeadlineError::Expired))
                }
                None => std::future::pending().await,
            }
        };
        check_auth_deadline(auth_deadline)?;
        let result = tokio::select! { biased;
            _ = cancelled => return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")),
            expired = total_expired => { expired?; unreachable!() },
            result = tokio::time::timeout(PROBE_TIMEOUT, probe) => result.unwrap_or(Err(ProbeError { code: "timeout", http_status: None })),
        };
        self.reconnect_generation
            .in_generation(generation, || {
                // 与既有状态投影统一 source→config 锁序，旧来源仍可能正在完成。
                let config = lock_recover(&self.config);
                check_auth_deadline(auth_deadline)?;
                if *config != input.config {
                    return Err(Status::aborted("DARWIN_CORE_AUTH_CONFIG_CHANGED"));
                }
                match receipt.is_current() {
                    Ok(true) => {}
                    Ok(false) => return Err(Status::aborted("DARWIN_CORE_ATRUST_CACHE_CHANGED")),
                    Err(_) => {
                        return Err(Status::aborted(
                            "DARWIN_CORE_ATRUST_CACHE_VERSION_UNAVAILABLE",
                        ));
                    }
                }
                match result {
                    Ok(status) if status.is_login => Ok(()),
                    Ok(_) => {
                        if receipt.forget_if_current().is_err() {
                            self.logs.aggregator().append_core(
                                "warn",
                                "core",
                                "ATRUST_SESSION_CACHE_FORGET_FAILED",
                                "网关明确报告未登录；旧缓存条件遗忘失败，本次继续认证",
                                &[],
                            );
                        }
                        let mut status = Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE);
                        status.metadata_mut().insert("x-exv-connect-cause", "atrust-cache-expired".parse().expect("固定缓存原因"));
                        Err(status)
                    }
                    Err(error) => {
                        self.logs.aggregator().append_core(
                            "info",
                            "core",
                            "ATRUST_SESSION_CACHE_PROBE_UNAVAILABLE",
                            "缓存探测未取得登录事实；保留缓存，以独立探测失败码拒绝（不弹凭据模态）",
                            &[("category", error.code.to_owned())],
                        );
                        // S2（2026-10-26 验收修复计划）：网络层失败=未知结果，
                        // 与「网关明确报告未登录」的凭据缺失码分离——错误处方
                        // 不再是重填密码，而是重试/重启（前端按本码路由文案）。
                        Err(Status::failed_precondition(
                            CONNECTION_PROBE_UNAVAILABLE_CODE,
                        ))
                    }
                }
            })
            .unwrap_or_else(|| Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")))
    }
}
