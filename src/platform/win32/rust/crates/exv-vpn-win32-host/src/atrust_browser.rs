//! 用户态系统默认 Chromium 认证交接；只将网关会话交给 Engine，不读取学校密码。
//!
//! 观测约定（P1 观测修复，2026-10-11 计划）：本模块诊断日志统一走 `log::` 门面、
//! target `exv.atrust_auth`——认证代码在 UI 进程内运行，tauri_plugin_log 桥接使其
//! 落 ui.log；此前 `tracing::` 在 UI 进程无全局 subscriber 恒丢弃（观测盲区）。

use crate::browser_auth::page_facts::{percent_decode_component, sanitize_server_detail};
use crate::browser_auth::{AuthControl, AuthError, AuthErrorCode, AuthIntent, AuthOperation};
use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_atrust::browser_selection::{BrowserKind, Decision, SelectionCache, WINDOWS_SUPPORTED};
use exv_vpn_win32_ipc::browser_session::{
    BrowserSession, CookieSessionError, session_from_cookie_pairs,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::watch;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

mod browser_detection;
mod system_browser;

/// 手动登录的网关级持久浏览器目录（2026-10-26 S4c，对齐 darwin S4a 形态）。
///
/// 形态：`<现有根>/manual-persistent/<sha256(gateway)>/`（哈希与
/// `manual-username-<sha>` 同源，见 [`crate::browser_cache_profile::gateway_hash`]）。
/// 跨次登录复用：浏览器记住的密码/用户名与 SSO 会话随目录保留，EXV 不随登录
/// 尝试结束删除目录；"干净登录"诉求应由显式清理入口承接
/// （TODO(S4c)：win32 目前没有该入口，见 browser_cache_profile::manual_persistent_profile）。
pub struct ManualBrowserProfile {
    path: PathBuf,
}
impl ManualBrowserProfile {
    pub fn create(root: &Path, gateway: &str) -> Result<Self, String> {
        Ok(Self {
            path: crate::browser_cache_profile::manual_persistent_profile(root, gateway)?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 启动 future 取消时也收割本次已启动的进程；成功交接后由渠道 owner 接管。
/// 2026-10-26 S4c：目录不再随尝试销毁，收割标记（原用于目录删除门）一并移除。
struct LaunchedBrowser {
    child: Option<Child>,
}
impl LaunchedBrowser {
    fn new(child: Option<Child>) -> Self {
        Self { child }
    }
    fn reap(&mut self) -> Result<(), String> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {},
                Ok(None) => child.kill().map_err(|e| format!("结束本次认证浏览器失败：{e}"))?,
                Err(e) => return Err(format!("收割本次认证浏览器失败：{e}")),
            }
            child.wait().map_err(|e| format!("收割本次认证浏览器失败：{e}"))?;
            self.child = None;
        }
        Ok(())
    }
}
impl Drop for LaunchedBrowser {
    fn drop(&mut self) {
        if let Err(cause) = self.reap() {
            log::warn!(target: "exv.atrust_auth", "auth.browser_reap_failed error={cause}");
        }
    }
}

/// 官方 Windows aTrust UA（逐字还原，含来源注释）；与原生认证默认 UA 同一常量。
/// 原生桌面协议保留官方身份；浏览器事务显式选择普通 Chrome 身份并随会话交接。
pub use exv_vpn_atrust::auth::client_identity::{ATRUST_OFFICIAL_USER_AGENT, PLAIN_CHROME_USER_AGENT};

/// Windows 浏览器路径的 aTrust 客户端身份：`clientType=SDPBrowserClient`。
/// 平台名是协议字段，不按编译 target 推导；桌面/隧道交接身份见 win32-engine 的 `SDPClient`。
#[must_use]
pub fn client_identity() -> AuthClientIdentity {
    AuthClientIdentity::browser("Windows", "zh-CN")
        .expect("static Windows browser client identity is valid")
}

/// 与原生认证客户端同源的 authConfig 请求路径，供两个浏览器入口共用。
/// L1：浏览器路径使用 `clientType=SDPBrowserClient`（桌面/隧道路径保持 `SDPClient`）。
#[must_use]
pub fn auth_config_path(need_ticket: bool) -> String {
    client_identity().auth_config_path(need_ticket)
}

/// 当前 aTrust 浏览器登录在同一个 owner 内可发生的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginAction {
    Embedded,
    External,
    UseSystemBrowser,
    Cancel,
}

/// 页面分类的稳定机器码；前端据 `code` 分流三选项，不得解析自由文本。
pub const BROWSER_AUTH_CLIENT_INSTALL_REQUIRED: &str = "browser_auth_client_install_required";
/// 认证失败页（如白底居中纯文本「失败」）的稳定机器码。
pub const BROWSER_AUTH_FAILED_PAGE: &str = "browser_auth_failed_page";
/// 认证长时间无进展的稳定机器码。
pub const BROWSER_AUTH_STALLED: &str = "browser_auth_stalled";
/// 网关限流/并发登录冲突页（`#/common_info` + `type=common_error`，如 `source=logout_cas`）
/// 的稳定机器码（2026-10-07 终端页研究报告 §一(c2)/§5.3：该 URL 由服务端 302 构造，
/// 前端无写点——到达即证明网关侧拒绝登录，语义 = 网关限流/并发登录冲突）。
pub const BROWSER_AUTH_GATEWAY_THROTTLED: &str = "browser_auth_gateway_throttled";
/// 已真实发生但尚无法归类的统一认证错误码；宿主失败与控制信号另保留来源。
/// 不进入 AppError 的用户文案映射，前端不得依赖。
pub const BROWSER_AUTH_UNCLASSIFIED: &str = "browser_auth_unclassified";

/// 分类错误串中「稳定机器码」与「服务端消息摘录」的分隔符（ASCII 单元分隔符）。
/// 摘录经 [`crate::browser_auth::page_facts::sanitize_server_detail`] 净化后不可能含控制字符，故切分无歧义。
pub const SERVER_DETAIL_SEPARATOR: char = '\u{1f}';
/// 停滞判定的连续轮次阈值：内嵌探针空转与系统浏览器跨 origin 停滞共用同一阈值。
pub const AUTH_STALL_ROUNDS: u32 = 30;

/// 内部认证结果保留来源；兼容文案只在最终 String/AppError 边界使用。
pub enum BrowserAuthError {
    Auth {
        error: AuthError,
        compatibility_message: String,
    },
    Host(String),
    Control {
        control: AuthControl,
        compatibility_message: String,
    },
    /// B2（2026-10-27 浏览器选择计划）阶梯零可用回退信号：本次连接应改用内嵌
    /// 浏览器完成，**不是失败**。宿主 acquire 入口在可用集为空时发出；壳层收到
    /// 后改派内嵌通道，不改用户认证设置、不产生用户可见提示。泄漏到 legacy 文案
    /// 边界时按未知原文处理（不进入固定文案白名单）。
    FallBackEmbedded,
}
impl BrowserAuthError {
    pub fn auth(error: AuthError, compatibility_message: String) -> Self {
        Self::Auth {
            error,
            compatibility_message,
        }
    }
    pub fn control(control: AuthControl, compatibility_message: impl Into<String>) -> Self {
        Self::Control {
            control,
            compatibility_message: compatibility_message.into(),
        }
    }
    pub fn legacy_message(&self) -> String {
        match self {
            Self::Auth {
                compatibility_message,
                ..
            }
            | Self::Control {
                compatibility_message,
                ..
            } => compatibility_message.clone(),
            Self::Host(message) => message.clone(),
            Self::FallBackEmbedded => "无可用外部浏览器，本次连接改用内嵌浏览器".to_owned(),
        }
    }
    pub fn log_line(&self) -> String {
        match self {
            Self::Auth { error, .. } => error.log_line(),
            Self::Control { control, .. } => format!("auth.control control={control:?}"),
            Self::Host(message) => format!(
                "auth.host_failed message={}",
                sanitize_server_detail(message)
            ),
            Self::FallBackEmbedded => "auth.browser_fallback_embedded".to_owned(),
        }
    }
}
impl From<String> for BrowserAuthError {
    fn from(message: String) -> Self {
        Self::Host(message)
    }
}
impl From<&str> for BrowserAuthError {
    fn from(message: &str) -> Self {
        Self::Host(message.into())
    }
}
// Debug 不格式化宿主原文、服务端摘录或兼容文案。
impl std::fmt::Debug for BrowserAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth { error, .. } => f
                .debug_struct("Auth")
                .field("code", &error.code)
                .field("operation", &error.operation)
                .field("native_code", &error.native_code)
                .finish(),
            Self::Control { control, .. } => f.debug_tuple("Control").field(control).finish(),
            Self::Host(_) => f.write_str("Host"),
            Self::FallBackEmbedded => f.write_str("FallBackEmbedded"),
        }
    }
}

/// 单个 EXV 窗口的登录所有者；重复点击不启动第二个认证浏览器。
///
/// `Clone` 共享同一 `LoginState`（Arc 语义）：内嵌认证窗口的 Destroyed handler 需要
/// 持有一份副本走取消收尾路径（修复 1 干净收尾契约，2026-10-08 UI 重试脱同步）。
#[derive(Debug, Default, Clone)]
pub struct BrowserLogin {
    state: std::sync::Arc<std::sync::Mutex<LoginState>>,
}

#[derive(Debug, Default)]
struct LoginState {
    generation: u64,
    active: Option<ActiveLogin>,
}

#[derive(Debug)]
struct ActiveLogin {
    sender: watch::Sender<LoginAction>,
    finished: watch::Sender<bool>,
    generation: u64,
}

/// 独立认证探针的短期候选所有者。
///
/// 只管理认证通道；生产 Tauri 请求使用 kernel::connection_request 的独立请求寿命。
#[derive(Debug)]
pub struct BrowserLoginLease {
    owner: std::sync::Arc<std::sync::Mutex<LoginState>>,
    sender: watch::Sender<LoginAction>,
    action: watch::Receiver<LoginAction>,
    generation: u64,
}

impl BrowserLoginLease {
    /// 发给当前 UI 等待事件的 owner 代际；取消后下一次 owner 使用新代际。
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
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

impl Drop for BrowserLoginLease {
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

impl BrowserLogin {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.state.lock().map_or(u64::MAX, |state| state.generation)
    }

    pub fn begin(&self, embedded: bool) -> Result<BrowserLoginLease, String> {
        self.begin_with_generation(embedded, self.generation())
    }

    pub fn begin_with_generation(
        &self,
        embedded: bool,
        expected_generation: u64,
    ) -> Result<BrowserLoginLease, String> {
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
            sender: sender.clone(),
            finished,
            generation,
        });
        Ok(BrowserLoginLease {
            owner: std::sync::Arc::clone(&self.state),
            sender,
            action,
            generation,
        })
    }

    /// 取消本封装仍在等待中的认证通道。
    #[must_use]
    pub fn cancel(&self) -> bool {
        self.cancel_with_completion().is_some()
    }

    /// 发出取消并返回可等待的 owner 完成通知。None 表示没有可取消的待认证 owner，
    /// 完成通知只表示本地认证封装已退出。
    #[must_use]
    pub fn cancel_with_completion(&self) -> Option<watch::Receiver<bool>> {
        let mut state = self.state.lock().ok()?;
        state.generation = state.generation.wrapping_add(1);
        let active = state.active.as_ref()?;
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
        if active.generation == generation {
            active.sender.send_replace(LoginAction::Cancel);
        }
    }

    /// 将当前内嵌认证切换到系统默认浏览器，不结束本次登录 owner。
    pub fn request_system_browser_fallback(&self) -> Result<(), String> {
        let state = self.state.lock().map_err(|_| "浏览器登录状态不可用")?;
        let Some(active) = state.active.as_ref() else {
            return Err("当前没有等待中的内嵌登录".into());
        };
        if *active.sender.borrow() != LoginAction::Embedded {
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
                state
                    .active
                    .as_ref()
                    .map(|active| *active.sender.borrow() == LoginAction::Embedded)
            })
            .unwrap_or(false)
    }

    pub async fn acquire(&self, server: &str, profile: &Path) -> Result<BrowserSession, String> {
        self.acquire_with_status(server, profile, |_| {}).await
    }

    pub async fn acquire_with_status<F: Fn(bool)>(
        &self,
        server: &str,
        profile: &Path,
        status: F,
    ) -> Result<BrowserSession, String> {
        let (session, lease) = self
            .acquire_with_status_held(server, profile, status)
            .await?;
        drop(lease);
        Ok(session)
    }

    /// 与 [`Self::acquire_with_status`] 相同，但把 owner lease 交给调用方，供登录完成后的
    /// 检查认证取消与候选获取完成。
    pub async fn acquire_with_status_held<F: Fn(bool)>(
        &self,
        server: &str,
        profile: &Path,
        status: F,
    ) -> Result<(BrowserSession, BrowserLoginLease), String> {
        let lease = self.begin(false)?;
        self.acquire_with_typed_lease_held(lease, status, move |mut action| async move {
            acquire_typed(server, profile, &mut action, None).await
        })
        .await
        .map_err(|error| error.legacy_message())
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
    ) -> Result<(BrowserSession, BrowserLoginLease), String>
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
        lease: BrowserLoginLease,
        status: impl Fn(bool),
        driver: F,
    ) -> Result<(BrowserSession, BrowserLoginLease), String>
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
        lease: BrowserLoginLease,
        status: impl Fn(bool),
        driver: F,
    ) -> Result<(BrowserSession, BrowserLoginLease), BrowserAuthError>
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
                return Err(error);
            }
        };
        if lease.is_cancelled() {
            let error = BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消");
            log::warn!(target: "exv.atrust_auth", "{}", error.log_line());
            return Err(error);
        }
        Ok((session, lease))
    }
}


struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    sequence: u64,
    // 测试仅替换一次 HTTP I/O，CDP、页面事实与活动编排保持真实调用。
    #[cfg(test)]
    probe_io: Option<ProbeIo>,
    #[cfg(test)]
    before_create_commit: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
    #[cfg(test)]
    after_create_commit: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}
impl Cdp {
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.call_with_commit(method, params, session, None).await
    }

    async fn call_with_commit(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
        committed: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            #[cfg(test)]
            if method == "Target.createTarget" {
                if let Some((entered, release)) = self.before_create_commit.take() {
                    let _ = entered.send(());
                    let _ = release.await;
                }
            }
            let message = Message::Text(request.to_string().into());
            if let Some(committed) = committed {
                // feed 在 start_send 接受消息时完成；即使 flush 仍待写入，已不可撤销。
                self.socket
                    .feed(message)
                    .await
                    .map_err(|_| "浏览器调试连接已关闭".to_owned())?;
                committed.store(true, std::sync::atomic::Ordering::Relaxed);
                #[cfg(test)]
                if method == "Target.createTarget" {
                    if let Some((entered, release)) = self.after_create_commit.take() {
                        let _ = entered.send(());
                        let _ = release.await;
                    }
                }
                self.socket
                    .flush()
                    .await
                    .map_err(|_| "浏览器调试连接已关闭".to_owned())?;
            } else {
                self.socket
                    .send(message)
                    .await
                    .map_err(|_| "浏览器调试连接已关闭".to_owned())?;
            }
            while let Some(message) = self.socket.next().await {
                let message = message.map_err(|_| "浏览器调试连接读取失败")?;
                if let Message::Text(text) = message {
                    let response: Value =
                        serde_json::from_str(&text).map_err(|_| "浏览器响应格式错误")?;
                    if response["id"].as_u64() == Some(id) {
                        if response.get("error").is_some() {
                            return Err("浏览器命令执行失败".into());
                        }
                        return Ok(response["result"].clone());
                    }
                }
            }
            Err("浏览器已退出".into())
        })
        .await
        .map_err(|_| "浏览器命令超时".to_owned())?
    }
}

fn endpoint_from_file(port_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(port_file).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    (port != 0 && path.starts_with("/devtools/browser/"))
        .then(|| format!("ws://127.0.0.1:{port}{path}"))
}

async fn connect_endpoint(port_file: &Path) -> Result<Cdp, &'static str> {
    let endpoint = endpoint_from_file(port_file).ok_or("port-file")?;
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(1),
        tokio_tungstenite::connect_async(endpoint),
    )
    .await
    .map_err(|_| "websocket-timeout")?
    .map_err(|_| "websocket")?;
    let mut cdp = Cdp {
        socket,
        sequence: 0,
        #[cfg(test)]
        probe_io: None,
        #[cfg(test)]
        before_create_commit: None,
        #[cfg(test)]
        after_create_commit: None,
    };
    let version = tokio::time::timeout(
        Duration::from_secs(1),
        cdp.call("Browser.getVersion", json!({}), None),
    )
    .await
    .map_err(|_| "cdp-version-timeout")?
    .map_err(|_| "cdp-version")?;
    if !version["userAgent"]
        .as_str()
        .is_some_and(|agent| !agent.trim().is_empty())
    {
        return Err("cdp-version-shape");
    }
    Ok(cdp)
}

fn startup_timeout_error(auth_deadline: Option<tokio::time::Instant>) -> BrowserAuthError {
    if auth_deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
        BrowserAuthError::auth(
            AuthError::at(AuthErrorCode::AcquisitionTimeout, AuthOperation::Wait),
            "浏览器登录超时，请重试".into(),
        )
    } else {
        "无法连接 EXV 专用认证浏览器，请关闭该认证窗口后重试".into()
    }
}

fn startup_boundary_error(
    deadline: tokio::time::Instant,
    auth_deadline: Option<tokio::time::Instant>,
    action: &Option<&mut watch::Receiver<LoginAction>>,
) -> Option<BrowserAuthError> {
    if action
        .as_ref()
        .is_some_and(|a| *a.borrow() == LoginAction::Cancel)
    {
        Some(BrowserAuthError::control(
            AuthControl::Cancel,
            "浏览器登录已取消",
        ))
    } else if tokio::time::Instant::now() >= deadline {
        Some(startup_timeout_error(auth_deadline))
    } else {
        None
    }
}

// 旧测试与探针接缝的最终字符串转换；实现只在 typed helper 中。
/// 仅约束无 target 的启动与复用握手；认证原期限不会在此重新计时。
async fn connect_or_launch_until_typed<F>(
    port_file: &Path,
    startup_budget: Duration,
    auth_deadline: Option<tokio::time::Instant>,
    action: Option<&mut watch::Receiver<LoginAction>>,
    launch: F,
) -> Result<(Cdp, Option<Child>), BrowserAuthError>
where
    F: FnOnce() -> Result<Child, String>,
{
    connect_or_launch_owned_until_typed(port_file, startup_budget, auth_deadline, action, launch).await
}

async fn connect_or_launch_owned_until_typed<F>(
    port_file: &Path,
    startup_budget: Duration,
    auth_deadline: Option<tokio::time::Instant>,
    mut action: Option<&mut watch::Receiver<LoginAction>>,
    launch: F,
) -> Result<(Cdp, Option<Child>), BrowserAuthError>
where
    F: FnOnce() -> Result<Child, String>,
{
    let started = std::time::Instant::now();
    let local_deadline = tokio::time::Instant::now() + startup_budget;
    let deadline = auth_deadline.map_or(local_deadline, |d| d.min(local_deadline));
    if let Some(error) = startup_boundary_error(deadline, auth_deadline, &action) {
        return Err(error);
    }
    let initial = if let Some(action) = action.as_deref_mut() {
        tokio::select! {
            biased;
            () = wait_for_cancel(action) => return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")),
            result = tokio::time::timeout_at(deadline, connect_endpoint(port_file)) => {
                result.map_err(|_| startup_timeout_error(auth_deadline))?
            }
        }
    } else {
        tokio::time::timeout_at(deadline, connect_endpoint(port_file)).await
            .map_err(|_| startup_timeout_error(auth_deadline))?
    };
    // I/O 即使与期限/取消同刻返回，也不能越过下一复用或 launch 边界。
    if let Some(error) = startup_boundary_error(deadline, auth_deadline, &action) {
        return Err(error);
    }
    let mut last_stage = match initial {

        Ok(cdp) => {
            // P1 观测修复：认证路径诊断从 tracing::（UI 进程无 subscriber 恒丢弃）
            // 换 log:: 门面（tauri_plugin_log 桥接 ui.log），事件名沿用 exv.atrust_auth
            // target 的 `auth.*` 稳定词汇。
            log::info!(
                target: "exv.atrust_auth",
                "auth.cdp_endpoint result=reused elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return Ok((cdp, None));
        }
        Err(stage) => stage,
    };
    if let Some(error) = startup_boundary_error(deadline, auth_deadline, &action) {
        return Err(error);
    }
    let child = match launch() {
        Ok(child) => child,
        Err(error) => {
            return Err(startup_boundary_error(deadline, auth_deadline, &action)
                .unwrap_or_else(|| BrowserAuthError::Host(error)));
        }
    };
    let mut launched = LaunchedBrowser::new(Some(child));
    let child = launched.child.as_mut().expect("本次启动进程");
    let mut exit_observed = false;
    let result = {
        let wait_for_endpoint = async {
            loop {
                match connect_endpoint(port_file).await {
                    Ok(cdp) => return cdp,
                    Err(stage) => last_stage = stage,
                }
                if !exit_observed {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            exit_observed = true;
                            log::info!(
                                target: "exv.atrust_auth",
                                "auth.launcher state=exited_while_waiting elapsed_ms={} status={:?}",
                                started.elapsed().as_millis(),
                                status
                            );
                        }
                        Err(error) => log::warn!(
                            target: "exv.atrust_auth",
                            "auth.launcher inspect_failed error={:?}",
                            error
                        ),
                        Ok(None) => {}
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let boundary_error = startup_boundary_error(deadline, auth_deadline, &action);
        if let Some(error) = boundary_error {
            Err(error)
        } else if let Some(action) = action.as_deref_mut() {
            tokio::select! {
                biased;
                () = wait_for_cancel(action) => Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")),
                result = tokio::time::timeout_at(deadline, wait_for_endpoint) => {
                    result.map_err(|_| startup_timeout_error(auth_deadline))
                }
            }
        } else {
            tokio::time::timeout_at(deadline, wait_for_endpoint)
                .await
                .map_err(|_| startup_timeout_error(auth_deadline))
        }
    };
    let result = result.and_then(|cdp| match startup_boundary_error(deadline, auth_deadline, &action) {
        Some(error) => Err(error),
        None => Ok(cdp),
    });
    match result {
        Ok(cdp) => {
            if let Ok(Some(status)) = child.try_wait() {
                log::info!(
                    target: "exv.atrust_auth",
                    "auth.launcher state=exited_before_handshake elapsed_ms={} status={:?}",
                    started.elapsed().as_millis(),
                    status
                );
                exit_observed = true;
            }
            log::info!(
                target: "exv.atrust_auth",
                "auth.cdp_handshake result=ok elapsed_ms={} launcher_exited={}",
                started.elapsed().as_millis(),
                exit_observed
            );
            Ok((cdp, launched.child.take()))
        }
        Err(error) => {
            log::warn!(
                target: "exv.atrust_auth",
                "auth.cdp_handshake result=failed elapsed_ms={} launcher_exited={} stage={}",
                started.elapsed().as_millis(),
                exit_observed,
                last_stage
            );
            match launched.reap() {
                Ok(()) => Err(error),
                Err(cause) => Err(append_cleanup(error, &cause)),
            }
        }
    }
}

async fn finish_target<T>(
    cdp: &mut Cdp,
    target_id: &str,
    result: Result<T, String>,
) -> Result<T, String> {
    let closed = tokio::time::timeout(
        Duration::from_secs(2),
        cdp.call("Target.closeTarget", json!({"targetId": target_id}), None),
    )
    .await;
    // 仅诊断，不改行为：记录 closeTarget 是否确认成功，控制流仍只走下方既有告警。
    let confirmed = matches!(closed, Ok(Ok(ref result)) if result["success"] == true);
    log::info!(
        target: "exv.atrust_auth",
        "auth.close_target result={}",
        if confirmed { "ok" } else { "failure" }
    );
    if !confirmed {
        log::warn!(target: "exv.atrust_auth", "auth.close_target cleanup_unconfirmed");
    }
    result
}

/// 仅诊断：只保留 `scheme://host[:port]/path`，剥掉 query 与 fragment，避免记下 ticket 等参数。
fn sanitized_url(raw: &str) -> String {
    raw.split(['?', '#']).next().unwrap_or(raw).to_owned()
}

/// 仅诊断：从 CDP cookie 数组提取名称集合，排序去重后以逗号拼接；绝不记录 cookie 值。
fn cookie_name_set(cookies: &Value) -> String {
    let Some(entries) = cookies.as_array() else {
        return "unavailable".to_owned();
    };
    let mut names: Vec<&str> = entries
        .iter()
        .filter_map(|cookie| cookie["name"].as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(",")
    }
}

/// 页面分类采集表达式：返回当前 origin/href 与高置信度标记的 JSON 字符串。
/// 仅读取主框架可见文本与门户标记元素，不读取凭据或表单内容。
/// 2026-10-07 终端页研究报告修正：新增 `errText`（`#errorData` 非空文本的
/// `encodeURIComponent` 摘录，≤60 字符——该元素在每个 shortcut 页恒存在，只有
/// 文本非空才是失败事实）与 `head`（正文足够短时才携带的正文摘录，供失败短语
/// 判定与长度护栏共用同一上限）；两者与内嵌窗口线的 PAGE_FACTS_SCRIPT 保持同一
/// 编码形态（`encodeURIComponent`），Rust 侧共用同一解码与判定。
const PAGE_FACTS_EXPRESSION: &str = r#"(function(){try{var b=document.body;var t=(b&&typeof b.innerText==='string')?b.innerText.trim():'';var e=(document.querySelector&&document.querySelector('#errorData'));var et=(e&&typeof e.innerText==='string')?e.innerText.trim():'';return JSON.stringify({origin:location.origin,href:location.href,marker:!!(document.querySelector&&document.querySelector('.down-client-pc__body')),shortcut:!!(document.querySelector&&(document.querySelector('#shortcut-page')||document.querySelector('#errorData'))),download:t.indexOf('下载客户端')!==-1,hasFail:t.indexOf('失败')!==-1,len:Math.min(t.length,99),errText:encodeURIComponent(et.slice(0,60)),head:(t.length<=60)?encodeURIComponent(t.slice(0,60)):''});}catch(e){return null;}})()"#;

/// 系统浏览器页面分类所需的原始事实。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageFacts {
    origin: String,
    href: String,
    marker: bool,
    shortcut: bool,
    download: bool,
    has_fail: bool,
    len: usize,
    /// `#errorData` 非空文本的解码摘录（失败页服务端消息；`None` = 元素缺失或为空）。
    error_text: Option<String>,
    /// 短正文摘录（仅当正文长度不超过携带上限时非空），供失败短语判定。
    body_head: Option<String>,
}

/// 从 CDP `Runtime.evaluate` 回包解析页面事实；形状不符返回 `None`。
fn parse_page_facts(reply: &Value) -> Option<PageFacts> {
    let raw = reply["result"]["value"].as_str()?;
    let value: Value = serde_json::from_str(raw).ok()?;
    let decode_excerpt = |field: &str| -> Option<String> {
        let decoded = percent_decode_component(value[field].as_str().unwrap_or(""));
        (!decoded.trim().is_empty()).then_some(decoded)
    };
    Some(PageFacts {
        origin: value["origin"].as_str()?.to_owned(),
        href: value["href"].as_str()?.to_owned(),
        marker: value["marker"].as_bool().unwrap_or(false),
        shortcut: value["shortcut"].as_bool().unwrap_or(false),
        download: value["download"].as_bool().unwrap_or(false),
        has_fail: value["hasFail"].as_bool().unwrap_or(false),
        len: value["len"].as_u64().unwrap_or(0) as usize,
        error_text: decode_excerpt("errText"),
        body_head: decode_excerpt("head"),
    })
}

/// 浏览器配置子目录名（按支持集种类；win32 支持集不含 Safari，该臂仅供枚举穷尽）。
fn browser_profile_name(kind: BrowserKind) -> &'static str {
    match kind {
        BrowserKind::Edge => "edge",
        BrowserKind::Chrome => "chrome",
        BrowserKind::Safari => "safari",
    }
}

/// acquire 入口的阶梯结论：继续驱动的外部浏览器与 exe，或零可用回退信号。
#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowserSelection {
    Use { kind: BrowserKind, executable: PathBuf },
    FallBackEmbedded,
}

/// B2 进程内浏览器选择状态：共享模块缓存槽 + 最近一次探测的 exe 表（缓存命中
/// 路径的轻量复核输入）。生命周期=进程（应用重启即清），与计划缓存裁定一致。
#[derive(Default)]
struct BrowserSelectionState {
    cache: SelectionCache,
    executables: RefCell<BTreeMap<BrowserKind, PathBuf>>,
}

/// 阶梯+缓存接线核心：探测 IO 与可用性复核注入（生产=真实注册表探测，测试=快照
/// 夹具），产出 [`BrowserSelection`]。
///
/// 缓存四律由共享 [`SelectionCache::resolve`] 承担：无缓存才完整探测；Use 落缓存、
/// 命中且轻量复核（exe 在位）可用即短路；复核不可用清缓存重探测；零可用
/// FallBackEmbedded 永不落缓存。
fn resolve_selection_with<D, F>(
    cache: &mut SelectionCache,
    executables: &RefCell<BTreeMap<BrowserKind, PathBuf>>,
    detect: D,
    is_available: F,
) -> BrowserSelection
where
    D: FnOnce() -> browser_detection::BrowserRegistry,
    F: Fn(BrowserKind) -> bool,
{
    let decision = cache.resolve(
        WINDOWS_SUPPORTED,
        || {
            let registry = detect();
            let detection = registry.detection();
            *executables.borrow_mut() = registry.into_executables();
            detection
        },
        is_available,
    );
    match decision {
        Decision::Use(kind) => {
            let executable = executables.borrow().get(&kind).cloned();
            match executable {
                Some(executable) => BrowserSelection::Use { kind, executable },
                // 缓存命中路径 exe 表可能未刷新：复核已确认可用（含已知目录兜底），
                // 但无记录可驱动的路径 → 视同零可用走回退信号。
                None => BrowserSelection::FallBackEmbedded,
            }
        }
        Decision::FallBackEmbedded => BrowserSelection::FallBackEmbedded,
    }
}

/// acquire/system-browser 入口的阶梯求值（进程内静态持有缓存槽）。
///
/// 默认浏览器不受支持时不再报错：按优先序（Edge>Chrome）自动选用已装可用浏览器；
/// 零可用返回 [`BrowserSelection::FallBackEmbedded`] 供壳层静默改派内嵌。
fn resolve_browser_selection() -> BrowserSelection {
    static BROWSER_SELECTION: OnceLock<Mutex<BrowserSelectionState>> = OnceLock::new();
    let state = BROWSER_SELECTION.get_or_init(Default::default);
    let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
    let BrowserSelectionState { cache, executables } = &mut *guard;
    // 缓存槽独占可变；exe 表降为共享引用，供探测闭包（写经 RefCell）与复核闭包同持。
    let executables = &*executables;
    resolve_selection_with(
        cache,
        executables,
        browser_detection::detect_registry,
        |kind| browser_detection::executable_in_place(&executables.borrow(), kind),
    )
}

/// 系统浏览器路径的 `Network.setBlockedURLs` 模式列表。
///
/// 只保留 `atrust:*`：屏蔽该 scheme 是为防官方 aTrust 客户端被网页唤起。
/// **不再屏蔽 `localhost.sangfor.com.cn`**：L3 本机仿真服务监听 `127.0.0.1:54630/54631`，
/// 该主机名经 hosts 映射到回环；屏蔽它会让门户永远连不上仿真服务、判定恒失败。伪装已由
/// 用户裁定全面推行，故解除该主机的屏蔽。不得顺带放开其他屏蔽项。
const BLOCKED_URL_PATTERNS: &[&str] = &["atrust:*"];

// 旧 `system_default_browser`（只读 HTTPS UserChoice + 已知目录猜路径；默认不受
// 支持即 Err）已于 B2（2026-10-27）退役：默认浏览器读取与可用集枚举统一在
// `browser_detection`，入口 `resolve_browser_selection`；"默认不受支持即报错"
// 路径退役为静默回退内嵌（`BrowserAuthError::FallBackEmbedded`）。

/// 拉起 EXV 专用 Chrome 配置目录；Chrome 自身保存浏览器登录状态。
/// 不读取其他 Chrome 配置目录，不安装网卡或路由，也不调用官方客户端。
pub async fn acquire(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
) -> Result<BrowserSession, String> {
    acquire_typed(server, profile, action, attempt)
        .await
        .map_err(|error| error.legacy_message())
}

pub async fn acquire_typed(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
) -> Result<BrowserSession, BrowserAuthError> {
    acquire_typed_with_intent(server, profile, action, attempt, AuthIntent::Automatic).await
}

pub async fn acquire_typed_with_intent(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
    intent: AuthIntent,
) -> Result<BrowserSession, BrowserAuthError> {
    let gateway = validated_gateway(server)?;
    acquire_inner(&gateway, profile, action, attempt, None, None, intent).await
}

/// X3a 过渡接缝：认证使用本次连接的原始期限，创建收束与关闭沿用独立小清理预算。
pub async fn acquire_with_deadline(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
    deadline: tokio::time::Instant,
) -> Result<BrowserSession, String> {
    acquire_with_deadline_inner(server, profile, action, attempt, Some(deadline)).await
}

/// 活动 UI 回退继续借用同一预算与事件；系统通道不接受学校凭据。
pub async fn acquire_with_context(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: &crate::browser_auth::AuthAttempt,
    events: &dyn crate::browser_auth::AuthEventSink,
    snapshots: &dyn crate::browser_auth::SessionSnapshotProvider,
) -> Result<BrowserSession, String> {
    acquire_with_context_typed(server, profile, action, attempt, events, snapshots)
        .await
        .map_err(|error| error.legacy_message())
}

pub async fn acquire_with_context_typed(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: &crate::browser_auth::AuthAttempt,
    events: &dyn crate::browser_auth::AuthEventSink,
    snapshots: &dyn crate::browser_auth::SessionSnapshotProvider,
) -> Result<BrowserSession, BrowserAuthError> {
    acquire_with_context_intent_typed(server, profile, action, attempt, events, snapshots, AuthIntent::Automatic).await
}

pub async fn acquire_with_context_intent_typed(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: &crate::browser_auth::AuthAttempt,
    events: &dyn crate::browser_auth::AuthEventSink,
    snapshots: &dyn crate::browser_auth::SessionSnapshotProvider,
    intent: AuthIntent,
) -> Result<BrowserSession, BrowserAuthError> {
    let gateway = validated_gateway(server)?;
    let deadline = attempt.deadline(Duration::from_secs(300));
    acquire_inner(
        &gateway,
        profile,
        action,
        None,
        Some(deadline),
        Some((attempt, events, snapshots)),
        intent,
    )
    .await
}

/// 复用既有浏览器网关补全与校验规则，供本地绑定和 Core 接纳比较使用。
pub fn validated_gateway(server: &str) -> Result<String, String> {
    let gateway = if server.starts_with("https://") {
        server.trim_end_matches('/').to_owned()
    } else {
        format!("https://{server}")
    };
    BrowserSession::new(
        gateway.clone(),
        "sid=validation".into(),
        "validation".into(),
    )
    .map_err(|_| "aTrust 网关地址无效".to_owned())?;
    Ok(gateway)
}

async fn acquire_with_deadline_inner(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
    deadline: Option<tokio::time::Instant>,
) -> Result<BrowserSession, String> {
    let gateway = validated_gateway(server)?;
    acquire_inner(&gateway, profile, action, attempt, deadline, None, AuthIntent::Automatic)
        .await
        .map_err(|error| error.legacy_message())
}

async fn wait_for_cancel(action: &mut watch::Receiver<LoginAction>) {
    while *action.borrow() != LoginAction::Cancel {
        if action.changed().await.is_err() {
            break;
        }
    }
}

/// 外部浏览器启动参数（前五项=既有已验证集合；末两项
/// `--disable-component-update`/`--disable-sync` 为 2026-10-27 追加的
/// Chrome 自更新记账通知抑制尝试，与 darwin external.rs BROWSER_ARGS
/// 同集、疗效待 E2E 实证；经进程参数 API 传递，不拼 shell）。
const BROWSER_ARGS: [&str; 7] = [
    "--remote-debugging-port=0",
    "--remote-debugging-address=127.0.0.1",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
];

async fn acquire_inner(
    gateway: &str,
    base_profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
    deadline: Option<tokio::time::Instant>,
    context: Option<(
        &crate::browser_auth::AuthAttempt,
        &dyn crate::browser_auth::AuthEventSink,
        &dyn crate::browser_auth::SessionSnapshotProvider,
    )>,
    intent: AuthIntent,
) -> Result<BrowserSession, BrowserAuthError> {
    if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
        return Err(BrowserAuthError::auth(
            AuthError::at(AuthErrorCode::AcquisitionTimeout, AuthOperation::Wait),
            "浏览器登录超时，请重试".into(),
        ));
    }
    let (browser, executable) = match resolve_browser_selection() {
        BrowserSelection::Use { kind, executable } => (kind, executable),
        // B2 静默回退：零可用外部浏览器——类型化信号交壳层改派内嵌完成本次连接，
        // 不视为失败、不改用户认证设置。
        BrowserSelection::FallBackEmbedded => return Err(BrowserAuthError::FallBackEmbedded),
    };
    // 2026-10-26 S4c：手动登录改网关级持久目录（对齐 darwin S4a），跨次登录复用；
    // 浏览器记住的密码/用户名与 SSO 会话随目录保留，EXV 不再销毁该目录。
    let manual = if intent == AuthIntent::FreshManual {
        Some(ManualBrowserProfile::create(base_profile, gateway)?)
    } else { None };
    let browser_root = manual.as_ref().map_or(base_profile, |owner| owner.path());
    let browser_profile = browser_root.join(browser_profile_name(browser));
    use std::os::windows::process::CommandExt;
    std::fs::create_dir_all(&browser_profile).map_err(|_| "无法创建 EXV 浏览器配置目录")?;
    let port_file = browser_profile.join("DevToolsActivePort");
    let (mut cdp, launched_child) = connect_or_launch_owned_until_typed(
        &port_file,
        Duration::from_secs(15),
        deadline,
        Some(action),
        || {
            Command::new(executable)
                .arg(format!("--user-data-dir={}", browser_profile.display()))
                .args(BROWSER_ARGS)
                .arg("about:blank")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x08000000)
                .spawn()
                .map_err(|_| "无法启动系统默认浏览器".to_owned())
        },
    )
    .await?;
    // 自动路径保留持久浏览器；人工路径独占刚启动的进程，但目录同样持久保留。
    let mut process = LaunchedBrowser::new(if manual.is_some() { launched_child } else { None });
    let (result, cleanup_started) = if let Some((auth_attempt, events, snapshots)) = context {
        system_browser::acquire_connected_with_intent(
            &mut cdp,
            gateway,
            base_profile,
            action,
            Duration::from_secs(2),
            auth_attempt,
            events,
            snapshots,
            intent,
        )
        .await
    } else {
    let deadline = deadline.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_mins(5));
    let auth_attempt = crate::browser_auth::AuthAttempt::with_deadline(deadline);
    let dependencies = system_browser::DefaultDependencies { profile: base_profile };
    let _ = attempt; // 系统渠道没有学校凭据注入权限。
    system_browser::acquire_connected_with_intent(
        &mut cdp, gateway, base_profile, action, Duration::from_secs(2),
        &auth_attempt, &dependencies, &dependencies, intent,
    ).await
    };
    if manual.is_some() {
        // 持久目录跨次登录复用：先结束渠道 target 与本实例（干净关闭让浏览器把
        // 登录痕迹落盘），但不再删除 manual-persistent 目录。
        let cleanup_deadline = cleanup_started + Duration::from_secs(4) + Duration::from_millis(100);
        if tokio::time::Instant::now() < cleanup_deadline {
            let _ = tokio::time::timeout_at(cleanup_deadline.min(tokio::time::Instant::now() + Duration::from_secs(1)), cdp.call("Browser.close", json!({}), None)).await;
        }
        let deadline = cleanup_deadline.min(tokio::time::Instant::now() + Duration::from_secs(1));
        if let Some(child) = process.child.as_mut() {
            while matches!(child.try_wait(), Ok(None)) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        }
        if let Err(cause) = process.reap() {
            return match result {
                Ok(_) => Err(BrowserAuthError::Host(cause)),
                Err(error) => Err(append_cleanup(error, &cause)),
            };
        }
    }
    result
}

fn append_cleanup(mut error: BrowserAuthError, cleanup: &str) -> BrowserAuthError {
    match &mut error {
        BrowserAuthError::Host(message)
        | BrowserAuthError::Auth { compatibility_message: message, .. }
        | BrowserAuthError::Control { compatibility_message: message, .. } => {
            message.push_str("；"); message.push_str(cleanup);
        }
        // 回退信号不携带可追加的兼容文案；该路径在浏览器启动前返回，无收尾清理。
        BrowserAuthError::FallBackEmbedded => {}
    }
    error
}

async fn acquire_connected_browser_until_typed(
    cdp: &mut Cdp,
    gateway: &str,
    base_profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    deadline: tokio::time::Instant,
    create_settle: Duration,
    attempt: Option<&crate::school_auto_login::SchoolAutoLogin>,
) -> Result<BrowserSession, BrowserAuthError> {
    let _ = attempt; // 兼容参数不授予系统通道注入能力。
    let auth_attempt = crate::browser_auth::AuthAttempt::with_deadline(deadline);
    let dependencies = system_browser::DefaultDependencies {
        profile: base_profile,
    };
    system_browser::acquire_connected(
        cdp,
        gateway,
        base_profile,
        action,
        create_settle,
        &auth_attempt,
        &dependencies,
        &dependencies,
    )
    .await
}
