//! 快速入门的最小后端编排。
//!
//! 此模块只协调现有 Core 配置边界：校验用户表单并保存配置。它不承担通知、窗口偏好
//! 或 C++ 历史 saga 的职责；密码仅沿既有 Core ConfigSet 边界传递。Windows 快速入门按
//! 原始密码是否非空派生记住标志，加密/清除仍由 Core 既有语义处理；普通设置的
//! ConfigSet 语义不变。
//!
//! T4（F8，2026-10-10）：删除「保存配置后按需安装服务」的组合安装腿——服务安装已由
//! 前端应用级协调器独立执行（唯一 busy/overlay 通道），后端组合腿是与真实路径分叉的
//! 死代码双真相。本命令从此只发 ConfigSet。

use std::collections::BTreeSet;
use std::future::Future;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::client::{ConfigItem, CoreClient, CoreState};
use super::error::AppError;
use super::state::ServiceStatus;

/// 前端提交的快速入门草稿。
#[derive(Debug, Clone, Deserialize)]
pub struct QuickStartApplyRequest {
    pub items: Vec<ConfigItem>,
}

/// 快速入门提交结果。
///
/// 配置保存失败走 command error；服务安装由前端独立编排，不再进入本回复
/// （`service_status` 保留为 wire 兼容占位，恒为 `None`）。
#[derive(Debug, Clone, Serialize)]
pub struct QuickStartApplyReply {
    pub ok: bool,
    pub service_status: Option<ServiceStatus>,
    pub message: String,
}

/// 快速入门允许写入的配置键，**必须覆盖前端草稿的完整键集**。
///
/// 前端 `product/quick-start.ts` 的 `DEFAULT_QUICK_START_CORE_DRAFT` 是另一处独立真相：
/// 少一个键，`validate_and_prepare_items` 就会在抵达 Core 之前拒绝整次提交，用户侧只能
/// 看到「快速入门提交失败，请重试。」，而 Core 日志里查不到任何记录（2026-09-21 的
/// `connection_mode`、2026-10-10 的 `campus_dns_domains` 就是这样漏掉的——后者即 T11
/// 所修的第二次同型事故）。键序逐行对齐前端草稿，便于直接比对。
/// `allowed_config_keys_mirror_the_pinned_frontend_core_draft_key_set`（kernel/tests.rs）
/// 以固定镜像清单钉住这条不变量；`pub(super)` 仅为该测试可见。
pub(super) const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "protocol",
    "server",
    "cisco_server",
    "atrust_server",
    "atrust_auth_mode",
    // T4c：快速入门默认草稿含自动登录默认值（false），白名单须同步接受。
    "atrust_auto_login",
    "username",
    "password",
    "remember_password",
    "connection_mode",
    "routes",
    // T11：10e31617d 起前端草稿含校园 DNS 域名键，白名单须同步接受（core config_set
    // 的 match 白名单已有同名分支，只读核对通过）。
    "campus_dns_domains",
    "user_agent",
    "mtu",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
    // T4 重连两设置项（§2.8；连接信封镜像键）。
    "reconnect_on_oneshot_engine_crash",
    "reconnect_notify",
];

/// Tauri 快速入门命令。`app/src/lib.rs` 统一负责命令注册。
///
/// T9（§2.9 业务唤醒）→ A1-1（2026-10-09 区域 A）：core 不可达时不再是一次
/// 「唤醒＋整编排重放」（旧形状把 ServiceControl 也卷进重试，存在二次 UAC 面），
/// 改为 ConfigSet 阶段的有界重试环（见 [`super::retry`]）：仅对 `CoreUnreachable`
/// 重试、唤醒走 `ensure_core_available`、整环 15s 硬上界；唤醒失败 → typed
/// `CoreWakeFailed`（前端弹「尝试重启程序 / 仅退出」模态，语义不变）。
#[tauri::command]
pub async fn quick_start_apply(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
    session: State<'_, super::client::CoreSession>,
    request: QuickStartApplyRequest,
) -> Result<QuickStartApplyReply, AppError> {
    apply(
        &CoreClient,
        &state,
        request,
        Some(|| async {
            super::bootstrap::ensure_core_available(&app, &state, &session)
                .await
                .map(|_| ())
        }),
    )
    .await
}

/// 可在真实 Core gRPC 边界测试的编排实现（带 ConfigSet 阶段的有界重试环）。
///
/// `wake` 参数化业务唤醒（win32 命令层注入 `ensure_core_available`；测试注入
/// 确定性桩或 `None` 模拟纯退避形状——darwin 同构参数化，见 darwin 侧同名模块）。
pub async fn apply<W, WFut>(
    client: &CoreClient,
    state: &CoreState,
    request: QuickStartApplyRequest,
    wake: Option<W>,
) -> Result<QuickStartApplyReply, AppError>
where
    W: FnMut() -> WFut,
    WFut: Future<Output = Result<(), AppError>>,
{
    let items = validate_and_prepare_items(request.items)?;
    // A1-1（复审须修 2）：重试环只包裹 ConfigSet 阶段——ConfigSet 幂等已核实
    // （core 侧原子全量重写，同 items 重放 = 同结果，无写放大）。本命令不再有
    // ServiceControl 阶段（T4 删除组合安装腿，服务安装由前端应用级协调器独立执行）。
    let saved = super::retry::retry_on_core_unreachable(
        || async { client.config_set(state, items.clone()).await },
        wake,
    )
    .await?;
    if !saved {
        return Err(AppError::Internal("快速入门配置未能保存".to_string()));
    }

    Ok(QuickStartApplyReply {
        ok: true,
        service_status: None,
        message: "快速入门配置已保存".to_string(),
    })
}

fn validate_and_prepare_items(items: Vec<ConfigItem>) -> Result<Vec<ConfigItem>, AppError> {
    let protocol = items
        .iter()
        .find(|item| item.key == "protocol")
        .map(|item| item.value.as_str())
        .unwrap_or("cisco");
    let mode = items
        .iter()
        .find(|item| item.key == "atrust_auth_mode")
        .map(|item| item.value.as_str())
        .unwrap_or("browser");
    let requires_username = protocol != "atrust" || mode == "headless";
    let mut keys = BTreeSet::new();
    let mut required = BTreeSet::new();
    let mut prepared = Vec::with_capacity(items.len());
    let mut password = String::new();

    for item in items {
        if !ALLOWED_CONFIG_KEYS.contains(&item.key.as_str()) {
            return Err(AppError::Internal(format!(
                "快速入门不接受配置项 '{}'",
                item.key
            )));
        }
        if !keys.insert(item.key.clone()) {
            return Err(AppError::Internal(format!(
                "快速入门配置项 '{}' 重复",
                item.key
            )));
        }
        if item.key == "server" || (item.key == "username" && requires_username) {
            if item.value.trim().is_empty() {
                return Err(AppError::Internal(format!(
                    "快速入门配置项 '{}' 不能为空",
                    item.key
                )));
            }
            required.insert(item.key.clone());
        }
        match item.key.as_str() {
            "password" => password = item.value,
            // 兼容旧请求字段，但不让旧复选框值覆盖快速入门的新规则。
            "remember_password" => {}
            _ => prepared.push(item),
        }
    }

    let required_keys: &[&str] = if requires_username {
        &["server", "username"]
    } else {
        &["server"]
    };
    for required_key in required_keys {
        if !required.contains(*required_key) {
            return Err(AppError::Internal(format!(
                "快速入门缺少配置项 '{required_key}'"
            )));
        }
    }

    let remember_password = !password.is_empty();
    if keys.contains("password") {
        prepared.push(ConfigItem {
            key: "password".to_string(),
            value: password,
        });
    }
    // S5a（2026-10-26，与 darwin 壳同构）：密码留空时**省略**派生的 remember_password
    // 键——Core 对缺键保持原值，已保存密文与 owner 不被"派生 false"误清除
    //（"留空保留原密码"承诺在快速入门同样成立；前端 `quickStartItems` 同款少键）。
    // 非空密码仍派生 true 一并提交（旧标志无论如何都会被覆盖为派生值，行为不变）。
    if remember_password {
        prepared.push(ConfigItem {
            key: "remember_password".to_string(),
            value: remember_password.to_string(),
        });
    }

    Ok(prepared)
}
