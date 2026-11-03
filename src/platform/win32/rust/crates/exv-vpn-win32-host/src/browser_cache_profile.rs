//! aTrust 认证缓存目录选择；与 WebView 用户数据和 AgentSim 证书材料分开。

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

const EXPLICIT_DIR: &str = "EXV_ATRUST_BROWSER_DATA_DIR";

#[derive(Clone)]
pub enum BrowserDataDirs {
    Default(PathBuf),
    Explicit(PathBuf),
}

impl BrowserDataDirs {
    /// 从同次冻结的缓存路径和原选择恢复对象，不重新解析进程环境。
    pub fn from_frozen_profile(profile: PathBuf, explicit: bool) -> Self {
        if explicit {
            Self::Explicit(profile)
        } else {
            Self::Default(profile)
        }
    }

    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit(_))
    }

    pub fn cache_profile(&self) -> &Path {
        match self {
            Self::Default(path) | Self::Explicit(path) => path,
        }
    }

    pub fn owned_webview_profile(
        &self,
        default_app_data: impl FnOnce() -> Result<PathBuf, String>,
    ) -> Result<PathBuf, String> {
        match self {
            Self::Default(_) => default_app_data().map(|path| path.join("atrust-webview")),
            Self::Explicit(path) => Ok(path.join("webview")),
        }
    }
}

/// 进程入口只在此处读环境变量；调用者共享纯解析结果。
pub fn browser_data_dirs() -> Result<BrowserDataDirs, String> {
    resolve_atrust_browser_data_dir(
        env::var_os(EXPLICIT_DIR).as_deref(),
        env::var_os("LOCALAPPDATA").as_deref(),
    )
}

/// 纯解析入口；显式值无效时拒绝，绝不读取默认认证缓存。
fn resolve_atrust_browser_data_dir(
    explicit: Option<&OsStr>,
    local_app_data: Option<&OsStr>,
) -> Result<BrowserDataDirs, String> {
    if let Some(value) = explicit {
        let unicode = value
            .to_str()
            .ok_or_else(|| format!("{EXPLICIT_DIR} 必须是 Unicode 绝对路径"))?;
        if unicode.is_empty() {
            return Err(format!("{EXPLICIT_DIR} 不能为空"));
        }
        let path = PathBuf::from(unicode);
        if !path.is_absolute() {
            return Err(format!("{EXPLICIT_DIR} 必须是绝对路径"));
        }
        return Ok(BrowserDataDirs::Explicit(path));
    }
    local_app_data
        .map(PathBuf::from)
        .map(|path| BrowserDataDirs::Default(path.join("EXV/atrust-browser")))
        .ok_or_else(|| "无法定位 EXV 浏览器配置目录".to_owned())
}

/// 网关键控哈希（SHA-256 小写十六进制）；`manual-username-<sha>` 与
/// `manual-persistent/<sha>` 目录共用同一哈希源，保证跨次登录与账户记忆同键。
pub fn gateway_hash(gateway: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(gateway.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn manual_persistent_profile(root: &std::path::Path, gateway: &str) -> Result<PathBuf, String> {
    let parent = root.join("manual-persistent");
    std::fs::create_dir_all(&parent)
        .map_err(|e| format!("创建手动登录持久目录失败：{e}"))?;
    let path = parent.join(gateway_hash(gateway));
    std::fs::create_dir_all(&path).map_err(|e| format!("创建手动登录持久目录失败：{e}"))?;
    Ok(path)
}

/// 仅保存上次成功的学校账户，用于新鲜手动表单预填；不包含密码或会话。
fn manual_username_path(root: &std::path::Path, gateway: &str) -> std::path::PathBuf {
    root.join(format!("manual-username-{}.json", gateway_hash(gateway)))
}
pub fn remembered_username(root: &std::path::Path, gateway: &str) -> Option<String> {
    let bytes = std::fs::read(manual_username_path(root, gateway)).ok()?;
    serde_json::from_slice::<String>(&bytes).ok().filter(|name| !name.trim().is_empty())
}
pub fn remember_username(root: &std::path::Path, gateway: &str, username: &str) -> Result<(), String> {
    if username.trim().is_empty() { return Ok(()); }
    std::fs::create_dir_all(root).map_err(|_| "无法创建学校账户记忆目录")?;
    let bytes = serde_json::to_vec(username.trim()).map_err(|_| "学校账户记忆编码失败")?;
    std::fs::write(manual_username_path(root, gateway), bytes).map_err(|_| "无法保存学校账户记忆".into())
}
pub const USERNAME_COOKIE: &str = "__exv_manual_username";
pub fn observed_username(value: &str) -> Option<String> {
    if value.len() > 2048 || value.len() % 2 != 0 { return None; }
    let bytes: Option<Vec<_>> = value.as_bytes().chunks_exact(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()).collect();
    String::from_utf8(bytes?).ok().map(|name| name.trim().to_owned()).filter(|name| !name.is_empty())
}
/// 只预填学校账户；仅在本次用户点击或提交时观察账户，不读取密码或代替提交。
pub fn username_memory_script(username: Option<&str>) -> String {
    let username = serde_json::to_string(username.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    let body = r#"(()=>{if(location.origin!=='https://sso.ecnu.edu.cn'||location.pathname!=='/login')return;
const remembered=__USERNAME__;let attached=false;
const setup=()=>{if(attached)return true;const form=document.querySelector('form#normalLoginForm')||document.querySelector('form[action*="/login"]');
const input=form&&form.querySelector('input[name="username"]');if(!input)return false;attached=true;
if(!input.value&&remembered){input.value=remembered;input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));}
const observe=()=>{const value=input.value.trim();if(!value)return;const hex=Array.from(new TextEncoder().encode(value),b=>b.toString(16).padStart(2,'0')).join('');document.cookie='__exv_manual_username='+hex+'; Path=/; Secure; SameSite=Lax';};
form.addEventListener('submit',observe,true);const button=form.querySelector('button#submitBtn');if(button)button.addEventListener('click',observe,true);return true;};
if(!setup()){const observer=new MutationObserver(()=>{if(setup())observer.disconnect();});observer.observe(document,{childList:true,subtree:true});addEventListener('pagehide',()=>observer.disconnect(),{once:true});}})()"#;
    body.replace("__USERNAME__", &username)
}
