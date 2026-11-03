//! T1（darwin 连接基建整改计划）：core 会话终结时的管线处置策略位。
//!
//! 背景：现状是 C++ 时代教义"core 崩则清"——core 连接 EOF 或心跳超时无条件触发
//! 管线逆序 teardown（oneshot 形态再退出进程）。keep 语义（v6 计划，吸收自 Windows
//! trace-infra）要求连接所有权下沉 engine：core 会话终结时**检测保留**（状态迁移
//! 与路径故障兜底不变），**退出/拆隧道动作改按策略**。
//!
//! 本模块只定义策略枚举与判定；策略值的运行期来源（连接信封/trace）由后续任务
//! 接线，T1 阶段生产默认恒 [`CoreExitPolicy::Teardown`]——即现状行为逐字节保持，
//! 策略位是 T2（service 形态 keep+采纳）的前置接缝。

/// core 会话终结（EOF/心跳超时；不含显式停止与 SIGTERM）时的管线处置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CoreExitPolicy {
    /// 现状默认：终结触发进入 W1 统一收敛（逆序 teardown；oneshot 形态随后退出）。
    #[default]
    Teardown,
    /// keep：终结触发不收敛 teardown，管线跨 core 会话存续（service 形态携带
    /// runtime 回 accept 等待下一会话；oneshot 形态进入持有等待，显式信号仍是
    /// 唯一既有终结出口）。检测（EOF 事实、心跳时间戳）保留用于状态迁移。
    KeepTunnel,
}
