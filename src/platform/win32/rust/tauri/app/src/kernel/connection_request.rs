//! Tauri 本地连接请求的唯一代次、取消完成与提交仲裁。
use exv_core::atrust_browser::{BrowserAuthError, LoginAction};
use exv_core::browser_auth::AuthControl;
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use tokio::sync::watch;

/// 仅本次请求的准备动作；命令必须携带当前 generation。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthPreparationAction {
    Retry,
    DisableOfficialAndRetry,
    UseEmbedded,
    TrySavedLogin,
    ApproveCertificate,
    Cancel,
}

/// 当前请求本地准备交互；Browser 仅接收本次渠道切换。
#[derive(Debug, Clone, Copy, Default)]
pub(super) enum AuthPreparationPhase {
    #[default]
    Idle,
    Exclusive,
    Certificate {
        approval_required: bool,
    },
    Browser,
}
/// `Clone` 共享同一 `LoginState`（Arc 语义）：内嵌认证窗口的 Destroyed handler 需要
/// 持有一份副本走取消收尾路径（修复 1 干净收尾契约，2026-10-08 UI 重试脱同步）。
#[derive(Debug, Default, Clone)]
pub struct ConnectionRequests {
    state: std::sync::Arc<std::sync::Mutex<LoginState>>,
    pub(super) auth_acquisitions: std::sync::Arc<super::auth_acquisition::AuthAcquisitions>,
    pub(super) commit_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Default)]
struct LoginState {
    generation: u64,
    active: Option<ActiveLogin>,
}

#[derive(Debug)]
struct ActiveLogin {
    preparation: watch::Sender<Option<AuthPreparationAction>>,
    preparation_phase: AuthPreparationPhase,
    preparation_busy: bool,
    auth_deadline_monotonic_ns: u64,
    sender: watch::Sender<LoginAction>,
    finished: watch::Sender<bool>,
    core_connect_started: bool,
    auth_acquisition: bool,
    generation: u64,
}

/// 配置读取前创建的请求 lease，覆盖认证与 Core 提交。
///
/// 认证交互期间不会把 async gate 持在此 lease 上；CoreClient 只在发送 Core Connect 前
/// 用 [`Self::begin_core_connect`] 与 Stop 取消做原子仲裁，并一直持有其独立 gate 至 RPC 收束。
#[derive(Debug)]
pub struct ConnectionRequest {
    owner: std::sync::Arc<std::sync::Mutex<LoginState>>,
    sender: watch::Sender<LoginAction>,
    action: watch::Receiver<LoginAction>,
    generation: u64,
}

impl ConnectionRequest {
    pub(super) fn set_auth_control_deadline(&self, deadline: u64) {
        if let Ok(mut state) = self.owner.lock()
            && let Some(active) = state
                .active
                .as_mut()
                .filter(|active| active.generation == self.generation)
        {
            active.auth_deadline_monotonic_ns = deadline;
        }
    }

    /// 发给当前 UI 等待事件的 owner 代际；取消后下一次 owner 使用新代际。
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 在共享 owner 锁内把等待态转换成 Connect RPC 已开始。若 Stop 已先发出取消则返回 false。
    #[must_use]
    pub fn begin_core_connect(&self) -> bool {
        let Ok(mut state) = self.owner.lock() else {
            return false;
        };
        if state.generation != self.generation {
            return false;
        }
        let Some(active) = state
            .active
            .as_mut()
            .filter(|active| active.sender.same_channel(&self.sender))
        else {
            return false;
        };
        if active.generation != self.generation || *active.sender.borrow() == LoginAction::Cancel {
            return false;
        }
        active.core_connect_started = true;
        true
    }

    pub(super) fn mark_auth_acquisition(&self) {
        if let Ok(mut state) = self.owner.lock()
            && let Some(active) = state
                .active
                .as_mut()
                .filter(|active| active.sender.same_channel(&self.sender))
        {
            active.auth_acquisition = true;
        }
    }

    /// 本地关窗的真实失败可回交仍在等待的 Core 原轮；已清除/替代由伴生消费者先拒绝。
    pub(super) fn begin_auth_failure_reply(&self) -> bool {
        let Ok(mut state) = self.owner.lock() else {
            return false;
        };
        let Some(active) = state.active.as_mut().filter(|active| {
            active.generation == self.generation && active.sender.same_channel(&self.sender)
        }) else {
            return false;
        };
        active.core_connect_started = true;
        true
    }

    /// 配置读取也受同一请求的取消通道约束。
    pub async fn cancelled(&self) {
        let mut action = self.action.clone();
        while *action.borrow() != LoginAction::Cancel {
            if action.changed().await.is_err() {
                break;
            }
        }
    }

    pub fn set_auth_mode(&self, embedded: bool) -> Result<(), String> {
        let state = self.owner.lock().map_err(|_| "连接请求状态不可用")?;
        if state.generation != self.generation || self.is_cancelled() {
            return Err("浏览器登录已取消".into());
        }
        self.sender.send_replace(if embedded {
            LoginAction::Embedded
        } else {
            LoginAction::External
        });
        Ok(())
    }

    fn is_cancelled(&self) -> bool {
        *self.action.borrow() == LoginAction::Cancel
    }
}

struct LoginStatus<F: Fn(bool)>(F);
impl<F: Fn(bool)> Drop for LoginStatus<F> {
    fn drop(&mut self) {
        (self.0)(false);
    }
}

impl Drop for ConnectionRequest {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.lock()
            && state
                .active
                .as_ref()
                .is_some_and(|active| active.sender.same_channel(&self.sender))
        {
            if let Some(active) = state.active.as_ref() {
                active.finished.send_replace(true);
            }
            state.active = None;
        }
    }
}

impl ConnectionRequests {
    /// 请求取消后仍由原 lease 持有此槽，直到实际收尾完成。
    pub(super) fn has_active_request(&self) -> bool {
        self.state
            .lock()
            .expect("connection request state")
            .active
            .is_some()
    }

    pub(super) fn set_auth_preparation(
        &self,
        generation: u64,
        phase: AuthPreparationPhase,
        busy: bool,
    ) {
        if let Ok(mut state) = self.state.lock()
            && let Some(active) = state
                .active
                .as_mut()
                .filter(|active| active.generation == generation)
        {
            active.preparation_phase = phase;
            active.preparation_busy = busy;
        }
    }

    pub fn auth_preparation_actions(
        &self,
        generation: u64,
    ) -> Result<watch::Receiver<Option<AuthPreparationAction>>, String> {
        let state = self.state.lock().map_err(|_| "连接请求状态不可用")?;
        let active = state
            .active
            .as_ref()
            .filter(|active| active.generation == generation)
            .ok_or("本次认证请求已结束")?;
        Ok(active.preparation.subscribe())
    }

    pub fn respond_auth_preparation(
        &self,
        generation: u64,
        action: AuthPreparationAction,
    ) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "连接请求状态不可用")?;
        let active = state
            .active
            .as_mut()
            .filter(|active| {
                active.generation == generation
                    && !active.core_connect_started
                    && *active.sender.borrow() != LoginAction::Cancel
            })
            .ok_or("本次认证请求已结束")?;
        if action != AuthPreparationAction::Cancel {
            if active.preparation_busy {
                return Err("本次认证准备操作正在执行".into());
            }
            let allowed = match active.preparation_phase {
                AuthPreparationPhase::Idle => false,
                AuthPreparationPhase::Exclusive => matches!(
                    action,
                    AuthPreparationAction::Retry
                        | AuthPreparationAction::DisableOfficialAndRetry
                        | AuthPreparationAction::UseEmbedded
                        | AuthPreparationAction::TrySavedLogin
                ),
                AuthPreparationPhase::Certificate { approval_required } => {
                    matches!(
                        action,
                        AuthPreparationAction::Retry
                            | AuthPreparationAction::UseEmbedded
                            | AuthPreparationAction::TrySavedLogin
                    ) || (approval_required && action == AuthPreparationAction::ApproveCertificate)
                }
                AuthPreparationPhase::Browser => matches!(
                    action,
                    AuthPreparationAction::UseEmbedded | AuthPreparationAction::TrySavedLogin
                ),
            };
            if !allowed {
                return Err("当前认证准备阶段不接受此操作".into());
            }
            active.preparation_busy = true;
        }
        if action == AuthPreparationAction::Cancel {
            active.sender.send_replace(LoginAction::Cancel);
        }
        if matches!(
            action,
            AuthPreparationAction::UseEmbedded | AuthPreparationAction::TrySavedLogin
        ) {
            active.sender.send_replace(LoginAction::Embedded);
        }
        active.preparation.send_replace(Some(action));
        Ok(())
    }

    pub(super) fn auth_control_deadline(&self, generation: u64) -> Result<u64, String> {
        self.state
            .lock()
            .map_err(|_| "连接请求状态不可用")?
            .active
            .as_ref()
            .filter(|active| {
                active.generation == generation && active.auth_deadline_monotonic_ns != 0
            })
            .map(|active| active.auth_deadline_monotonic_ns)
            .ok_or_else(|| "本次认证期限已结束".into())
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.state.lock().map_or(u64::MAX, |state| state.generation)
    }

    pub fn begin(&self, embedded: bool) -> Result<ConnectionRequest, String> {
        self.begin_with_generation(embedded, self.generation())
    }

    pub fn begin_with_generation(
        &self,
        embedded: bool,
        expected_generation: u64,
    ) -> Result<ConnectionRequest, String> {
        let mut state = self.state.lock().map_err(|_| "浏览器登录状态不可用")?;
        if state.generation != expected_generation {
            return Err("浏览器登录已取消".into());
        }
        if state.active.is_some() {
            return Err("正在等待浏览器登录，请完成或取消当前登录".into());
        }
        // 修复 1（2026-10-08 UI 重试脱同步，主案）：每次成功登记即递增代际，并以
        // 新代登记 ActiveLogin 与返回的 lease。此前代只在取消（Stop）时递增，认证
        // 失败后的重试以同一代 emit active:true，被前端「代 ≤ 已退役代即丢弃」过滤
        // 器整体吞掉，等待模态全程缺失。等值校验保留——取消后旧 expected 仍被拒，
        // 语义不变；由此任何尝试终结（完成/失败/关窗/取消）后，下一次 begin 天然
        // 携带严格更大的代（干净收尾四件套之②）。
        let generation = expected_generation.wrapping_add(1);
        state.generation = generation;
        let initial = if embedded {
            LoginAction::Embedded
        } else {
            LoginAction::External
        };
        let (sender, action) = watch::channel(initial);
        let (finished, _finished_receiver) = watch::channel(false);
        state.active = Some(ActiveLogin {
            preparation: watch::channel(None).0,
            preparation_phase: AuthPreparationPhase::Idle,
            preparation_busy: false,
            auth_deadline_monotonic_ns: 0,
            sender: sender.clone(),
            finished,
            core_connect_started: false,
            auth_acquisition: false,
            generation,
        });
        Ok(ConnectionRequest {
            owner: std::sync::Arc::clone(&self.state),
            sender,
            action,
            generation,
        })
    }

    /// 取消仍在等待中的浏览器认证。Core Connect 已原子标记开始时返回 false，调用方必须
    /// 等待 Connect RPC 收束后再执行真正的 Core Stop。
    #[must_use]
    pub fn cancel(&self) -> bool {
        self.cancel_with_completion().is_some()
    }

    /// 发出取消并返回可等待的 owner 完成通知。None 表示没有可取消的待认证 owner，
    /// 或该 owner 已进入 Core Connect 提交阶段。
    #[must_use]
    pub fn cancel_with_completion(&self) -> Option<watch::Receiver<bool>> {
        let mut state = self.state.lock().ok()?;
        state.generation = state.generation.wrapping_add(1);
        let active = state.active.as_ref()?;
        if active.core_connect_started {
            return None;
        }
        let finished = active.finished.subscribe();
        active.sender.send_replace(LoginAction::Cancel);
        Some(finished)
    }

    /// 修复 1 干净收尾（关窗变体）：窗口 Destroyed 时的取消收尾路径。
    ///
    /// 与 [`Self::cancel_with_completion`] 的差别在**代际守卫**——只有当前代仍是
    /// `generation` 对应的这次尝试（即它尚未收尾、也没有更新的尝试登记）时才
    /// 递增代际并向活动 owner 发 Cancel；代际不匹配（本尝试已终结、或新尝试已经
    /// `begin_with_generation` 登记到更大代）时只保留日志语义、不做任何状态变更，
    /// 保证残留窗口被下一次认证开头清理（`stale.close()`）或用户延迟关闭时，
    /// 迟到的 Destroyed 绝不误杀新尝试、也不重复推进代际。
    pub fn cancel_if_still_owner(&self, generation: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.generation != generation {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        let Some(active) = state.active.as_ref() else {
            return;
        };
        if active.generation == generation && !active.core_connect_started {
            active.sender.send_replace(LoginAction::Cancel);
        }
    }

    /// 归属随原 lease 保持到真实回执，清除 companion 不提前退役该事实。
    pub(super) fn has_auth_acquisition_owner(&self) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .active
                .as_ref()
                .is_some_and(|active| active.auth_acquisition)
        })
    }

    pub(super) fn cancel_auth_owner(&self, generation: u64) -> Option<watch::Receiver<bool>> {
        let mut state = self.state.lock().ok()?;
        if state.generation != generation {
            return None;
        }
        let active = state.active.as_ref()?;
        if active.generation != generation {
            return None;
        }
        let finished = active.finished.subscribe();
        if !active.core_connect_started {
            active.sender.send_replace(LoginAction::Cancel);
            state.generation = state.generation.wrapping_add(1);
        }
        Some(finished)
    }

    /// 将当前内嵌认证切换到系统默认浏览器，不结束本次登录 owner。
    pub fn request_system_browser_fallback(&self) -> Result<(), String> {
        let state = self.state.lock().map_err(|_| "浏览器登录状态不可用")?;
        let Some(active) = state.active.as_ref() else {
            return Err("当前没有等待中的内嵌登录".into());
        };
        if active.core_connect_started || *active.sender.borrow() != LoginAction::Embedded {
            return Err("当前登录方式不支持切换浏览器".into());
        }
        active
            .sender
            .send(LoginAction::UseSystemBrowser)
            .map_err(|_| "当前登录已结束".into())
    }

    #[must_use]
    pub fn system_browser_fallback_available(&self) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|state| {
                state.active.as_ref().map(|active| {
                    !active.core_connect_started && *active.sender.borrow() == LoginAction::Embedded
                })
            })
            .unwrap_or(false)
    }

    /// 在同一登录所有权与动作通道下运行内嵌或外部浏览器。
    pub async fn acquire_with_driver<F, Fut>(
        &self,
        status: impl Fn(bool),
        embedded: bool,
        driver: F,
    ) -> Result<BrowserSession, String>
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, String>>,
    {
        let (session, lease) = self
            .acquire_with_driver_held(status, embedded, driver)
            .await?;
        drop(lease);
        Ok(session)
    }

    /// 在同一登录所有权与动作通道下运行浏览器，并将成功认证的 lease 交给调用方。
    pub async fn acquire_with_driver_held<F, Fut>(
        &self,
        status: impl Fn(bool),
        embedded: bool,
        driver: F,
    ) -> Result<(BrowserSession, ConnectionRequest), String>
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, String>>,
    {
        let lease = self.begin(embedded)?;
        self.acquire_with_lease_held(lease, status, driver).await
    }

    /// 运行已在 admission 短锁内登记的 owner；浏览器交互期间不持有 admission 锁。
    pub async fn acquire_with_lease_held<F, Fut>(
        &self,
        lease: ConnectionRequest,
        status: impl Fn(bool),
        driver: F,
    ) -> Result<(BrowserSession, ConnectionRequest), String>
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, String>>,
    {
        self.acquire_with_typed_lease_held(lease, status, move |action| async move {
            driver(action).await.map_err(BrowserAuthError::Host)
        })
        .await
        .map_err(|error| error.legacy_message())
    }

    /// 汇聚本 owner 的终结结果；驱动保留统一错误、原生原因与控制三种来源。
    pub async fn acquire_with_typed_lease_held<F, Fut>(
        &self,
        lease: ConnectionRequest,
        status: impl Fn(bool),
        driver: F,
    ) -> Result<(BrowserSession, ConnectionRequest), BrowserAuthError>
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, BrowserAuthError>>,
    {
        self.acquire_with_typed_lease_held_and_success(lease, status, |_| {}, driver)
            .await
    }

    pub(super) async fn acquire_with_typed_lease_held_and_success<F, Fut>(
        &self,
        lease: ConnectionRequest,
        status: impl Fn(bool),
        authenticated: impl Fn(u64),
        driver: F,
    ) -> Result<(BrowserSession, ConnectionRequest), BrowserAuthError>
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, BrowserAuthError>>,
    {
        let (result, lease) = self
            .acquire_outcome_with_lease(lease, status, authenticated, driver)
            .await;
        result.map(|session| (session, lease))
    }

    /// 失败回交同样持有原 owner，直到 RespondInteraction 真正返回。
    pub(super) async fn acquire_outcome_with_lease<F, Fut>(
        &self,
        lease: ConnectionRequest,
        status: impl Fn(bool),
        authenticated: impl Fn(u64),
        driver: F,
    ) -> (Result<BrowserSession, BrowserAuthError>, ConnectionRequest)
    where
        F: FnOnce(watch::Receiver<LoginAction>) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserSession, BrowserAuthError>>,
    {
        let status = LoginStatus(status);
        (status.0)(true);
        let session = match driver(lease.action.clone()).await {
            Ok(session) => session,
            Err(error) => {
                log::warn!(target: "exv.atrust_auth", "{}", error.log_line());
                return (Err(error), lease);
            }
        };
        let current = self.state.lock().is_ok_and(|state| {
            state.generation == lease.generation
                && state.active.as_ref().is_some_and(|active| {
                    active.generation == lease.generation
                        && active.sender.same_channel(&lease.sender)
                })
        });
        if !current || lease.is_cancelled() {
            let error = BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消");
            log::warn!(target: "exv.atrust_auth", "{}", error.log_line());
            return (Err(error), lease);
        }
        authenticated(lease.generation());
        (Ok(session), lease)
    }
}
