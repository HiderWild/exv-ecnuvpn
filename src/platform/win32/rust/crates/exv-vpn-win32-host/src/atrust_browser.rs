//! 用户态系统默认 Chromium 认证交接；只将网关会话交给 Engine，不读取学校密码。

use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_win32_ipc::browser_session::{
    BrowserSession, CookieSessionError, session_from_cookie_pairs,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::sync::watch;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

/// Windows 产品声明的 aTrust 客户端身份；平台名是协议字段，不按编译 target 推导。
#[must_use]
pub fn client_identity() -> AuthClientIdentity {
    AuthClientIdentity::new("SDPClient", "Windows", "zh-CN")
        .expect("static Windows client identity is valid")
}

/// 与原生认证客户端同源的 authConfig 请求路径，供两个浏览器入口共用。
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

/// 单个 EXV 窗口的登录所有者；重复点击不启动第二个认证浏览器。
#[derive(Debug, Default)]
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
    core_connect_started: bool,
    generation: u64,
}

/// 已取得网关会话但尚未完成 Core Connect 的短期 owner lease。
///
/// 认证交互期间不会把 async gate 持在此 lease 上；CoreClient 只在发送 Core Connect 前
/// 用 [`Self::begin_core_connect`] 与 Stop 取消做原子仲裁，并一直持有其独立 gate 至 RPC 收束。
#[derive(Debug)]
pub struct BrowserLoginLease {
    owner: std::sync::Arc<std::sync::Mutex<LoginState>>,
    sender: watch::Sender<LoginAction>,
    action: watch::Receiver<LoginAction>,
    generation: u64,
}

impl BrowserLoginLease {
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
        if active.generation != self.generation
            || *active.sender.borrow() == LoginAction::Cancel
        {
            return false;
        }
        active.core_connect_started = true;
        true
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
            core_connect_started: false,
            generation: expected_generation,
        });
        Ok(BrowserLoginLease {
            owner: std::sync::Arc::clone(&self.state),
            sender,
            action,
            generation: expected_generation,
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
                    !active.core_connect_started
                        && *active.sender.borrow() == LoginAction::Embedded
                })
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
        let (session, lease) = self.acquire_with_status_held(server, profile, status).await?;
        drop(lease);
        Ok(session)
    }

    /// 与 [`Self::acquire_with_status`] 相同，但把 owner lease 交给调用方，供登录完成后的
    /// Core Connect 短临界区与 Stop 取消进行原子仲裁。
    pub async fn acquire_with_status_held<F: Fn(bool)>(
        &self,
        server: &str,
        profile: &Path,
        status: F,
    ) -> Result<(BrowserSession, BrowserLoginLease), String> {
        self.acquire_with_driver_held(status, false, move |mut action| async move {
            acquire(server, profile, &mut action).await
        })
        .await
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
        let (session, lease) = self.acquire_with_driver_held(status, embedded, driver).await?;
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
        let status = LoginStatus(status);
        (status.0)(true);
        let session = driver(lease.action.clone()).await?;
        if lease.is_cancelled() {
            return Err("浏览器登录已取消".into());
        }
        Ok((session, lease))
    }
}

struct BrowserChild(Child);
impl Drop for BrowserChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    sequence: u64,
}
impl Cdp {
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            self.socket
                .send(Message::Text(request.to_string().into()))
                .await
                .map_err(|_| "浏览器调试连接已关闭".to_owned())?;
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

/// 页面未就绪或接口失败不是“未登录”，不能据此主动打开门户。
fn auth_status(reply: &Value) -> Option<bool> {
    let value = &reply["result"]["value"];
    if value["code"] != 0 {
        return None;
    }
    match &value["isLogin"] {
        Value::Bool(logged_in) => Some(*logged_in),
        Value::Number(number) if number.as_u64() == Some(1) => Some(true),
        Value::Number(number) if number.as_u64() == Some(0) => Some(false),
        _ => None,
    }
}

fn login_url(gateway: &str, reply: &Value) -> Result<String, String> {
    let path = reply["result"]["value"]["firstAuth"]
        .as_array()
        .and_then(|entries| entries.iter().find_map(Value::as_str))
        .ok_or("网关未提供浏览器认证入口")?;
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '\r', '\n']) {
        return Err("网关浏览器认证入口不是有效的本站路径".into());
    }
    Ok(format!("{gateway}{path}"))
}

fn session_from_cookies(
    gateway: &str,
    user_agent: &str,
    cookies: &Value,
) -> Result<BrowserSession, String> {
    let cookies = cookies.as_array().ok_or("浏览器 Cookie 响应格式错误")?;
    // CDP 采集适配留在宿主：按数组顺序取字符串 name/value，缺字段或非字符串条目跳过；
    // 允许集合过滤、sid 存在性与控制字符校验由 Common 共享函数给出。
    let mut pairs = Vec::new();
    for cookie in cookies {
        let (Some(name), Some(value)) = (cookie["name"].as_str(), cookie["value"].as_str()) else {
            continue;
        };
        pairs.push((name.to_owned(), value.to_owned()));
    }
    session_from_cookie_pairs(gateway, user_agent, pairs).map_err(|error| match error {
        CookieSessionError::MissingSession => "浏览器尚未取得网关会话".to_owned(),
        CookieSessionError::InvalidField => "浏览器会话字段无效".to_owned(),
        CookieSessionError::Session(_) => "浏览器会话格式无效".to_owned(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportedBrowser {
    Edge,
    Chrome,
}

impl SupportedBrowser {
    fn profile_name(self) -> &'static str {
        match self {
            Self::Edge => "edge",
            Self::Chrome => "chrome",
        }
    }

    fn executable_candidates(self) -> &'static [&'static str] {
        match self {
            Self::Edge => &["Microsoft/Edge/Application/msedge.exe"],
            Self::Chrome => &["Google/Chrome/Application/chrome.exe"],
        }
    }
}

fn browser_for_progid(progid: &str) -> Option<SupportedBrowser> {
    match progid {
        "MSEdgeHTM" => Some(SupportedBrowser::Edge),
        "ChromeHTML" | "GoogleChromeHTML" => Some(SupportedBrowser::Chrome),
        _ => None,
    }
}

/// 只读取 Windows HTTPS UserChoice，最终可执行文件仍只从已知安装目录中选择。
fn system_default_browser() -> Result<(SupportedBrowser, PathBuf), String> {
    use std::os::windows::process::CommandExt;
    let output = Command::new("reg.exe")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
            "/v",
            "ProgId",
        ])
        .creation_flags(0x08000000)
        .output()
        .map_err(|_| "无法读取系统默认浏览器设置".to_owned())?;
    if !output.status.success() {
        return Err("无法读取系统默认浏览器设置".into());
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let progid = output_text.split_whitespace().last().unwrap_or_default().trim();
    let browser = browser_for_progid(progid).ok_or_else(|| {
        "系统默认浏览器目前不受支持；请将 HTTPS 默认浏览器设为 Microsoft Edge 或 Chrome 后重试"
            .to_owned()
    })?;
    for variable in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
        if let Some(root) = std::env::var_os(variable) {
            for relative in browser.executable_candidates() {
                let path = PathBuf::from(&root).join(*relative);
                if path.is_file() {
                    return Ok((browser, path));
                }
            }
        }
    }
    Err("系统默认浏览器已识别，但未找到对应的 Edge 或 Chrome 程序".into())
}

/// 拉起 EXV 专用 Chrome 配置目录；Chrome 自身保存浏览器登录状态。
/// 不读取其他 Chrome 配置目录，不安装网卡或路由，也不调用官方客户端。
pub async fn acquire(
    server: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
) -> Result<BrowserSession, String> {
    let gateway = if server.starts_with("https://") {
        server.trim_end_matches('/').to_owned()
    } else {
        format!("https://{server}")
    };
    // 复用交接载体的 HTTPS origin 校验，只校验输入，不代表候选会话已认证。
    BrowserSession::new(
        gateway.clone(),
        "sid=validation".into(),
        "validation".into(),
    )
    .map_err(|_| "aTrust 网关地址无效".to_owned())?;
    tokio::select! {
        biased;
        () = wait_for_cancel(action) => Err("浏览器登录已取消".into()),
        result = tokio::time::timeout(Duration::from_mins(5), acquire_inner(&gateway, profile)) => {
            result.map_err(|_| "浏览器登录超时，请重试".to_owned())?
        }
    }
}

async fn wait_for_cancel(action: &mut watch::Receiver<LoginAction>) {
    while *action.borrow() != LoginAction::Cancel {
        if action.changed().await.is_err() {
            break;
        }
    }
}

async fn acquire_inner(gateway: &str, base_profile: &Path) -> Result<BrowserSession, String> {
    let (browser, executable) = system_default_browser()?;
    let browser_profile = base_profile.join(browser.profile_name());
    use std::os::windows::process::CommandExt;
    std::fs::create_dir_all(&browser_profile).map_err(|_| "无法创建 EXV 浏览器配置目录")?;
    let port_file = browser_profile.join("DevToolsActivePort");
    match std::fs::remove_file(&port_file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法准备浏览器调试入口".into()),
    }
    let mut child = BrowserChild(
        Command::new(executable)
            .arg(format!("--user-data-dir={}", browser_profile.display()))
            .args([
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|_| "无法启动系统默认浏览器")?,
    );
    let endpoint = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if child
                .0
                .try_wait()
                .map_err(|_| "无法检查浏览器状态")?
                .is_some()
            {
                return Err("系统默认浏览器启动后已退出".to_owned());
            }
            if let Ok(port_text) = std::fs::read_to_string(&port_file) {
                let mut lines = port_text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next())
                    && let Ok(port) = port.parse::<u16>()
                        && port != 0 && path.starts_with("/devtools/browser/") {
                            break Ok(format!("ws://127.0.0.1:{port}{path}"));
                        }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "等待浏览器调试入口超时")??;
    let (socket, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .map_err(|_| "无法连接浏览器调试入口")?;
    let mut cdp = Cdp {
        socket,
        sequence: 0,
    };
    let version = cdp.call("Browser.getVersion", json!({}), None).await?;
    let mut user_agent = version["userAgent"]
        .as_str()
        .ok_or("浏览器未返回标识")?
        .to_owned();
    let target = cdp
        .call("Target.createTarget", json!({"url":"about:blank"}), None)
        .await?;
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
            None,
        )
        .await?;
    let session = attached["sessionId"]
        .as_str()
        .ok_or("无法绑定登录标签页")?
        .to_owned();
    cdp.call("Network.enable", json!({}), Some(&session))
        .await?;
    if let Ok(Some(saved)) = crate::atrust_session_cache::load(base_profile, gateway) {
        let cookies: Vec<Value> = saved
            .cookie_header()
            .split(';')
            .filter_map(|field| {
                let (name, value) = field.trim().split_once('=')?;
                Some(json!({"name":name,"value":value,"url":gateway,"secure":true,"httpOnly":true}))
            })
            .collect();
        cdp.call(
            "Network.setCookies",
            json!({"cookies":cookies}),
            Some(&session),
        )
        .await?;
        user_agent = saved.user_agent().to_owned();
        cdp.call(
            "Network.setUserAgentOverride",
            json!({"userAgent":user_agent}),
            Some(&session),
        )
        .await?;
    }
    cdp.call(
        "Network.setBlockedURLs",
        json!({"urls":[
            "*://localhost.sangfor.com.cn:*/*", "atrust:*"
        ]}),
        Some(&session),
    )
    .await?;
    // 身份路径由 win32-host 共享 helper 与原生认证客户端同源构造，本入口只消费路径字节。
    let auth_path = auth_config_path(false);
    // 已保存的有效会话直接交接，避免先执行门户脚本（可能唤起官方客户端）。
    cdp.call(
        "Page.navigate",
        json!({"url":format!("{gateway}{auth_path}")}),
        Some(&session),
    )
    .await?;
    let expression = format!(
        r"(async()=>{{if(location.origin!=={})return null;try{{const r=await fetch('{auth_path}',{{credentials:'include'}});const j=await r.json();return {{code:j.code,isLogin:j.data?.isLogin,firstAuth:j.data?.firstAuth}};}}catch{{return null;}}}})()",
        serde_json::to_string(gateway).map_err(|_| "网关地址编码失败")?
    );
    let mut portal_opened = false;
    loop {
        let ready = cdp
            .call(
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
                Some(&session),
            )
            .await;
        let status = ready.as_ref().ok().and_then(auth_status);
        if status == Some(true) {
            let cookies = cdp
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway]}),
                    Some(&session),
                )
                .await?;
            let candidate = session_from_cookies(gateway, &user_agent, &cookies["cookies"])?;
            crate::atrust_session_cache::save(base_profile, &candidate)?;
            let _ = cdp.call("Browser.close", json!({}), None).await;
            // 正常退出留出 浏览器写入配置目录的时间，超时仍由 Drop 回收本次子进程。
            let _ = tokio::time::timeout(Duration::from_secs(3), async {
                while matches!(child.0.try_wait(), Ok(None)) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            return Ok(candidate);
        }
        if status == Some(false) && !portal_opened {
            crate::atrust_session_cache::forget(base_profile, gateway);
            let entry = login_url(gateway, ready.as_ref().map_err(|_| "无法读取网关认证入口")?)?;
            cdp.call("Page.navigate", json!({"url":entry}), Some(&session))
                .await?;
            portal_opened = true;
        }
        if child
            .0
            .try_wait()
            .map_err(|_| "无法检查浏览器状态")?
            .is_some()
        {
            return Err("登录窗口已关闭".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
