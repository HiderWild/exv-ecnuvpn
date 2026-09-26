//! kernel — UI<->core 语义层（win32 `kernel/` 的 darwin 移植；win32 wire 契约）。
//!
//! Darwin 前端全量接入 win32 UI 后，Tauri Command 返回值与 Event payload 的类型源
//! 照抄 win32 镜像（proto/exv/v1 生成代码共用 `exv-vpn-wire`；serde snake_case）。
//! 旧 darwin camelCase DTO（IdleSnapshotDto/TransitionEventDto 等）已随旧前端废弃。
//!
//! 政策（与 win32 同源）：
//!   * Command ↔ unary；Event ↔ server-streaming（范式同构、栈分离）。
//!   * core 是唯一语义网关，UI 不直连 engine。
//!   * snapshots/events 永不携带口令、cookie、原始证书或自由文本诊断栈。
//!
//! darwin 差异落点：
//!   * 进程管理与 UDS 认证拨号在 [`core_process`]（guard 唯一放行的进程启动点）；
//!   * 会话生命周期（拉起/恢复/断流收敛）在 [`bootstrap`]；
//!   * `service_control` 已透传 core（W3-4/P4 v1：query=服务代理健康探测实装，
//!     变更动作=core typed 指引拒绝）；`open_external` 为壳侧 AppKit 外部打开
//!     实装（[`external_open`]：`NSWorkspace openURL:`，非进程/插件旁路，修复
//!     关于页 GitHub 链接点击无反应），`trigger_latency_refresh` 仍为平台旁路
//!     no-op（见 `commands.rs`）；
//!   * `tunnel_address` 为壳侧只读观测实装（[`adapter_address`]：getifaddrs 枚举
//!     engine utun，判据与 None 语义对齐 win32 `adapter_address` 本地 seam 契约）。

pub(crate) mod adapter_address;
pub(crate) mod bootstrap;
pub(crate) mod client;
pub(crate) mod commands;
pub(crate) mod core_process;
pub(crate) mod error;
pub(crate) mod events;
pub(crate) mod external_open;
pub(crate) mod logs;
pub(crate) mod pre_uninstall;
pub(crate) mod quick_start;
pub(crate) mod saved_password;
pub(crate) mod state;
pub(crate) mod stats;
pub(crate) mod uninstall;
pub(crate) mod wire;

