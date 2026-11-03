//! 快速入门的最小配置入口：校验草稿后沿既有 Core ConfigSet 保存。
//!
//! 服务安装由前端在配置安全回读后，通过独立服务操作入口发起；本命令不组合安装。
//! 密码仅沿既有 Core ConfigSet 边界传递。S5a（2026-10-26）：非空密码派生记住标志
//! `remember_password=true` 一并提交；密码留空时**省略**该键——Core 对缺键保持
//! 原值，已保存密文与 owner 不被"派生 false"误清除。普通设置的 ConfigSet 语义不变。

use std::collections::BTreeSet;
use std::future::Future;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::client::{ConfigItem, CoreClient, CoreState};
use super::error::AppError;
use super::state::ServiceStatus;

/// 前端提交的快速入门草稿。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct QuickStartApplyRequest {
    pub items: Vec<ConfigItem>,
}

/// 快速入门提交结果。
///
/// 配置保存失败走 command error；成功仅表示配置已保存。
/// `service_status` 保留回复兼容形状，本入口恒为 `None`。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct QuickStartApplyReply {
    pub ok: bool,
    pub service_status: Option<ServiceStatus>,
    pub message: String,
}

/// 快速入门允许写入的配置键，**必须覆盖前端草稿的完整键集**。
///
/// 前端 `product/quick-start.ts` 的 `DEFAULT_QUICK_START_CORE_DRAFT` 是另一处独立真相：
/// 少一个键，`validate_and_prepare_items` 就会在抵达 Core 之前拒绝整次提交，用户侧只能
/// 看到「快速入门提交失败，请重试。」，而 Core 日志里查不到任何记录（win32 2026-09-21 的
/// `connection_mode` 就是这样漏掉的）。键序逐行对齐前端草稿，便于直接比对。
/// `quick_start_apply_accepts_the_full_frontend_draft_key_set` 钉住这条不变量。
const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "server",
    "username",
    "password",
    "remember_password",
    "routes",
    "user_agent",
    "mtu",
    "protocol",
    "atrust_server",
    "atrust_auth_mode",
    "atrust_auto_login",
    "campus_dns_domains",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
];

/// Tauri 快速入门命令。`app/src/main.rs` 统一负责命令注册。
///
/// A1-1d（2026-10-09 区域 A）：ConfigSet 阶段的有界重试环（[`super::retry`]，
/// win32 同构参数化——darwin 无唤醒设施，`wake` 恒 `None` 纯退避：core 若死则
/// 环耗尽后快速失败，交给连接路径的恢复语义）。darwin 宿主未运行，本改动为
/// 参数化文本改动，未经本地编译/测试验证，如实登记。
#[tauri::command]
pub(crate) async fn quick_start_apply(
    state: State<'_, CoreState>,
    request: QuickStartApplyRequest,
) -> Result<QuickStartApplyReply, AppError> {
    apply(&CoreClient, &state, request, super::retry::NO_WAKE).await
}

/// 可在真实 Core gRPC 边界测试的编排实现（带 ConfigSet 阶段的有界重试环）。
///
/// `wake` 参数化业务唤醒（win32 命令层注入 `ensure_core_available`；darwin 恒
/// `None`，形状与 win32 [`apply`] 同构，未来落地唤醒设施时直接收编）。
pub(crate) async fn apply<W, WFut>(
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
    // 重试环只包裹 ConfigSet（幂等：原子全量重写），不包含独立服务操作。
    let saved = super::retry::retry_on_core_unreachable(
        || async { client.config_set(state, items.clone()).await },
        wake,
    )
    .await?;
    if !saved {
        return Err(AppError::Internal(
            "快速入门配置未能保存".to_string(),
        ));
    }

    Ok(QuickStartApplyReply {
        ok: true,
        service_status: None,
        message: "快速入门配置已保存".to_string(),
    })
}

fn validate_and_prepare_items(items: Vec<ConfigItem>) -> Result<Vec<ConfigItem>, AppError> {
    // 先检查整批键，避免账号或协议必填分支掩盖后续未知键／重复键。
    let mut keys = BTreeSet::new();
    for item in &items {
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
    }

    // 只使用本次请求的协议；缺键按旧Cisco入口，并在同批ConfigSet中明确写回。
    let protocol = match items.iter().find(|item| item.key == "protocol") {
        Some(item) => match item.value.trim() {
            "cisco" => "cisco",
            "atrust" => "atrust",
            _ => return Err(AppError::Internal("快速入门协议必须为 cisco 或 atrust".to_string())),
        },
        None => "cisco",
    };
    let required_keys: &[&str] = if protocol == "atrust" {
        &["atrust_server"]
    } else {
        &["server", "username"]
    };
    for required_key in required_keys {
        let Some(item) = items.iter().find(|item| item.key == *required_key) else {
            return Err(AppError::Internal(format!(
                "快速入门缺少配置项 '{required_key}'"
            )));
        };
        if item.value.trim().is_empty() {
            return Err(AppError::Internal(format!(
                "快速入门配置项 '{required_key}' 不能为空"
            )));
        }
    }

    let mut prepared = Vec::with_capacity(items.len() + 1);
    let mut password = String::new();
    for mut item in items {
        match item.key.as_str() {
            // 保留原始密码字节，仅由是否非空派生记住标志。
            "password" => password = item.value,
            "remember_password" => {}
            "protocol" => {
                item.value = protocol.to_string();
                prepared.push(item);
            }
            // 未使用协议的显式字段仍原样送交Core校验，不删除也不补虚拟账号。
            _ => prepared.push(item),
        }
    }
    if !keys.contains("protocol") {
        prepared.push(ConfigItem {
            key: "protocol".to_string(),
            value: protocol.to_string(),
        });
    }
    let remember_password = !password.is_empty();
    prepared.push(ConfigItem {
        key: "password".to_string(),
        value: password,
    });
    // S5a（2026-10-26）：密码留空时**省略**派生的 remember_password 键——Core 对
    // 缺键保持原值，已保存密文与 owner 不被"派生 false"误清除（"留空保留原密码"
    // 承诺在快速入门同样成立；前端 `quickStartItems` 同款少键）。非空密码仍派生
    // true 一并提交（旧标志无论如何都会被覆盖为派生值，行为不变）。
    if remember_password {
        prepared.push(ConfigItem {
            key: "remember_password".to_string(),
            value: "true".to_string(),
        });
    }

    Ok(prepared)
}
