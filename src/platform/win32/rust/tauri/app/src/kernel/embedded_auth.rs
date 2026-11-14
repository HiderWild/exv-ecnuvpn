use exv_core::atrust_browser::BrowserAuthError;
use std::{collections::HashMap, time::Duration};

use exv_core::atrust_browser::{AUTH_STALL_ROUNDS, LoginAction};
use exv_core::browser_auth::page_facts::{percent_decode_component, url_is_auth_config_path};
use exv_vpn_win32_ipc::browser_session::{
    BrowserSession, CookieSessionError, GATEWAY_COOKIE_NAMES, session_from_cookie_pairs,
};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::watch;

mod owned_webview;

/// 仅采集有限类别；每个新文档分别保留首个同步异常与当前未处理拒绝，资源失败仅作备用。
/// 不回传 URL、源码、堆栈或消息原文。
pub(super) const PAGE_STARTUP_SCRIPT: &str = r#"(()=>{
try {
if (window.__exvAuthStartupSample) return;
let scriptError=null,resourceError=null;
const rejections=new Map();
const phase=()=>['loading','interactive','complete'].includes(document.readyState)?document.readyState:'unknown';
const observation=(error,message,fallback)=>{
let kind=fallback,hasMessage=false,source="unknown";
try {
const name=error&&error.name;
if(fallback==='script_error'&&['SyntaxError','ReferenceError','TypeError','RangeError','EvalError','URIError'].includes(name)) kind=name;
hasMessage=!!(message||(error&&error.message)||(typeof error==='string'&&error));
if(window.__exvIsOwnedLocalApiFailure&&window.__exvIsOwnedLocalApiFailure(error)) source='exv_local_api';
} catch(_) {}
return {kind,hasMessage,source,phase:phase()};
};
addEventListener('error',(event)=>{
if(event.target===window) {
if(!scriptError) scriptError=observation(event.error,event.message,'script_error');
} else if(event.target&&event.target.tagName==='SCRIPT'&&!resourceError) {
resourceError=observation(null,event.message,'resource_error');
}
},true);
addEventListener('unhandledrejection',(event)=>{
if(!rejections.has(event.promise)) rejections.set(event.promise,observation(event.reason,null,'promise_rejection'));
});
addEventListener('rejectionhandled',(event)=>rejections.delete(event.promise));
window.__exvAuthStartupSample=()=>{
const first=scriptError||rejections.values().next().value||resourceError;
let loading='unknown',code='none';
try {
const e=document.querySelector('#preContainer, #loading, #loading-mask, .loading, .loading-page, .el-loading-mask');
const style=e&&getComputedStyle(e);
loading=e&&e.getClientRects().length&&style.display!=='none'&&style.visibility!=='hidden'?'1':'0';
} catch(_) {}
try {
const info=JSON.parse(new URLSearchParams(location.search).get('err_info')||'null');
const raw=info&&(info.errorCode??info.code);
if((typeof raw==='number'||typeof raw==='string')&&/^-?[0-9]{1,10}$/.test(String(raw))) {
const n=Number(raw);if(Number.isSafeInteger(n)&&n>=-2147483648&&n<=2147483647) code=String(n);
}
} catch(_) {}
return ['v2',phase(),loading,first?first.kind:'none',first&&first.hasMessage?'1':'0',first?first.phase:'none',code,first?first.source:'none'].join('|');
};
} catch(_) {}
})()"#;

/// 页面可改写回传值，宿主在日志边界再次只接受有限枚举及 i32 数字，不记录自由文本。
pub(super) fn startup_diagnostic(payload: &str) -> Option<String> {
    let fields: Vec<_> = payload.split('|').collect();
    if !((fields.len() == 7 && fields[0] == "v1")
        || (fields.len() == 8 && fields[0] == "v2" && ["none", "unknown", "exv_local_api"].contains(&fields[7])))
        || !["loading", "interactive", "complete", "unknown"].contains(&fields[1])
        || !["0", "1", "unknown"].contains(&fields[2])
        || ![
            "none",
            "SyntaxError",
            "ReferenceError",
            "TypeError",
            "RangeError",
            "EvalError",
            "URIError",
            "script_error",
            "resource_error",
            "promise_rejection",
        ]
        .contains(&fields[3])
        || !["0", "1"].contains(&fields[4])
        || !["none", "loading", "interactive", "complete", "unknown"].contains(&fields[5])
    {
        return None;
    }
    let code = if fields[6] == "none" {
        "none".to_owned()
    } else {
        fields[6].parse::<i32>().ok()?.to_string()
    };
    let mut diagnostic = format!(
        "phase={} loading_visible={} first_error={} has_message={} error_phase={} gateway_code={}",
        fields[1], fields[2], fields[3], fields[4], fields[5], code
    );
    if fields.len() == 8 {
        diagnostic.push_str(&format!(" error_source={}", fields[7]));
    }
    Some(diagnostic)
}

pub const FALLBACK_REQUESTED: &str = "__exv_system_browser_fallback_requested";
const WINDOW_TITLE_WAITING: &str = "学校统一身份认证 — 请在学校页面完成登录";
const WINDOW_TITLE_CONFIRMING: &str = "学校统一身份认证 — 正在确认登录结果";
const WINDOW_TITLE_SUCCESS: &str = "学校统一身份认证 — 登录成功，正在建立连接";
/// 仅诊断：连续无探针结果轮次达到该值时告警一次；不改变空转行为。
const PROBE_IDLE_WARN_ROUNDS: u32 = AUTH_STALL_ROUNDS;
/// P1（§7.10-1 方案 B）：内嵌窗口的 Chromium 浏览器参数，用于对抗 WebView2
/// `SetIsVisible(false)` 把隐藏窗口判为后台页而触发的后台节流/挂起。
///
/// Tauri `additional_browser_args` 会**整体替换** wry 默认值（wry
/// `webview2/mod.rs:293` 的 `unwrap_or_else`），故必须显式带上 wry 默认三项
/// `msWebOOUI,msPdfOOUI,msSmartScreenProtection`；再追加
/// `CalculateNativeWinOcclusion` 关闭窗口遮挡计算（隐藏/被遮挡窗口被判后台的根因
/// 之一），以及三项禁用后台节流/后台化的开关。窗口仍保持 `.visible(false)`。
/// 若本方案验证仍不成立，则回退到 §7.8 选项 A（可见窗口 + 自研启动页）。
const EMBEDDED_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion --disable-background-timer-throttling --disable-backgrounding-occluded-windows --disable-renderer-backgrounding";
/// S5：纯函数——在既有参数基串上按需追加 CDP 调试端口，既有参数一字不动。
/// 端口仅接受非空纯 ASCII 数字，其余（未设置/空串/非法值）一律忽略，避免命令行注入。
#[cfg(any(debug_assertions, test))]
fn build_embedded_browser_args(base: &str, cdp_port: Option<&str>) -> String {
    match cdp_port {
        Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{base} --remote-debugging-port={port}")
        }
        _ => base.to_owned(),
    }
}

/// S5：debug 专用测试钩子——读 `EXV_EMBEDDED_CDP_PORT`，非空纯数字时追加
/// `--remote-debugging-port=<port>`，供外部 CDP 客户端附加到内嵌 WebView2 做端到端测试。
/// release 走下方 `#[cfg(not(debug_assertions))]` 版本，物理上不含 env 读取与调试端口。
#[cfg(debug_assertions)]
fn embedded_browser_args() -> String {
    build_embedded_browser_args(
        EMBEDDED_BROWSER_ARGS,
        std::env::var("EXV_EMBEDDED_CDP_PORT").ok().as_deref(),
    )
}

/// S5：release 不含任何测试钩子，原样返回既有常量。
#[cfg(not(debug_assertions))]
fn embedded_browser_args() -> String {
    EMBEDDED_BROWSER_ARGS.to_owned()
}
/// 内嵌窗口页面分类回传 cookie（一次性；读取后即删）。
const PAGE_CLASS_COOKIE: &str = "__exv_page_class";
const PAGE_STARTUP_COOKIE: &str = "__exv_page_startup";
/// 内嵌页面分类脚本：只读取主框架可见文本与门户标记元素，结果经 cookie 回传。
/// cookie 值为八字段，`+` 分隔；最后的当前可见挑战与正文在同一次脚本中采集。
/// `marker+shortcut+download+hasFail+len+errText+head+challenge`；摘录字段为
/// `encodeURIComponent` 输出（该编码永不产出字面 `+`，切分无歧义）：
/// - `errText`：`#errorData` **非空 innerText** 的摘录（≤60 字符）——该元素在每个
///   shortcut 页恒存在，只有文本非空才是失败事实（服务端 message 原文）；
/// - `head`：正文足够短（≤60 字符）时才携带的正文摘录，供失败短语判定与长度
///   护栏共用同一上限；长正文（正常门户页）不携带，物理上不可能命中失败短语。
/// 判定规则留在 Rust 侧（短语表与长度上限单源）。
const PAGE_FACTS_SCRIPT: &str = "(()=>{const b=document.body;const raw=(b&&typeof b.innerText==='string')?b.innerText:'';const t=raw.trim();const marker=(document.querySelector&&document.querySelector('.down-client-pc__body'))?'1':'0';const shortcut=(document.querySelector&&(document.querySelector('#shortcut-page')||document.querySelector('#errorData')))?'1':'0';const download=t.indexOf('下载客户端')!==-1?'1':'0';const hasFail=t.indexOf('失败')!==-1?'1':'0';const len=Math.min(t.length,99);const e=(document.querySelector&&document.querySelector('#errorData'));const et=(e&&typeof e.innerText==='string')?e.innerText.trim():'';const errText=encodeURIComponent(et.slice(0,60));const head=(t.length<=60)?encodeURIComponent(t.slice(0,60)):'';const challenge=location.origin==='https://sso.ecnu.edu.cn'&&(Array.from(document.querySelectorAll('[id*=\"captcha\" i], [class*=\"captcha\" i]')).some((el)=>el.tagName!=='INPUT'&&el.offsetParent!==null)||Array.from(document.querySelectorAll('iframe')).some((frame)=>frame.offsetParent!==null&&/captcha/i.test(frame.src||'')));document.cookie='__exv_page_class='+marker+'+'+shortcut+'+'+download+'+'+hasFail+'+'+len+'+'+errText+'+'+head+'+'+(challenge?'1':'0')+'; Path=/; Secure; SameSite=Lax';})()";
/// 认证开始前的门户残留 localStorage 预清理（2026-10-07 终端页研究报告 §5.6-3 /
/// §四 R3/R4/R5：检测缓存与门户分支状态跨轮残留是同一次登录随机落页的燃料）。
/// 在网关 origin 页面上执行（localStorage 按 origin 隔离）；脚本整体 try/catch、
/// 逐键 try/catch 包裹——清理失败仅记日志，绝不影响认证流程（fail-safe）。
const PORTAL_STORAGE_CLEANUP_SCRIPT: &str = "(()=>{try{const keys=['agent','online','clientLogin','stopNotifyDownClient','readyOpenDefaultUrl'];for(let i=0;i<keys.length;i++){try{localStorage.removeItem(keys[i])}catch(e){}}}catch(e){}})()";

const SESSION_POLL_TICK: Duration = Duration::from_secs(1);
/// 仅诊断：连续无探针结果的时长达到该值时告警一次；不改变空转行为。
/// 原 `PROBE_IDLE_WARN_ROUNDS`(30) × 1s/轮 ≈ 30s。跨 origin 停滞同用该量级
///（既有代码 foreign_origin 与 probe_idle 共用 30 轮预算）。
const PROBE_IDLE_WARN_DURATION: Duration = Duration::from_secs(PROBE_IDLE_WARN_ROUNDS as u64);

/// §7.9：自动登录实时阶段（既有 `exv://browser-login` 载荷上的可选 `stage` 值）。
/// 仅新增可观测点回传，不改变 P1/P2/P3 逻辑与错误码，也不新增事件通道。
const STAGE_OPENING: &str = "opening";
const STAGE_SCHOOL_LOGIN: &str = "school_login";
const STAGE_SUBMITTING: &str = "submitting";
const STAGE_GATEWAY_CONFIRMING: &str = "gateway_confirming";
const STAGE_AWAITING_MANUAL: &str = "awaiting_manual";

/// §7.9：阶段仅在变化时回传一次，避免刷屏；代际沿用发起本次认证的 owner 代际。
/// 用独立、可调整大小的 Tauri WebView 完成 aTrust 原页认证，并在本机进程内
/// 回收网关限定的候选会话。远程页面不通过 Tauri IPC 发送或接收任何认证数据。
///
/// `school_auto_login`（T4c，2026-09-30 用户授权）：窗口到达已确认学校登录页且
/// 预算可用时自动填交一次；挑战/异常即停，页面留给人工。凭据只进 eval 脚本，
/// 不经 Tauri IPC、不落日志。
///
/// `login`（修复 1 干净收尾，2026-10-08 UI 重试脱同步）：本次认证所属的登录
/// owner。窗口 Destroyed（等待期用户关窗 / 失败后残留窗口被用户关闭）走取消
/// 收尾路径——`cancel_if_still_owner` 带代际守卫，迟到销毁不误杀新尝试。
/// X4 在同一次编排中先尝试零窗 HTTP，再静默打开隐藏 Owned；原 owner/attempt 不变。
pub(super) async fn acquire_channels(
    app: &AppHandle,
    server: &str,
    generation: u64,
    mut action: watch::Receiver<LoginAction>,
    school_auto_login: Option<&exv_core::school_auto_login::SchoolAutoLogin>,
    login: &crate::kernel::connection_request::ConnectionRequests,
    attempt: &exv_core::browser_auth::AuthAttempt,
    http_first: bool,
    intent: exv_core::browser_auth::AuthIntent,
    interaction: std::sync::Arc<std::sync::atomic::AtomicBool>,
    profiles: Option<&exv_core::browser_cache_profile::BrowserDataDirs>,
    silent: bool,
) -> Result<BrowserSession, BrowserAuthError> {
    use exv_core::browser_auth::*;
    let _ = attempt.remaining_budget(Duration::from_secs(300));
    let gateway = normalize_gateway(server)?;
    let started = std::time::Instant::now();
    log::info!(target: "exv.atrust_auth", "embedded.begin owner={} round=1 channel={} credentials_available={}", generation, if http_first { "http_then_owned" } else { "owned_webview" }, school_auto_login.is_some());
    let mut adapter =
        owned_webview::OwnedWebViewAdapter::prepare_with_profiles(app, gateway.clone(), generation, login, intent, profiles)?;
    adapter.interaction = interaction;
    let deps = owned_webview::HostDependencies {
        app: app.clone(),
        generation,
        label: adapter.snapshot_label.clone(),
        user_agent: adapter.snapshot_user_agent.clone(),
        cache_profile: adapter.cache_profile.clone(),
        intent,
        silent,
    };
    let mut http = HttpProbeAdapter::new(adapter.cache_profile.clone());
    let mut config = AuthConfig::new(
        gateway,
        adapter.user_agent.clone(),
        adapter.auth_path.clone(),
    );
    config.gateway_page_recovery = true;
    config.allow_interaction = !silent;
    if !intent.allows_reuse() { config.initial_presentation = PresentationIntent::Visible; }
    struct MissingCredentials;
    impl CredentialProvider for MissingCredentials {
        fn delivery(&self, _: &str) -> Option<CredentialDelivery> {
            None
        }
    }
    let missing = MissingCredentials;
    let credentials = if intent.allows_reuse() {
        school_auto_login.map(|source| source as &dyn CredentialProvider)
            .or(Some(&missing as &dyn CredentialProvider))
    } else { None };
    let orchestrator = AuthOrchestrator {
        config,
        events: &deps,
        snapshots: &deps,
        cache: &deps,
        credentials,
        attempt,
    };
    let result = drive_channels(&orchestrator, &mut http, &mut adapter, &mut action, http_first, intent).await;
    let result = match result {
        Ok(AcquisitionOutcome::Ready(artifact)) => Ok(artifact.session),
        Ok(AcquisitionOutcome::Control(AuthControl::UseSystemBrowser)) => Err(
            BrowserAuthError::control(AuthControl::UseSystemBrowser, FALLBACK_REQUESTED),
        ),
        Ok(AcquisitionOutcome::Control(control)) => {
            Err(BrowserAuthError::control(control, "登录已取消"))
        }
        Ok(AcquisitionOutcome::Interactive(_)) => Err(BrowserAuthError::Host(
            "学校已确认登录，但未取得网关会话，请重试登录".to_owned(),
        )),
        Err(error) => {
            let message = embedded_error_message(&error);
            Err(BrowserAuthError::auth(error, message))
        }
    };
    let result = if intent == AuthIntent::FreshManual {
        match tokio::time::timeout(Duration::from_secs(2), adapter.close()).await {
            Ok(Ok(())) => result,
            cleanup => {
                let cause = match cleanup {
                    Ok(Err(error)) => error,
                    _ => exv_core::browser_auth::AuthError { code: AuthErrorCode::ChannelTransportFailed, operation: AuthOperation::Close, native_code: None, server_detail: None, failure_kind: None },
                };
                deps.failure(&cause);
                finish_manual_cleanup(result, cause, adapter.context_released())
            }
        }
    } else { result };
    // 引擎已收束本轮窗口；若原生清理失败仍保留窗口，仅更新当前轮次标题。
    if let Err(error) = &result
        && let Some(window) =
            app.get_webview_window(&deps.label.lock().unwrap_or_else(|e| e.into_inner()))
    {
        set_window_title(
            &window,
            if matches!(
                error,
                BrowserAuthError::Control {
                    control: AuthControl::UseSystemBrowser,
                    ..
                }
            ) {
                "学校统一身份认证 — 请在系统浏览器中继续"
            } else if matches!(
                error,
                BrowserAuthError::Control {
                    control: AuthControl::Cancel,
                    ..
                }
            ) {
                "学校统一身份认证 — 登录已取消"
            } else {
                "学校统一身份认证 — 登录未完成，请查看应用提示"
            },
        );
    }
    let outcome = match &result {
        Ok(_) => "authenticated",
        Err(BrowserAuthError::Control {
            control: AuthControl::UseSystemBrowser,
            ..
        }) => "fallback",
        Err(BrowserAuthError::Control {
            control: AuthControl::Cancel,
            ..
        }) => "cancelled",
        Err(BrowserAuthError::Auth { error, .. })
            if error.code == AuthErrorCode::AcquisitionTimeout =>
        {
            "timeout"
        }
        _ => "error",
    };
    log::info!(target: "exv.atrust_auth", "embedded.result owner={} round={:?} outcome={} elapsed_ms={}", generation, attempt.round(), outcome, started.elapsed().as_millis());
    result
}

/// 关闭失败影响渠道交接；已销毁窗口只保留诊断，不新增认证门禁。
/// S4c（2026-10-26）：手动通道不再有随尝试销毁的临时目录，收尾只关窗口；
/// 持久 manual-persistent 目录跨次登录保留。
pub(super) fn finish_manual_cleanup(
    result: Result<BrowserSession, BrowserAuthError>,
    cause: exv_core::browser_auth::AuthError,
    context_released: bool,
) -> Result<BrowserSession, BrowserAuthError> {
    use exv_core::browser_auth::AuthControl;
    if context_released {
        return result;
    }
    match result {
        Err(original @ BrowserAuthError::Control { control: AuthControl::Cancel, .. }) => Err(original),
        Ok(_) | Err(BrowserAuthError::Control { control: AuthControl::UseSystemBrowser, .. }) => {
            Err(BrowserAuthError::auth(cause, "本次认证窗口关闭未完成，请重试".into()))
        }
        Err(mut original) => {
            match &mut original {
                BrowserAuthError::Host(message)
                | BrowserAuthError::Auth { compatibility_message: message, .. }
                | BrowserAuthError::Control { compatibility_message: message, .. } => {
                    message.push_str("；本次认证窗口关闭未完成");
                }
                // B2 回退信号无可追加的兼容文案；壳层在浏览器启动前改派内嵌。
                BrowserAuthError::FallBackEmbedded => {}
            }
            Err(original)
        }
    }
}

fn owner_control(action: LoginAction) -> Option<exv_core::browser_auth::AuthControl> {
    use exv_core::browser_auth::AuthControl;
    match action {
        LoginAction::Embedded => None,
        LoginAction::UseSystemBrowser => Some(AuthControl::UseSystemBrowser),
        LoginAction::External | LoginAction::Cancel => Some(AuthControl::Cancel),
    }
}

fn embedded_error_message(error: &exv_core::browser_auth::AuthError) -> String {
    use exv_core::browser_auth::{AuthErrorCode as Code, AuthOperation};
    match error.code {
        Code::ClientInstallRequired | Code::FailedPage | Code::GatewayThrottled | Code::Stalled => {
            error.legacy_message()
        }
        Code::AcquisitionTimeout => "内嵌登录超时，请重试".into(),
        Code::ChannelOpenFailed => "无法打开 aTrust 内嵌浏览器".into(),
        Code::ChannelTransportFailed if error.operation == AuthOperation::Observe => {
            "登录窗口已关闭".into()
        }
        Code::ChannelTransportFailed if error.operation == AuthOperation::Navigate => {
            "无法打开学校认证入口".into()
        }
        Code::ChannelTransportFailed => "无法读取内嵌登录会话".into(),
        Code::LoginEntryInvalid => "学校认证入口无效".into(),
        Code::SessionMissing | Code::SessionInvalid => {
            "学校已确认登录，但未取得网关会话，请重试登录".into()
        }
        _ => error.legacy_message(),
    }
}

/// 杠杆 2（提速研究 2026-10-07 §二 杠杆 2）纯决策：建窗初始页是否直达学校认证
/// 入口（`{gateway}{firstAuth}`）。
///
/// 条件（全部满足才直达）：会话缓存为冷（`cache_present=false`，冷缓存 jar 为空、
/// 登录态未知无意义，cookie 头为空即可代表窗口首轮探针），宿主直连预探针明示
/// `code_ok && !is_login`，且 `firstAuth` 是合法的本站路径（复用 [`login_url`]
/// 校验）。其余一切情形（缓存命中需先恢复 cookie 再探针、探针失败登录态未知、
/// 已登录、入口缺失/非法）一律保持 authConfig 初始页，与既有行为逐字节一致。
///
/// 初始 authConfig 页被核实**不参与**任何检测判据（研究报告 §二 杠杆 2 排查
/// 1–4）：网关 cookie 由 CAS 302 链自行播种；authConfig 路径被失败页判定显式排除
/// （`url_is_auth_config_path`）；FIX-1 重导航只复用 `probe_url` 字符串、不依赖
/// 初始导航发生过；缓存命中路径从不读页面内容。跳过它只省时间，不削弱错误识别。
fn direct_login_target(
    cache_present: bool,
    probe: &Result<
        exv_core::atrust_session_probe::SessionProbe,
        exv_core::atrust_session_probe::ProbeError,
    >,
    gateway: &str,
) -> Option<String> {
    if cache_present {
        return None;
    }
    let state = probe.as_ref().ok()?;
    // 探针 Ok 时 code_ok 恒为 true（SessionProbe 文档）；仍显式校验以钉死条件。
    if !state.code_ok || state.is_login {
        return None;
    }
    let first_auth = state.first_auth.as_deref().unwrap_or_default();
    login_url(gateway, first_auth).ok()
}

fn set_window_title(window: &WebviewWindow, title: &str) {
    if window.set_title(title).is_err() {
        log::warn!(target: "exv.atrust_auth", "embedded.title_update_failed");
    }
}

/// 就地记一次表单登录（setup 未 manage 计数器时只告警，不影响认证流程）。
/// 日志只含窗口内总数，不含任何秘密。
pub(super) fn record_form_login_once(app: &AppHandle) {
    match app.try_state::<super::login_attempt_tracker::LoginAttemptTracker>() {
        Some(tracker) => match tracker.record_form_login(std::time::SystemTime::now()) {
            Ok(total_in_window) => log::info!(
                target: "exv.atrust_auth",
                "embedded.form_login_counted total_in_window={total_in_window}"
            ),
            Err(error) => log::warn!(
                target: "exv.atrust_auth",
                "embedded.form_login_counted tracker_error error={error}"
            ),
        },
        None => log::warn!(
            target: "exv.atrust_auth",
            "embedded.form_login_counted tracker=unavailable"
        ),
    }
}

/// 内嵌页面分类的原始事实（cookie：`marker+shortcut+download+hasFail+len+errText+head+challenge`）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageFacts {
    challenge_visible: bool,
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

/// 解码摘录字段：percent 解码后为空（元素缺失/正文过长未携带）归一为 `None`。
fn decode_excerpt_field(field: &str) -> Option<String> {
    let decoded = percent_decode_component(field);
    (!decoded.trim().is_empty()).then_some(decoded)
}

fn parse_page_facts(value: &str) -> Option<PageFacts> {
    let mut fields = value.splitn(8, '+');
    let marker = fields.next()? == "1";
    let shortcut = fields.next()? == "1";
    let download = fields.next()? == "1";
    let has_fail = fields.next()? == "1";
    let len = fields.next()?.parse::<usize>().ok()?;
    let error_text = decode_excerpt_field(fields.next()?);
    let body_head = decode_excerpt_field(fields.next()?);
    Some(PageFacts {
        challenge_visible: fields.next() == Some("1"),
        marker,
        shortcut,
        download,
        has_fail,
        len,
        error_text,
        body_head,
    })
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

/// 用建窗时显式设定的 UA 与 Cookie API 读到的网关 Cookie 组装候选会话。
///
/// 方案 A：不再经页面回读 `navigator.userAgent`；UA 全程复用建窗取值，保证探针、会话缓存
/// 与引擎协议 UA 三者同源。
fn build_session_from_webview(
    gateway: &str,
    cookies: Vec<(String, String)>,
    user_agent: &str,
) -> Result<BrowserSession, String> {
    // 内嵌 WebView 的合并/去重与按允许集合排序仍在宿主；纯规则交给 Common 共享函数。
    let mut values: HashMap<String, String> = cookies.into_iter().collect();
    let fields = GATEWAY_COOKIE_NAMES
        .iter()
        .filter_map(|name| {
            values
                .remove(*name)
                .map(|value| ((*name).to_owned(), value))
        })
        .collect::<Vec<_>>();
    session_from_cookie_pairs(gateway, user_agent, fields).map_err(|error| match error {
        CookieSessionError::MissingSession => "学校认证尚未取得网关会话".to_owned(),
        CookieSessionError::InvalidField => "内嵌浏览器会话字段无效".to_owned(),
        CookieSessionError::Session(_) => "内嵌浏览器会话格式无效".to_owned(),
    })
}

/// 两个网关字符串是否同一 HTTPS origin：比对解析后的 origin，容忍尾斜杠/大小写
/// 差异；任一不可解析即视为不同源——不同网关的会话材料不得复用。
fn same_gateway_origin(left: &str, right: &str) -> bool {
    match (left.parse::<tauri::Url>(), right.parse::<tauri::Url>()) {
        (Ok(left), Ok(right)) => {
            left.origin().ascii_serialization() == right.origin().ascii_serialization()
        }
        _ => false,
    }
}

/// S3：由 agent 仿真的会话快照构造 [`BrowserSession`]：先校验快照网关与当前网关
/// 同源，再复用既有 Cookie 头解析（允许集合过滤 + `sid` 存在性 + 控制字符校验）；
/// 网关不匹配或材料不合法返回 `None`。构造规则与 WebView 路径共用同一套共享函数，
/// 不另立第二份实现。
///
/// S7 阶段 C：无头线（headless_auth）经 `pub(crate)` 复用同一构造器组装
/// `SessionExchange` 会话集——Cookie 头解析、允许集合与校验不得出现第二份实现。
pub(crate) fn build_session_from_snapshot(
    gateway: &str,
    user_agent: &str,
    snapshot_gateway: &str,
    snapshot_cookie_header: &str,
) -> Option<BrowserSession> {
    if !same_gateway_origin(snapshot_gateway, gateway) {
        return None;
    }
    session_from_cookie_pairs(
        gateway,
        user_agent,
        cached_cookie_pairs(snapshot_cookie_header),
    )
    .ok()
}

/// 由 Cookie API 读到的 `(name, value)` 拼 Cookie 请求头，供宿主直连探针使用。
///
/// 只取 [`GATEWAY_COOKIE_NAMES`] 允许集合，避免把无关 cookie（含非法字符）带入探针请求；
/// 返回值**绝不落日志**。
fn cookie_header(cookies: &[(String, String)]) -> String {
    GATEWAY_COOKIE_NAMES
        .iter()
        .filter_map(|name| {
            cookies
                .iter()
                .find(|(cookie_name, _)| cookie_name == name)
                .map(|(_, value)| format!("{name}={value}"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// S7 阶段 C：headless/embedded 的 acquire_channels 复用同一网关规范化——会话缓存与 SSO 档案都
/// 按规范化网关字符串作键，两条线必须逐字节同键。
pub(crate) fn normalize_gateway(server: &str) -> Result<String, String> {
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

/// 网关认证入口 URL（`firstAuth` 以网关下发为准；C4 原生系统浏览器线复用同一
/// 形状校验，不另立第二份实现）。
pub(crate) fn login_url(gateway: &str, first_auth: &str) -> Result<String, String> {
    if !first_auth.starts_with('/')
        || first_auth.starts_with("//")
        || first_auth.contains(['\\', '\r', '\n'])
    {
        return Err("网关返回了无效的认证入口".into());
    }
    Ok(format!("{}{}", gateway.trim_end_matches('/'), first_auth))
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

/// 仅诊断：从网关 cookie 提取名称集合，排序去重后以逗号拼接；绝不记录 cookie 值。
fn gateway_cookie_names(cookies: &[(String, String)]) -> String {
    let mut names: Vec<&str> = cookies.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(",")
    }
}

/// [`export_sso_cookies`] 的导出结果（仅供日志与测试断言；只含 cookie 名与计数，
/// 不含任何 cookie/deviceId 值）。
#[derive(Debug, PartialEq, Eq)]
enum SsoExportOutcome {
    /// 已写入档案：落盘后的 cookie 名全集（含既有保留项）、`deviceId` 是否为本次
    /// 新生成、因字符非法被跳过的名值对数。
    Saved {
        names: Vec<String>,
        fresh_device: bool,
        skipped_invalid: usize,
    },
    /// 未读到任何 SSO 域 cookie：跳过写入——绝不能用空集清掉既有可用档案（缓存
    /// 命中直接成功的路径可能全程未访问 SSO 域）。
    NoCookies,
}

/// S7 阶段 B（交付物 5）核心：把窗口读出的 SSO 域 cookie 名值对写入无头 SSO 档案
/// （纯函数，供成功路径的 [`export_sso_session_jar`] 与单测共用）。
///
/// 档案 loaded-or-fresh：既有档案复用其稳定 `deviceId`（首次生成、永久复用，由
/// `exv_core::sso_session_jar` 的 API 自身管理）；无档案或档案损坏（jar 文档允许
/// 调用方丢弃重建）时新建，`deviceId` 随本次落盘持久化。非法字符的名值对跳过并
/// 计数（cookie 是附带状态，跳过不劣于没有），其余照常入库；读档/落盘失败返回
/// 错误字符串（不含值），由调用方只记日志。
fn export_sso_cookies(
    pairs: &[(String, String)],
    profile: &std::path::Path,
) -> Result<SsoExportOutcome, String> {
    if pairs.is_empty() {
        return Ok(SsoExportOutcome::NoCookies);
    }
    let mut fresh_device = false;
    let mut jar = match exv_core::sso_session_jar::load(profile) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            fresh_device = true;
            exv_core::sso_session_jar::SsoSessionJar::new_fresh()?
        }
        Err(reason) => {
            // 档案损坏：按 jar 文档「丢弃重建」——登录已成功，新鲜 SSO cookie 与新
            // deviceId 比不可用档案更有价值；重建事实记一条日志（不含任何值）。
            fresh_device = true;
            log::warn!(
                target: "exv.atrust_auth",
                "embedded.sso_export result=jar_rebuilt reason={reason}"
            );
            exv_core::sso_session_jar::SsoSessionJar::new_fresh()?
        }
    };
    let mut skipped_invalid = 0;
    for (name, value) in pairs {
        if jar.insert_cookie(name, value).is_err() {
            skipped_invalid += 1;
        }
    }
    let names = jar.cookie_names().into_iter().map(str::to_owned).collect();
    exv_core::sso_session_jar::save(profile, &jar)?;
    Ok(SsoExportOutcome::Saved {
        names,
        fresh_device,
        skipped_invalid,
    })
}

/// S7 阶段 B（交付物 5）：内嵌窗口成功路径的附加 SSO cookie 导出（严格附加步骤）。
///
/// 在窗口存活期内用与网关探针相同的 Cookie API 机制（[`read_cookies`]）读取
/// `sso.ecnu.edu.cn` 域 cookie——HttpOnly 的 `SESSION`/`SOURCEID_TGC` 由此可读，
/// 读取结果只有名/值对（属性标志已被 API 丢弃）——经 [`export_sso_cookies`] 写入
/// 无头 SSO 档案。档案目录与 `atrust_session_cache` 共用同一根
/// （`%LOCALAPPDATA%/EXV/atrust-browser`），阶段 C 的 headless 线按同一约定读取。
///
/// 严格附加：任何失败只记一条 `exv.atrust_auth` 日志后正常返回（返回 `()`，无错误
/// 传播通道），登录成功语义不受影响；日志只携带 cookie 名集合与计数，绝不携带值。
async fn export_sso_session_jar(window: &WebviewWindow, profile: &std::path::Path) {
    // SCHOOL_LOGIN_ORIGIN 即学校 SSO 域 origin；解析失败只可能源于常量损坏。
    let Ok(sso_url) =
        format!("{}/", exv_core::school_auto_login::SCHOOL_LOGIN_ORIGIN).parse::<tauri::Url>()
    else {
        log::warn!(target: "exv.atrust_auth", "embedded.sso_export result=url_error");
        return;
    };
    let cookies = match read_cookies(window.clone(), sso_url).await {
        Ok(cookies) => cookies,
        Err(_) => {
            log::warn!(target: "exv.atrust_auth", "embedded.sso_export result=read_error");
            return;
        }
    };
    match export_sso_cookies(&cookies, profile) {
        Ok(SsoExportOutcome::Saved {
            names,
            fresh_device,
            skipped_invalid,
        }) => {
            log::info!(
                target: "exv.atrust_auth",
                "embedded.sso_export result=ok cookies={} fresh_device={} skipped_invalid={}",
                if names.is_empty() {
                    "none".to_owned()
                } else {
                    names.join(",")
                },
                fresh_device,
                skipped_invalid
            );
        }
        Ok(SsoExportOutcome::NoCookies) => {
            log::info!(target: "exv.atrust_auth", "embedded.sso_export result=no_cookies");
        }
        Err(reason) => {
            log::warn!(
                target: "exv.atrust_auth",
                "embedded.sso_export result=error reason={reason}"
            );
        }
    }
}


/// 原渠道执行接缝：HTTP/Owned 次序、FreshManual 与 owner 控制仍走同一编排器。
pub(super) async fn drive_channels<'channels>(
    orchestrator: &exv_core::browser_auth::AuthOrchestrator<'_>,
    http: &'channels mut dyn exv_core::browser_auth::ChannelTrait,
    owned: &'channels mut dyn exv_core::browser_auth::ChannelTrait,
    action: &mut watch::Receiver<LoginAction>,
    http_first: bool,
    intent: exv_core::browser_auth::AuthIntent,
) -> Result<exv_core::browser_auth::AcquisitionOutcome, exv_core::browser_auth::AuthError> {
    use exv_core::browser_auth::*;
    let (sender, mut receiver) = watch::channel(owner_control(*action.borrow()));
    let result = {
        let mut channels: Vec<&mut dyn ChannelTrait> = if http_first && intent.allows_reuse() {
            vec![&mut *http, &mut *owned]
        } else {
            vec![&mut *owned]
        };
        let acquisition = orchestrator.acquire(&mut channels, &mut receiver);
        tokio::pin!(acquisition);
        let mut owner_alive = true;
        loop {
            tokio::select! { biased;
                changed = action.changed(), if owner_alive => {
                    if changed.is_err() { owner_alive = false; sender.send_replace(Some(AuthControl::Cancel)); }
                    else { sender.send_replace(owner_control(*action.borrow_and_update())); }
                }
                result = &mut acquisition => break result,
            }
        }
    };
    result
}
