
//! 控制面管道名 / authority 互斥名的**单一事实源**（T11 管道名统一，D12）。
//!
//! 历史上 oneshot 管道按拉起 core 的 PID 实例化（`\\.\pipe\exv-engine-{pid}-c`）、
//! 服务形态用裸常量名 `\\.\pipe\exv-engine-service-c`——两者同属 C++ 时代"engine 随
//! core 生灭"教义下的配对绑定系统。连接所有权下沉 engine 后（本计划 v2 起），
//! "孤儿 engine 对新 core 不可发现"与"单 engine 互斥"成为硬需求，per-PID 名翻转为债：
//! 统一为**每用户 sid 段常量名**——同一用户的新 core 从自己的 SID 即可算出与现存
//! engine 相同的名字（采纳免发现记录），不同用户（A/B 并行）天然互不串扰（复审 F2）。
//!
//! - [`control_pipe_name`]：`\\.\pipe\exv-engine-c-{user-sid}`（服务/oneshot 两形态
//!   同名；首实例 `FILE_FLAG_FIRST_PIPE_INSTANCE` 由建管路径纪律化，见
//!   `named_pipe_io` 与 engine `grpc_transport`——防抢注 + **每用户单 engine 互斥**）。
//! - [`authority_name`]：`Local\exv-engine-{user-sid}-authority`（authority 互斥随
//!   sid 段迁移；`Local\` 会话作用域天然每用户一份，无需 `Global\`，复审 F10）。
//! - [`validate_user_sid`]：SID 文法校验的**canonical** 实现（`S-1-` 前缀 + 仅数字与
//!   连字符 + 长度上限）。SID 段直接拼入内核对象名，必须杜绝路径注入/名字走私
//!   （`..`、分隔符、任意名）；engine `connection_trace` 的同款校验自此复用本实现。
//!
//! 升级过渡属性（一次切换，无兼容期）：旧版本 engine 持旧名（per-PID 名 / 裸常量
//! 服务名），新版本持新名——**跨版本名字不同，首实例互不冲突**；同名冲突只剩
//! "同版本残留 engine 仍在跑"一种，由建管侧的有界 bootstrap 重试 + 诚实失败收口
//! （见 engine `grpc_transport`）。

use crate::peer_auth;

/// 用户 SID 文法长度上限（Windows SID 理论上限 = 15 个 sub-authority ≈ 184 字符；
/// 与 engine `connection_trace` 历史上限一致）。
pub const USER_SID_MAX_CHARS: usize = 184;

/// SID 文法校验失败（结构非法：前缀不符 / 含数字与连字符以外的字符 / 长度越界）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUserSid;

impl std::fmt::Display for InvalidUserSid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid user SID (expect S-1- prefix, digits and hyphens only, <= {USER_SID_MAX_CHARS} chars)"
        )
    }
}

impl std::error::Error for InvalidUserSid {}

/// 校验用户 SID 文法（canonical 实现，供名字构造与 engine trace 复用）。
///
/// 只看字符集与长度，不验证 SID 是否真实存在（OS 侧 `current_user_sid` 产出的 SID
/// 恒合法；本校验防的是外部注入值被拼入内核对象名/文件名）。
#[must_use]
pub fn is_valid_user_sid(user_sid: &str) -> bool {
    const PREFIX: &str = "S-1-";
    user_sid.len() > PREFIX.len()
        && user_sid.len() <= USER_SID_MAX_CHARS
        && user_sid.starts_with(PREFIX)
        && user_sid[PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-')
}

/// 校验用户 SID 文法（[`is_valid_user_sid`] 的 Result 形态）。
///
/// # Errors
/// [`InvalidUserSid`]：结构非法。
pub fn validate_user_sid(user_sid: &str) -> Result<(), InvalidUserSid> {
    if is_valid_user_sid(user_sid) {
        Ok(())
    } else {
        Err(InvalidUserSid)
    }
}

/// engine 控制面 Named Pipe 常量名（每用户 sid 段，D12）：
/// `\\.\pipe\exv-engine-c-{user-sid}`。
///
/// 服务形态（安装时以安装用户 SID 注册启动参数）与 oneshot 形态（core 以自己的 SID
/// 构造 spawn 参数）共用同一函数——**采纳友好**：新 core 从自己的 SID 算出与现存
/// engine 相同的名字，无需任何发现记录。
///
/// # Errors
/// [`InvalidUserSid`]：sid 非法（名字段只允许合法 SID 字符集）。
pub fn control_pipe_name(user_sid: &str) -> Result<String, InvalidUserSid> {
    validate_user_sid(user_sid)?;
    Ok(format!(r"\\.\pipe\exv-engine-c-{user_sid}"))
}

/// authority 互斥名（每用户 sid 段，D12）：`Local\exv-engine-{user-sid}-authority`。
///
/// `Local\` 作用域 = 当前会话/登录态：每用户（每会话）天然一份，无需 `Global\`
///（复审 F10——服务与 oneshot 都不需要跨会话争用同一 authority）。
///
/// # Errors
/// [`InvalidUserSid`]：sid 非法。
pub fn authority_name(user_sid: &str) -> Result<String, InvalidUserSid> {
    validate_user_sid(user_sid)?;
    Ok(format!(r"Local\exv-engine-{user_sid}-authority"))
}

/// 当前用户的控制面管道名（拨号侧便捷函数：host 探活/连接 service engine 用）。
///
/// 服务以安装用户注册名字；host（core）与安装用户同拓扑（runas 提权保持同一用户
/// SID），故"当前用户 SID"即正确名字段。SID 不可解析（极罕见）→ `None`（调用方
/// 按各自语义保守处理：探活 false / 连接诚实失败）。
#[must_use]
pub fn control_pipe_name_for_current_user() -> Option<String> {
    let sid = peer_auth::current_user_sid()?;
    control_pipe_name(&sid).ok()
}

// ---------------------------------------------------------------------------
// 单元测试：sid 段格式 + 非法 sid 拒绝 + 每用户隔离（B 用户并行，验收 12 的单测投影）。
// ---------------------------------------------------------------------------
