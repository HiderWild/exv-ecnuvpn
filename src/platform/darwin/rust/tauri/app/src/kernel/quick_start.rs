//! 快速入门的最小后端编排（win32 `kernel/quick_start.rs` 的 darwin 移植）。
//!
//! 语义同 win32：校验用户表单 → 先保存配置（darwin core `ConfigSet`）→ 再按需
//! 处理服务安装。差异仅在服务安装分支：darwin 的服务安装走**服务面板**（core
//! `ServiceControl` 经服务代理提权执行），快速入门不代为安装；`install_service = true`
//! 时以 `ok = false` + 可读 message 如实回报（前端不把快速入门显示成已完成安装）。
//! 密码仅沿既有 Core ConfigSet 边界传递。快速入门按原始密码是否非空派生记住标志，
//! 加密/清除仍由 Core 既有语义处理；普通设置的 ConfigSet 语义不变。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::client::{ConfigItem, CoreClient, CoreState};
use super::error::AppError;
use super::state::ServiceStatus;

/// 前端提交的快速入门草稿。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct QuickStartApplyRequest {
    pub items: Vec<ConfigItem>,
    pub install_service: bool,
}

/// 快速入门提交结果。
///
/// 配置保存失败走 command error；服务安装是后续独立副作用，因此其业务失败以可显示的
/// `ok=false` 回复返回，前端不得把它显示成已完成。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct QuickStartApplyReply {
    pub ok: bool,
    pub service_status: Option<ServiceStatus>,
    pub message: String,
}

const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "server",
    "username",
    "password",
    "remember_password",
    "routes",
    "user_agent",
    "mtu",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
];

/// Tauri 快速入门命令。`app/src/main.rs` 统一负责命令注册。
#[tauri::command]
pub(crate) async fn quick_start_apply(
    state: State<'_, CoreState>,
    request: QuickStartApplyRequest,
) -> Result<QuickStartApplyReply, AppError> {
    apply(&CoreClient, &state, request).await
}

/// 可在真实 Core gRPC 边界测试的编排实现。
pub(crate) async fn apply(
    client: &CoreClient,
    state: &CoreState,
    request: QuickStartApplyRequest,
) -> Result<QuickStartApplyReply, AppError> {
    let items = validate_and_prepare_items(request.items)?;
    let saved = client.config_set(state, items).await?;
    if !saved {
        return Err(AppError::Internal(
            "快速入门配置未能保存，未尝试安装服务".to_string(),
        ));
    }

    if !request.install_service {
        return Ok(QuickStartApplyReply {
            ok: true,
            service_status: None,
            message: "快速入门配置已保存".to_string(),
        });
    }

    // darwin 平台旁路（诚实语义）：服务安装由服务面板单独发起，快速入门不调用
    // core ServiceControl；配置已真实保存，服务安装以 ok=false + 可读 message 如实回报。
    Ok(darwin_service_install_reply())
}

/// darwin 服务安装分支的诚实回复：配置已保存，服务形态不适用。
fn darwin_service_install_reply() -> QuickStartApplyReply {
    QuickStartApplyReply {
        ok: false,
        service_status: None,
        message: "快速入门配置已保存；Darwin 的服务安装请在设置的服务面板单独进行。"
            .to_string(),
    }
}

fn validate_and_prepare_items(items: Vec<ConfigItem>) -> Result<Vec<ConfigItem>, AppError> {
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
        if matches!(item.key.as_str(), "server" | "username") {
            if item.value.trim().is_empty() {
                return Err(AppError::Internal(format!(
                    "快速入门配置项 '{}' 不能为空",
                    item.key
                )));
            }
            required.insert(item.key.clone());
        }
        match item.key.as_str() {
            // 取出密码用于派生记住标志：既不入 prepared，也不被 core 的重复键检查看见。
            "password" => password = item.value,
            // 兼容旧请求字段，但不让旧复选框值覆盖快速入门的新规则。
            "remember_password" => {}
            _ => prepared.push(item),
        }
    }

    for required_key in ["server", "username"] {
        if !required.contains(required_key) {
            return Err(AppError::Internal(format!(
                "快速入门缺少配置项 '{required_key}'"
            )));
        }
    }

    let remember_password = !password.is_empty();
    prepared.push(ConfigItem {
        key: "password".to_string(),
        value: password,
    });
    prepared.push(ConfigItem {
        key: "remember_password".to_string(),
        value: remember_password.to_string(),
    });

    Ok(prepared)
}
