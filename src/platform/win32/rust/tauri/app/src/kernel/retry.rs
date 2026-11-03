
use std::future::Future;

use super::error::AppError;

/// 整环硬上界（墙钟）。取 15s 的论证：
/// * 覆盖最坏叠加区间（10-17s）的主要部分——两轮唤醒的典型路径（管道断 →
///   probe 快速失败 → 重拉 → 拨号就绪，每轮 ≈2-4s）加退避与三次尝试在 10s
///   量级内完成；
/// * 超出 15s 的极端叠加（区间上端）被硬切断并如实报 `core_unreachable`——
///   宁可提前报错也不无界等待（用户手势必须有界且可见，按钮已有
///   「正在保存配置…」置灰态反馈）；
/// * 回滚/调参单点：改此常量即可（六区裁定「参数集中在助手常量」要求）。
pub(crate) const CONFIG_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// 尝试间退避序列（序列长度 = 额外重试次数；总尝试次数 = 长度 + 1 = 3）。
/// 500ms/1500ms 递增退避覆盖「唤醒成功但新 core 拨号就绪稍慢」的残余窗口
/// （`dial_spawned_core` 有界等待之后的订阅替换、channel 就绪间隙）。
const CONFIG_RETRY_BACKOFFS: [std::time::Duration; 2] = [
    std::time::Duration::from_millis(500),
    std::time::Duration::from_millis(1500),
];

/// 有界重试环：`attempt` 返回 `CoreUnreachable` 时，先走 `wake`（win32 =
/// [`super::bootstrap::ensure_core_available`] 业务唤醒；`None` = 无唤醒纯退避，
/// darwin 参数化形状），退避后重试；整环受 [`CONFIG_RETRY_DEADLINE`] 硬上界约束。
///
/// * 非 `CoreUnreachable` 错误一次即返（不重试）；
/// * `wake` 自身的失败（如 `CoreWakeFailed`）立即透传——唤醒失败走前端
///   「尝试重启程序 / 仅退出」模态，与既有 T9 语义一致；
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
        tracing::warn!(
            target: "exv.core_recovery",
            "config_retry.deadline_exceeded budget_secs={}",
            CONFIG_RETRY_DEADLINE.as_secs()
        );
        Err(AppError::CoreUnreachable(format!(
            "core did not become reachable within the {}s bounded retry budget",
            CONFIG_RETRY_DEADLINE.as_secs()
        )))
    })
}
