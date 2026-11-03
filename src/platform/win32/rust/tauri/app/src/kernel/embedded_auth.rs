use std::{collections::HashMap, time::Duration};

use exv_core::atrust_browser::LoginAction;
use exv_vpn_win32_ipc::browser_session::{
    BrowserSession, CookieSessionError, GATEWAY_COOKIE_NAMES, session_from_cookie_pairs,
};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::watch;

const PROBE_COOKIE: &str = "__exv_auth_probe";
const USER_AGENT_COOKIE: &str = "__exv_user_agent";
pub const FALLBACK_REQUESTED: &str = "__exv_system_browser_fallback_requested";
const WINDOW_TITLE_WAITING: &str = "学校统一身份认证 — 请在学校页面完成登录";
const WINDOW_TITLE_CONFIRMING: &str = "学校统一身份认证 — 正在确认登录结果";
const WINDOW_TITLE_SUCCESS: &str = "学校统一身份认证 — 登录成功，正在建立连接";
const RESULT_DISPLAY_DURATION: Duration = Duration::from_millis(700);

/// 用独立、可调整大小的 Tauri WebView 完成 aTrust 原页认证，并在本机进程内
/// 回收网关限定的候选会话。远程页面不通过 Tauri IPC 发送或接收任何认证数据。
pub async fn acquire(
    app: &AppHandle,
    server: &str,
    action: watch::Receiver<LoginAction>,
) -> Result<BrowserSession, String> {
    let started_at = std::time::Instant::now();
    let gateway = normalize_gateway(server)?;
    // authConfig 路径与原生认证客户端同源；身份由 win32-host 宿主 helper 声明。
    let auth_path = exv_core::atrust_browser::auth_config_path(false);
    let probe_url = format!("{gateway}{auth_path}");
    let profile = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位 aTrust 内嵌浏览器数据目录")?
        .join("atrust-webview");
    std::fs::create_dir_all(&profile).map_err(|_| "无法准备 aTrust 内嵌浏览器数据目录")?;

    let cache_profile = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .map(|root| root.join("EXV/atrust-browser"))
        .ok_or("无法定位 EXV 浏览器配置目录")?;
    let cached = exv_core::atrust_session_cache::load(&cache_profile, &gateway)
        .ok()
        .flatten();

    let label = format!("atrust-auth-{}", uuid::Uuid::new_v4().simple());
    let initial_url = probe_url.parse().map_err(|_| "aTrust 认证入口无效")?;
    let mut builder =
        WebviewWindowBuilder::new(app, label.clone(), WebviewUrl::External(initial_url));
    if let Some(session) = &cached {
        builder = builder.user_agent(session.user_agent());
    }
    let window = builder
    .title("学校统一身份认证")
    .inner_size(540.0, 700.0)
    .min_inner_size(380.0, 480.0)
    .resizable(true)
    .center()
    .data_directory(profile)
    // 认证可能在 SSO 域或辅助验证页打开新标签。允许 HTTPS 弹窗由 Tauri
    // WebView 创建；该标签没有匹配 main 的 capability，不能调用应用 IPC。
    .on_new_window(|url, _features| {
        if url.scheme() == "https" {
            tauri::webview::NewWindowResponse::Allow
        } else {
            tauri::webview::NewWindowResponse::Deny
        }
    })
    // aTrust scheme 会唤起官方客户端，认证窗口只承载网页步骤。
    .on_navigation(|url| url.scheme() == "https")
    .build()
    .map_err(|_| "无法打开 aTrust 内嵌登录窗口")?;
    set_window_title(&window, WINDOW_TITLE_WAITING);
    let destroy_timer = started_at;
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            log::info!(
                target: "exv.atrust_auth",
                "embedded.window_destroyed elapsed_ms={}",
                destroy_timer.elapsed().as_millis()
            );
        }
    });
    log::info!(
        target: "exv.atrust_auth",
        "embedded.window_created elapsed_ms={}",
        started_at.elapsed().as_millis()
    );

    if let Some(session) = &cached {
        if restore_cached_cookies(&window, &gateway, session)
            .await
            .is_err()
        {
            exv_core::atrust_session_cache::forget(&cache_profile, &gateway);
        }
    }

    let mut action = action;
    let mut result = tokio::select! {
        biased;
        _ = wait_for_action(&mut action) => if *action.borrow() == LoginAction::UseSystemBrowser { Err(FALLBACK_REQUESTED.to_owned()) } else { log::info!(target: "exv.atrust_auth", "embedded.cancelled"); Err("登录已取消".to_owned()) },
        result = tokio::time::timeout(
            Duration::from_secs(300),
            wait_for_session(
                app.clone(),
                label,
                window.clone(),
                &gateway,
                &auth_path,
                &cache_profile,
                started_at,
            ),
        ) => match result {
            Ok(result) => result,
            Err(_) => Err("内嵌登录超时，请重试".to_owned()),
        }
    };
    if result.is_ok() {
        set_window_title(&window, WINDOW_TITLE_SUCCESS);
        if let Err(error) = wait_for_success_display(&mut action, RESULT_DISPLAY_DURATION).await {
            result = Err(error);
        }
    }
    if result.is_ok() {
        log::info!(
            target: "exv.atrust_auth",
            "embedded.authenticated elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
    }
    match &result {
        Ok(_) => {}
        Err(error) if error == FALLBACK_REQUESTED => {
            set_window_title(&window, "学校统一身份认证 — 请在系统浏览器中继续");
        }
        Err(error) if error == "登录已取消" => {
            set_window_title(&window, "学校统一身份认证 — 登录已取消");
        }
        Err(_) => set_window_title(&window, "学校统一身份认证 — 登录未完成，请查看应用提示"),
    }
    let outcome = match &result {
        Ok(_) => "authenticated",
        Err(error) if error == FALLBACK_REQUESTED => "fallback",
        Err(error) if error == "登录已取消" => "cancelled",
        Err(error) if error == "内嵌登录超时，请重试" => "timeout",
        Err(_) => "error",
    };
    log::info!(
        target: "exv.atrust_auth",
        "embedded.result outcome={outcome} elapsed_ms={}",
        started_at.elapsed().as_millis()
    );

    match window.close() {
        Ok(()) => log::info!(
            target: "exv.atrust_auth",
            "embedded.close_requested result=success elapsed_ms={}",
            started_at.elapsed().as_millis()
        ),
        Err(_) => log::warn!(
            target: "exv.atrust_auth",
            "embedded.close_requested result=failure elapsed_ms={}",
            started_at.elapsed().as_millis()
        ),
    }
    result
}

fn set_window_title(window: &WebviewWindow, title: &str) {
    if window.set_title(title).is_err() {
        log::warn!(target: "exv.atrust_auth", "embedded.title_update_failed");
    }
}

async fn wait_for_success_display(
    action: &mut watch::Receiver<LoginAction>,
    duration: Duration,
) -> Result<(), String> {
    tokio::select! {
        biased;
        _ = wait_for_action(action) => {
            if *action.borrow() == LoginAction::UseSystemBrowser {
                Err(FALLBACK_REQUESTED.to_owned())
            } else {
                log::info!(target: "exv.atrust_auth", "embedded.cancelled");
                Err("登录已取消".to_owned())
            }
        }
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

async fn wait_for_action(action: &mut watch::Receiver<LoginAction>) {
    while *action.borrow() == LoginAction::Embedded {
        if action.changed().await.is_err() {
            break;
        }
    }
}

async fn wait_for_session(
    app: AppHandle,
    label: String,
    window: WebviewWindow,
    gateway: &str,
    auth_path: &str,
    cache_profile: &std::path::Path,
    started_at: std::time::Instant,
) -> Result<BrowserSession, String> {
    let gateway_url: tauri::Url = gateway.parse().map_err(|_| "aTrust 网关地址无效")?;
    let gateway_origin = gateway_url.origin().ascii_serialization();
    let mut login_started = false;
    let mut probe_logged = false;

    loop {
        if app.get_webview_window(&label).is_none() {
            return Err("登录窗口已关闭".into());
        }

        if window
            .url()
            .is_ok_and(|url| url.origin().ascii_serialization() == gateway_origin)
        {
            if !probe_logged {
                log::info!(target: "exv.atrust_auth", "embedded.gateway_probe");
                probe_logged = true;
            }
            // 只从当前 aTrust 原页请求公开 authConfig，再通过临时、非认证 cookie
            // 将三个需要的字段交给 Rust；不会把远程页面当成有权限的 Tauri 前端。
            window
                .eval(&auth_config_probe_script(auth_path))
                .map_err(|_| "无法读取网关认证状态")?;
            tokio::time::sleep(Duration::from_millis(400)).await;

            let cookies = read_cookies(window.clone(), gateway_url.clone()).await?;
            if let Some(state) = probe_state(&cookies) {
                delete_probe_cookie(&window, PROBE_COOKIE);
                if state.code_ok && state.logged_in {
                    set_window_title(&window, WINDOW_TITLE_CONFIRMING);
                    if !has_session_cookie(&cookies) {
                        return Err("学校已确认登录，但未取得网关会话，请重试登录".into());
                    }
                    let session =
                        build_session_from_webview(&window, gateway, gateway_url, cookies).await?;
                    let _ = exv_core::atrust_session_cache::save(cache_profile, &session);
                    log::info!(
                        target: "exv.atrust_auth",
                        "embedded.session_ready elapsed_ms={}",
                        started_at.elapsed().as_millis()
                    );
                    return Ok(session);
                }
                if state.code_ok && !state.logged_in {
                    exv_core::atrust_session_cache::forget(cache_profile, gateway);
                }
                if state.code_ok && !state.logged_in && !login_started {
                    let login_url = login_url(gateway, &state.first_auth)?;
                    window
                        .navigate(login_url.parse().map_err(|_| "学校认证入口无效")?)
                        .map_err(|_| "无法打开学校认证入口")?;
                    log::info!(
                        target: "exv.atrust_auth",
                        "embedded.login_navigation elapsed_ms={}",
                        started_at.elapsed().as_millis()
                    );
                    login_started = true;
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn cached_cookie_pairs(cookie_header: &str) -> Vec<(String, String)> {
    cookie_header
        .split(';')
        .filter_map(|field| {
            let (name, value) = field.trim().split_once('=')?;
            GATEWAY_COOKIE_NAMES
                .contains(&name)
                .then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

async fn restore_cached_cookies(
    window: &WebviewWindow,
    gateway: &str,
    session: &BrowserSession,
) -> Result<(), String> {
    let host = tauri::Url::parse(gateway)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or("aTrust 网关地址无效")?;
    let cookies = cached_cookie_pairs(session.cookie_header());
    if !cookies
        .iter()
        .any(|(name, value)| name == "sid" && !value.is_empty())
    {
        return Err("加密会话缓存缺少网关会话".into());
    }

    let dispatcher = window.clone();
    let webview_window = window.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher
        .run_on_main_thread(move || {
            let result = cookies.into_iter().try_for_each(|(name, value)| {
                let cookie = tauri::webview::Cookie::build((name, value))
                    .domain(host.clone())
                    .path("/")
                    .secure(true)
                    .http_only(true)
                    .build();
                webview_window
                    .set_cookie(cookie)
                    .map_err(|_| "无法恢复加密的 aTrust 会话".to_owned())
            });
            let _ = tx.send(result);
        })
        .map_err(|_| "无法恢复加密的 aTrust 会话".to_owned())?;
    rx.await
        .map_err(|_| "无法恢复加密的 aTrust 会话".to_owned())?
}

async fn build_session_from_webview(
    window: &WebviewWindow,
    gateway: &str,
    gateway_url: tauri::Url,
    mut cookies: Vec<(String, String)>,
) -> Result<BrowserSession, String> {
    window
        .eval(&user_agent_probe_script())
        .map_err(|_| "无法读取内嵌浏览器标识")?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let fresh = read_cookies(window.clone(), gateway_url).await?;
    cookies.extend(fresh);
    delete_probe_cookie(window, USER_AGENT_COOKIE);

    let mut values: HashMap<String, String> = cookies.into_iter().collect();
    let user_agent = values
        .remove(USER_AGENT_COOKIE)
        .and_then(|value| decode_hex(&value))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|value| !value.is_empty())
        .ok_or("无法读取内嵌浏览器标识")?;
    // 内嵌 WebView 的合并/去重与按允许集合排序仍在宿主；纯规则交给 Common 共享函数。
    let fields = GATEWAY_COOKIE_NAMES
        .iter()
        .filter_map(|name| {
            values
                .remove(*name)
                .map(|value| ((*name).to_owned(), value))
        })
        .collect::<Vec<_>>();
    session_from_cookie_pairs(gateway, &user_agent, fields).map_err(|error| match error {
        CookieSessionError::MissingSession => "学校认证尚未取得网关会话".to_owned(),
        CookieSessionError::InvalidField => "内嵌浏览器会话字段无效".to_owned(),
        CookieSessionError::Session(_) => "内嵌浏览器会话格式无效".to_owned(),
    })
}

fn normalize_gateway(server: &str) -> Result<String, String> {
    let gateway = if server.starts_with("https://") {
        server.trim_end_matches('/').to_owned()
    } else {
        format!("https://{}", server.trim_matches('/'))
    };
    BrowserSession::new(
        gateway.clone(),
        "sid=validation".into(),
        "validation".into(),
    )
    .map_err(|_| "aTrust 网关地址无效".to_owned())?;
    Ok(gateway)
}

fn login_url(gateway: &str, first_auth: &str) -> Result<String, String> {
    if !first_auth.starts_with('/')
        || first_auth.starts_with("//")
        || first_auth.contains(['\\', '\r', '\n'])
    {
        return Err("网关返回了无效的认证入口".into());
    }
    Ok(format!("{}{}", gateway.trim_end_matches('/'), first_auth))
}

fn auth_config_probe_script(auth_path: &str) -> String {
    let path = serde_json::to_string(auth_path).expect("static URL serializes");
    let cookie = serde_json::to_string(PROBE_COOKIE).expect("static cookie name serializes");
    format!(
        "void ({})({path},{cookie});",
        include_str!("auth_config_probe.js")
    )
}

fn user_agent_probe_script() -> String {
    format!(
        r#"(()=>{{const hex=s=>Array.from(new TextEncoder().encode(s)).map(b=>b.toString(16).padStart(2,'0')).join('');document.cookie='{USER_AGENT_COOKIE}='+hex(navigator.userAgent)+'; Path=/; Secure; SameSite=Lax';}})()"#
    )
}

fn probe_state(cookies: &[(String, String)]) -> Option<AuthProbe> {
    let value = cookies
        .iter()
        .find(|(name, _)| name == PROBE_COOKIE)
        .map(|(_, value)| value)?;
    let mut fields = value.splitn(3, '-');
    let code_ok = fields.next()? == "1";
    let logged_in = fields.next()? == "1";
    let first_auth = String::from_utf8(decode_hex(fields.next()?)?).ok()?;
    Some(AuthProbe {
        code_ok,
        logged_in,
        first_auth,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct AuthProbe {
    code_ok: bool,
    logged_in: bool,
    first_auth: String,
}

async fn read_cookies(
    window: WebviewWindow,
    gateway: tauri::Url,
) -> Result<Vec<(String, String)>, String> {
    tokio::task::spawn_blocking(move || {
        window
            .cookies_for_url(gateway)
            .map(|cookies| {
                cookies
                    .into_iter()
                    .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
                    .collect()
            })
            .map_err(|_| "无法读取内嵌登录会话".to_owned())
    })
    .await
    .map_err(|_| "无法读取内嵌登录会话".to_owned())?
}

fn delete_probe_cookie(window: &WebviewWindow, name: &str) {
    let _ = window.eval(&format!(
        "document.cookie='{name}=; Max-Age=0; Path=/; Secure; SameSite=Lax'"
    ));
}

fn has_session_cookie(cookies: &[(String, String)]) -> bool {
    cookies
        .iter()
        .any(|(name, value)| name == "sid" && !value.is_empty())
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}
