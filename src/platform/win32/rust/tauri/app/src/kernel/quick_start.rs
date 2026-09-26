//! 快速入门的最小后端编排。
//!
//! 此模块只协调现有 Core 配置与服务控制边界：校验用户表单、先保存配置、再按需安装
//! 服务。它不承担通知、窗口偏好或 C++ 历史 saga 的职责；密码仅沿既有 Core ConfigSet
//! 边界传递。Windows 快速入门按原始密码是否非空派生记住标志，加密/清除仍由 Core
//! 既有语义处理；普通设置的 ConfigSet 语义不变。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::client::{ConfigItem, CoreClient, CoreState};
use super::error::AppError;
use super::state::{ServiceControlAction, ServiceStatus};

/// 前端提交的快速入门草稿。
#[derive(Debug, Clone, Deserialize)]
pub struct QuickStartApplyRequest {
    pub items: Vec<ConfigItem>,
    pub install_service: bool,
}

/// 快速入门提交结果。
///
/// 配置保存失败走 command error；服务安装是后续独立副作用，因此其业务失败以可显示的
/// `ok=false` 回复返回，前端不得把它显示成已完成。
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
/// `connection_mode` 就是这样漏掉的）。键序逐行对齐前端草稿，便于直接比对。
/// `quick_start_apply_accepts_the_full_frontend_draft_key_set` 钉住这条不变量。
const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "server",
    "username",
    "password",
    "remember_password",
    "connection_mode",
    "routes",
    "user_agent",
    "mtu",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
];

/// Tauri 快速入门命令。`app/src/lib.rs` 统一负责命令注册。
#[tauri::command]
pub async fn quick_start_apply(
    state: State<'_, CoreState>,
    request: QuickStartApplyRequest,
) -> Result<QuickStartApplyReply, AppError> {
    apply(&CoreClient, &state, request).await
}

/// 可在真实 Core gRPC 边界测试的编排实现。
pub async fn apply(
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

    let service = client
        .service_control(state, ServiceControlAction::Install)
        .await?;
    Ok(QuickStartApplyReply {
        ok: service.ok,
        service_status: service.service_status,
        message: service.message,
    })
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
