//! 学校登录尝试计数器（验证码风险预告，2026-10-07 用户需求）。
//!
//! ## 需求与边界
//!
//! 只统计**到达学校 SSO 登录表单页**的窗口线登录（`embedded_auth` 到达
//! `sso.ecnu.edu.cn/login`；判定复用 [`exv_core::school_auto_login`] 的
//! `SCHOOL_LOGIN_ORIGIN` + `SCHOOL_LOGIN_PATH` 常量，与系统浏览器线
//! `atrust_browser.rs` 的「已到学校页」判据同源）。依据（提速/无头取证）：被
//! 网关限流/触发验证码的是**凭据表单登录**——快速连续的表单登录会被限流，
//! 纯 HTTP 重定向链（无头快路径、缓存命中、casLogin 穿透）从未触发。因此：
//!   * 无头快路径、会话缓存命中、不经表单的 casLogin 重定向穿透**不计数**；
//!   * 每次 `embedded_auth::acquire` 调用至多计一次（per-acquire latch，
//!     见 `embedded_auth::form_login_reached`），同一尝试内页面 reload 不重复计数；
//!   * 已知局限（如实记录）：窗口保持打开期间用户在表单内手动重复提交不单独计数。
//!
//! 计数是**纯观察性**的：不改变认证流程任何行为，只用于在**下一次连接**的
//! 认证等待模态里给出预告——滚动窗口（[`WARNING_WINDOW`]，30 分钟）内已记录
//! `>= [`WARNING_THRESHOLD`]`（3）次时，用户再点连接，`exv://browser-login`
//! 开始事件附带 [`LoginThrottleWarning`]，前端在认证等待模态内提示
//! 「您在30分钟内已经尝试登录过3次，可能触发验证码，请您留意」。
//!
//! ## 持久化
//!
//! `%LOCALAPPDATA%\EXV\profile\default\login-attempts.json`（与 `ui_prefs.rs`
//! 同一产品状态目录、同一原子写策略）。构造时载入内存，此后每次记录即落盘，
//! 应用重启不丢计数；文件损坏/不可解析 → 记一条 warn、从空计数开始（不阻断
//! 认证）。文件内容只有时间戳（epoch 毫秒），不含任何凭据或秘密。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 滚动窗口长度：30 分钟（警示句中「30分钟」的唯一事实源）。
pub const WARNING_WINDOW: Duration = Duration::from_secs(30 * 60);
/// 触发阈值：窗口内已记录 >= 3 次时，下一次连接给出预告（句中「3次」的锚点）。
pub const WARNING_THRESHOLD: u32 = 3;
/// 本模块持久化文件名（位于 `ui_prefs::ui_state_dir()` 产品状态目录）。
const ATTEMPTS_FILE: &str = "login-attempts.json";

/// `exv://browser-login` 载荷上的登录频次预告（数字随载荷下发，前端据此渲染
/// 警示句；句式模板只在前端 `BrowserAuthWaitingModal.vue` 一处）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LoginThrottleWarning {
    /// 窗口内已记录的表单登录次数（>= threshold；如实上报实际值）。
    pub attempts: u32,
    /// 滚动窗口长度（分钟；= `WARNING_WINDOW`）。
    pub window_minutes: u32,
    /// 触发阈值（= `WARNING_THRESHOLD`）。
    pub threshold: u32,
}

/// 磁盘 JSON 形状（键名 snake_case；只有时间戳，无秘密）。
#[derive(Debug, Default, Deserialize, Serialize)]
struct AttemptsFile {
    /// 表单登录时刻（epoch 毫秒，升序追加）。
    #[serde(default)]
    attempts: Vec<u64>,
}

/// 进程内共享的登录尝试计数器（setup 时按产品状态目录构造并 `manage`；
/// 持锁串行化读写，锁内无 panic 点）。
pub struct LoginAttemptTracker {
    /// 计数文件完整路径。
    path: PathBuf,
    /// 内存态：已记录时刻（epoch 毫秒，升序）。
    inner: Mutex<Vec<u64>>,
}

impl LoginAttemptTracker {
    /// 生产构造：产品状态目录不可解析时返回 `None`（调用方不 manage，计数停用）。
    #[must_use]
    pub fn from_product_dir() -> Option<Self> {
        let dir = crate::ui_prefs::ui_state_dir()?;
        Some(Self { path: dir.join(ATTEMPTS_FILE), inner: Mutex::new(load_attempts(&dir)) })
    }

    /// 测试构造：注入目录。
    #[must_use]
    pub fn for_dir(dir: PathBuf) -> Self {
        Self { path: dir.join(ATTEMPTS_FILE), inner: Mutex::new(load_attempts(&dir)) }
    }

    /// 记录一次到达学校 SSO 登录表单页的窗口线登录，返回记录后窗口内的总次数
    /// （供 `embedded.form_login_counted` 日志；不含秘密）。同时清理过期条目并
    /// 尽力落盘（IO 失败仅告警，不影响内存计数——预告语义可容忍文件滞后）。
    ///
    /// # Errors
    /// 内部锁中毒（理论不可达：锁内无 panic 点）。
    pub fn record_form_login(&self, now: SystemTime) -> Result<u32, String> {
        let mut attempts = self.inner.lock().map_err(|e| e.to_string())?;
        let stamp = epoch_millis(now).ok_or("系统时间早于 Unix 纪元")?;
        prune_expired(&mut attempts, now);
        attempts.push(stamp);
        if let Err(error) = store_atomic(&self.path, &attempts) {
            log::warn!(
                target: "exv.login_attempts",
                "login_attempts.persist_failed error={error}"
            );
        }
        Ok(attempts.len() as u32)
    }

    /// 判定是否应给出验证码风险预告：窗口内已记录次数 >= 阈值时返回预告载荷。
    ///
    /// 只读不改内存/磁盘（过期清理在下次 [`Self::record_form_login`] 时进行）；
    /// 调用时机是浏览器认证开始（`exv://browser-login` active=true），此时本次
    /// 尝试尚未计数，因此判定反映的是**此前**的尝试——与需求「3+ 次之后再点
    /// 连接时预告」一致。
    ///
    /// # Errors
    /// 内部锁中毒（理论不可达：锁内无 panic 点）。
    pub fn warning_due(&self, now: SystemTime) -> Result<Option<LoginThrottleWarning>, String> {
        let attempts = self.inner.lock().map_err(|e| e.to_string())?;
        let in_window = attempts
            .iter()
            .filter(|stamp| within_window(**stamp, now))
            .count() as u32;
        if in_window >= WARNING_THRESHOLD {
            Ok(Some(LoginThrottleWarning {
                attempts: in_window,
                window_minutes: u32::try_from(WARNING_WINDOW.as_secs() / 60).unwrap_or(u32::MAX),
                threshold: WARNING_THRESHOLD,
            }))
        } else {
            Ok(None)
        }
    }
}

/// `stamp`（epoch 毫秒）在 `now` 的滚动窗口内（严格小于窗口长度；恰好到期即出窗）。
/// 未来时刻（系统时钟回拨的残留）在窗口宽限内仍视为有效。
fn within_window(stamp: u64, now: SystemTime) -> bool {
    let now_ms = epoch_millis(now).unwrap_or(u64::MAX);
    let window_ms = u64::try_from(WARNING_WINDOW.as_millis()).unwrap_or(u64::MAX);
    stamp > now_ms.saturating_sub(window_ms) && stamp <= now_ms.saturating_add(window_ms)
}

/// 就地清理 `attempts` 中已出窗（及明显异常：远未来的时钟残留）的条目，保持
/// 剩余条目原有顺序。
fn prune_expired(attempts: &mut Vec<u64>, now: SystemTime) {
    attempts.retain(|stamp| within_window(*stamp, now));
}

/// `SystemTime` → epoch 毫秒（早于纪元返回 `None`）。
fn epoch_millis(at: SystemTime) -> Option<u64> {
    u64::try_from(at.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// 读目录下的计数文件；不存在/损坏/不可解析 → 空表并记 warn（从空计数开始）。
fn load_attempts(dir: &Path) -> Vec<u64> {
    let path = dir.join(ATTEMPTS_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AttemptsFile>(&text) {
            Ok(file) => file.attempts,
            Err(error) => {
                log::warn!(
                    target: "exv.login_attempts",
                    "login_attempts.load_corrupt error={error}; starting empty"
                );
                Vec::new()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            log::warn!(
                target: "exv.login_attempts",
                "login_attempts.load_failed error={error}; starting empty"
            );
            Vec::new()
        }
    }
}

/// 原子写（tmp + rename），失败静默降级为直接写（与 `ui_prefs.rs` 同策略）。
fn store_atomic(path: &Path, attempts: &[u64]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&AttemptsFile { attempts: attempts.to_vec() })
        .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if fs::rename(&tmp, path).is_err() {
        fs::copy(&tmp, path).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&tmp);
    }
    Ok(())
}

// ---- 测试 ----
