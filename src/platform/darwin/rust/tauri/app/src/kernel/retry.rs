//! ConfigSet 阶段对 `CoreUnreachable` 的纯退避有界重试（A1-1，2026-10-09 区域 A）。
//!
//! win32 `kernel/retry.rs` 的 darwin 同构参数化形状（六区裁定 §二.A-1「同构
//! config_set/darwin 两命令参数化」）：同一助手逻辑、同一常量集（3 次尝试、
//! 500ms/1500ms 退避、整环 15s `tokio::time::timeout` 硬上界），差异仅在唤醒——
//! darwin 无 `ensure_core_available` 唤醒基础设施（core 恢复只有连接路径的
//! `recover_stopped_core_for_connect`），故 `wake` 恒传 `None`：纯退避重试，
//! core 若死则环耗尽后快速失败，交给连接路径的恢复语义。
//!
//! 与 win32 相同的强制约束：
//! * 环只包裹 ConfigSet 阶段（ConfigSet 幂等：core 侧原子全量重写，同 items
//!   重放 = 同结果，无写放大）；ServiceControl 绝不进环。
//! * 仅对 `CoreUnreachable` 重试；其余错误一次即返。
//! * 整环硬上界使预算可证：无论内部环节如何叠加，用户侧最坏等待不超过
//!   [`CONFIG_RETRY_DEADLINE`]，超时按 core 不可达如实报错（kind=
//!   `core_unreachable`，前端 A1-2 文案分流命中「后台服务未就绪」）。
//!
//! darwin 宿主未运行（2026-10-09 本区执行在 Windows 工作树进行）：本模块为
//! 参数化文本改动，未经 darwin 本地编译/测试验证，如实登记；win32 同构版已由
//! `exv-ui --lib kernel::retry`/`quick_start` 用例群验证（5+17 通过）。

use std::future::Future;

use super::error::AppError;

/// 整环硬上界（墙钟，与 win32 同值）。darwin 无唤醒（纯退避），最坏叠加 =
/// 三次尝试 + 退避 500+1500ms；15s 上界与 win32 保持同构，未来 darwin 落地
/// 唤醒设施时无需重新校准预算。
pub(crate) const CONFIG_RETRY_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(15);

/// 尝试间退避序列（序列长度 = 额外重试次数；总尝试次数 = 长度 + 1 = 3）。
const CONFIG_RETRY_BACKOFFS: [std::time::Duration; 2] = [
    std::time::Duration::from_millis(500),
    std::time::Duration::from_millis(1500),
];

/// 无唤醒形状的典型化占位：darwin 调用方恒用（直接传 `None` 需要给 `W` 一个
/// 满足 `FnMut` 约束的具体类型——fn 指针实现 FnMut，永不被调用）。
pub(crate) const NO_WAKE: Option<fn() -> std::future::Pending<Result<(), AppError>>> = None;

/// 有界重试环（win32 同构；darwin 调用方恒传 `None` 唤醒）：
/// `attempt` 返回 `CoreUnreachable` 时退避后重试；整环受
/// [`CONFIG_RETRY_DEADLINE`] 硬上界约束。
///
/// * 非 `CoreUnreachable` 错误一次即返（不重试）；
/// * `wake` 自身的失败立即透传（darwin 当前无唤醒设施，`Some` 分支为未来
///   同构收编预留——见模块注释）；
/// * 硬上界耗尽 → `CoreUnreachable`（预算可证：耗尽时间 ≤ 上界）。
pub(crate) async fn retry_on_core_unreachable<T, F, Fut, W, WFut>(
    mut attempt: F,
    mut wake: Option<W>,
) -> Result<T, AppError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AppError>>,
    W: FnMut() -> WFut,
    WFut: Future<Output = Result<(), AppError>>,
{
    let outcome = tokio::time::timeout(CONFIG_RETRY_DEADLINE, async {
        let mut result = attempt().await;
        for backoff in CONFIG_RETRY_BACKOFFS {
            match result {
                Err(AppError::CoreUnreachable(_)) => {}
                other => return other,
            }
            if let Some(wake) = wake.as_mut() {
                wake().await?;
            }
            tokio::time::sleep(backoff).await;
            result = attempt().await;
        }
        result
    })
    .await;

    outcome.unwrap_or_else(|_| {
        Err(AppError::CoreUnreachable(format!(
            "core did not become reachable within the {}s bounded retry budget",
            CONFIG_RETRY_DEADLINE.as_secs()
        )))
    })
}
