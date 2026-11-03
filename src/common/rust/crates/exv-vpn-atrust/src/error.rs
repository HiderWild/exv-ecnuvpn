//! 全部 typed 错误与唯一的脱敏约定。
//!
//! # 脱敏约定（本 crate 允许提及敏感材料的唯一入口）
//!
//! 敏感材料指：Cookie（含 CAS/SSO Cookie）、ticket、登录结果（一次性 code/token）、密码、
//! 证书或公钥材料、会话 `sid`、每流 connect token，以及任何由它们派生的片段。
//!
//! **当前边界（与实际实现一致，不夸大）**：[`Secret`] 是 T04 认证材料与 T06 会话材料的指定
//! 入口，但 T03 已落地的线格式对象**没有**走它：
//! [`crate::transport::l3::codec::DataFrame::token`]、认证请求/响应载荷与
//! [`crate::session::PendingUnit::payload`] 仍是可 `Clone` 的裸 `Vec<u8>`，不进入 [`Secret`]、
//! 不做零化——协议重试（认证重试、数据面重连重发）需要克隆这些字节，克隆副本也不受零化约束。
//! 它们只满足下面的规则 5（`Debug` 只打印长度），内存中的明文不受 [`Secret`] 的 `Drop` 保护。
//! 需要更强保证时必须先改数据流本身（T04/T06），不要指望本文件的约定自动覆盖它们。
//!
//! 规则：
//!
//! 1. 敏感材料只能放进 [`Secret`]；它的 `Display` 与 `Debug` 只输出
//!    `<redacted:len=N>`，`Drop` 时零化。
//! 2. 错误消息**不得**包含敏感材料的任何部分，也不得包含其前缀、哈希、截断片段或明文；
//!    需要区分“空值/长度异常”时只给长度（[`redacted_sized`]），需要说明阶段时只给阶段
//!    名（例如 [`UnderlayErrorKind`] 的稳定代码）。
//! 3. 本文件声明的错误类型不携带自由文本原因。宿主把自身故障映射到
//!    [`UnderlayErrorKind`] / [`UnderlayError`] 时同样受此约束：症状用固定词汇表达，
//!    上下文（主机名、端口、用途）由调用点决定是否记录，不塞进错误。
//! 4. 协议/资源字段（端口原文、CIDR 原文、域名、`AccessModel` 取值）**不是**凭据材料，
//!    可以按原文出现在跳过记录与错误里；它们用于解释“为什么这条资源没进目录”。
//! 5. payload 字节不得进入 `Debug`：见 [`crate::transport::l3::codec::DataFrame`] 与
//!    [`crate::session::PendingUnit`] 的手写 `Debug`（只打印长度）。
//!
//! 违反该约定的错误消息属于缺陷，`tests/error_redaction.rs` 会对这些性质做断言。

use std::fmt;

use thiserror::Error;
use zeroize::Zeroize;

use crate::transport::ConnectPurpose;

/// 敏感材料的占位符，不含长度。
pub const REDACTED: &str = "<redacted>";

/// 只保留长度的脱敏占位符：`<redacted:len=N>`。
///
/// 仅在确实需要区分“空值 / 长度异常”的诊断场合使用；不要用它来编码可反推内容的摘要。
#[must_use]
pub fn redacted_sized(len: usize) -> String {
    format!("<redacted:len={len}>")
}

/// 敏感材料的指定包装类型。
///
/// 该类型刻意**不实现** `Clone`：敏感材料不应被无意识地复制传播。需要取值时用
/// [`Secret::expose`]，调用点必须保证该值不进入日志、错误消息、前端持久状态或普通配置。
///
/// 当前使用范围：T03 的线格式与队列**没有**使用它（见本模块头部“当前边界”）；T04 的认证
/// 材料与 T06 的会话材料必须经它传递，后续任务不得再把新的敏感材料塞进裸 `Vec<u8>`。
pub struct Secret(String);

impl Secret {
    /// 包装一个敏感值。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 取出明文。调用点负责不再传播该引用。
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// 长度（字节）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redacted_sized(self.0.len()))
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redacted_sized(self.0.len()))
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// 本 crate 的错误总入口。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// 资源目录解析被拒绝。
    #[error("aTrust resource catalog: {0}")]
    Resource(#[from] ResourceError),
    /// L3 帧被拒绝。
    #[error("aTrust L3 frame: {0}")]
    Codec(#[from] L3CodecError),
    /// 会话状态被拒绝。
    #[error("aTrust session: {0}")]
    Session(#[from] SessionError),
    /// 宿主出口端口返回的失败。
    #[error("aTrust underlay: {0}")]
    Underlay(#[from] UnderlayError),
}

/// 本 crate 的结果别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 资源目录解析错误。
///
/// 只有“整份正文无法使用”才是错误；单条资源不可用属于带原因的跳过，见
/// [`crate::resource::SkippedEntry`]。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResourceError {
    /// 正文超过输入上限，在解析前拒绝，避免无界分配。
    #[error("resource body of {bytes} bytes exceeds the {limit}-byte cap")]
    BodyTooLarge {
        /// 实际字节数。
        bytes: usize,
        /// 允许的上限。
        limit: usize,
    },
    /// JSON 解析失败。只保留位置信息，不包含原文片段。
    #[error("resource body is not valid JSON (line {line}, column {column})")]
    Json {
        /// 行号（1 起）。
        line: usize,
        /// 列号（1 起）。
        column: usize,
    },
}

/// L3 帧编码/解码错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum L3CodecError {
    /// 帧首字节不是本实现支持的版本。
    #[error("L3 frame version 0x{actual:02x} is not the supported version 0x{expected:02x}")]
    UnsupportedVersion {
        /// 实际读到的版本字节。
        actual: u8,
        /// 本实现支持的版本字节。
        expected: u8,
    },
    /// 未知命令字节。
    #[error("unknown L3 frame command 0x{command:02x}")]
    UnknownCommand {
        /// 实际读到的命令字节。
        command: u8,
    },
    /// 声明的帧长度超过上限；该判断在分配/等待载荷之前完成。
    #[error("L3 frame of {declared} bytes exceeds the {limit}-byte limit")]
    OversizeFrame {
        /// 声明的总字节数。
        declared: usize,
        /// 允许的上限。
        limit: usize,
    },
    /// 定长帧声明了不该有的载荷。
    #[error("L3 frame 0x{command:02x} must carry no payload but declares {declared} bytes")]
    UnexpectedPayloadLength {
        /// 命令字节。
        command: u8,
        /// 声明的载荷长度。
        declared: usize,
    },
    /// connect token 超长。
    #[error("L3 connect token of {len} bytes exceeds the {limit}-byte limit")]
    TokenTooLong {
        /// 实际长度。
        len: usize,
        /// 允许的上限。
        limit: usize,
    },
    /// 编码数据请求时携带的包数不是「一包一消息」契约要求的 1。
    ///
    /// 「一包一消息」是设计 §3.1 固化的 EXV 写出契约：每个 IP 包单独成帧，编码侧不接受
    /// 多包帧（解码侧对入站多包帧的容忍见 [`crate::transport::l3::codec`] 的模块文档）。
    #[error(
        "L3 data request carries {len} packet(s); the one-packet-per-message contract requires exactly 1"
    )]
    PacketCountNotOne {
        /// 实际包数。
        len: usize,
    },
    /// 流结束时缓冲区里还留着不完整的帧。
    #[error("L3 stream ended with {buffered} buffered byte(s) that do not form a complete frame")]
    IncompleteAtEof {
        /// 残留字节数。
        buffered: usize,
    },
    /// 载荷不是可用的 IPv4 包。
    #[error("L3 IPv4 payload is malformed: {reason}")]
    MalformedPacket {
        /// 固定词汇的原因（不含 payload 内容）。
        reason: &'static str,
    },
    /// 数据面为 IPv4-only，收到 IPv6 包。
    #[error("L3 data plane is IPv4-only; an IPv6 packet was offered")]
    Ipv6Unsupported,
}

/// 会话状态错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    /// 会话已关闭，不再接受新工作。
    #[error("session {session} is closed and accepts no new work")]
    Closed {
        /// 会话 ID。
        session: u64,
    },
    /// 结果属于另一个会话，不能进入当前会话。
    #[error("result belongs to session {belongs_to} and cannot enter session {offered_to}")]
    StaleResult {
        /// 结果所属的会话 ID。
        belongs_to: u64,
        /// 试图消费该结果的会话 ID。
        offered_to: u64,
    },
    /// 入队会超过会话排队的显式预算（有界背压）：本次入队被拒绝，队列状态不变。
    #[error(
        "session {session} queue would hold {would_hold_units} unit(s) / {would_hold_bytes} byte(s), above the {max_units}-unit / {max_bytes}-byte budget"
    )]
    QueueBudgetExceeded {
        /// 会话 ID。
        session: u64,
        /// 接受本次入队后队列将持有的条数。
        would_hold_units: usize,
        /// 接受本次入队后队列将持有的字节数。
        would_hold_bytes: usize,
        /// 条数预算（见 [`crate::session::QueueBudget`]）。
        max_units: usize,
        /// 字节预算（见 [`crate::session::QueueBudget`]）。
        max_bytes: usize,
    },
}

/// 宿主出口失败的固定症状词汇。
///
/// 该词汇表是封闭的：任何自由文本原因都可能携带敏感材料，因此宿主不得扩展它来传递
/// 原始错误字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlayErrorKind {
    /// 解析失败（含宿主解析器不可用）。
    ResolveFailed,
    /// 连接被拒绝。
    ConnectRefused,
    /// 连接超时。
    ConnectTimedOut,
    /// 其它连接失败（不可达、宿主出口缺失、接口不可用等）。
    ConnectFailed,
    /// 建连或解析在完成前被取消。
    Cancelled,
}

impl UnderlayErrorKind {
    /// 稳定机器码，供宿主映射 UI 文案。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ResolveFailed => "resolve-failed",
            Self::ConnectRefused => "connect-refused",
            Self::ConnectTimedOut => "connect-timed-out",
            Self::ConnectFailed => "connect-failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for UnderlayErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// 宿主出口失败；Display 保持固定词汇，原捕获事实仅以有界脱敏字段提供。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("underlay {purpose} target {kind}")]
pub struct UnderlayError {
    kind: UnderlayErrorKind,
    purpose: ConnectPurpose,
    operation: Option<&'static str>,
    io_kind: Option<std::io::ErrorKind>,
    raw_os_error: Option<i32>,
    detail: Option<String>,
}

impl UnderlayError {
    /// 构造。
    #[must_use]
    pub fn new(kind: UnderlayErrorKind, purpose: ConnectPurpose) -> Self {
        Self {
            kind,
            purpose,
            operation: None,
            io_kind: None,
            raw_os_error: None,
            detail: None,
        }
    }

    /// 在原 I/O 捕获点保留原字段，不推断缺省系统码。
    #[must_use]
    pub fn from_io(
        kind: UnderlayErrorKind,
        purpose: ConnectPurpose,
        operation: &'static str,
        error: &std::io::Error,
    ) -> Self {
        Self {
            kind,
            purpose,
            operation: Some(operation),
            io_kind: Some(error.kind()),
            raw_os_error: error.raw_os_error(),
            detail: Some(crate::auth::transport_error::io_detail(error)),
        }
    }

    /// 非 I/O 原因（如解析端口文本或期限耗尽）不伪造 I/O 类别或原生码。
    #[must_use]
    pub fn with_detail(
        kind: UnderlayErrorKind,
        purpose: ConnectPurpose,
        operation: &'static str,
        detail: &str,
    ) -> Self {
        Self {
            kind,
            purpose,
            operation: Some(operation),
            io_kind: None,
            raw_os_error: None,
            detail: Some(crate::auth::transport_error::text_detail(detail)),
        }
    }

    #[must_use]
    pub fn operation(&self) -> Option<&'static str> {
        self.operation
    }
    #[must_use]
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_kind
    }
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.raw_os_error
    }
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// 症状。
    #[must_use]
    pub fn kind(&self) -> UnderlayErrorKind {
        self.kind
    }

    /// 该失败发生在哪类目标上。
    #[must_use]
    pub fn purpose(&self) -> ConnectPurpose {
        self.purpose
    }
}

