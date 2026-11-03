//! L3 本机 aTrust agent 仿真（Win32）。
//!
//! 规格：`docs/research/atrust/2026-10-05-l3-local-agent-simulation-spec.md`。
//!
//! 目标：让门户在**不运行官方客户端**时判定「已安装且在运行」。判定链是「浏览器能连上
//! 本机 agent 的 HTTPS 服务」（`127.0.0.1:54630` 等候选，旧门户走 HTTP `/v1/detect`，
//! 新门户在该判定后走 WebSocket 状态通道）。本 crate 提供：
//!
//! * [`contract`]：静态端点响应与 WS 信封（纯函数，逐条单测锁定）；未知 `/v1/service/*`
//!   显式错误（S2 兜底纪律，杜绝"形似成功"吞票据）；
//! * [`consume`]：S2 真实核销——`/v1/service/reportEnvBeforeLogin`（endpointStrategy →
//!   reportEnv，票据核销点）与 `/v1/service/start`（sessionIdExchange → 会话落地），
//!   网关 I/O 收敛在 [`consume::GatewayConsumer`] 之后（可注入测试）；
//! * [`cert`]：自建 CA + 叶子证书（SAN `IP:127.0.0.1`、`DNS:localhost.sangfor.com.cn`、
//!   `DNS:localhost`）生成与持久化；
//! * [`trust`]：把 CA 装入**当前用户** Root（`certutil -user Root`）与一键卸载；
//! * [`probe`]：启动前断言官方客户端未运行（进程 + 端口）；
//! * [`server`]：本地 HTTPS/WSS 服务与生命周期句柄 [`AgentSim`]（含会话访问器
//!   `AgentSim::session_snapshot`，供 S3 可选注入 WebView）。
//!
//! # 落点与集成理由
//!
//! 本 crate 是独立 Win32 平台库（非特权、无新进程），由 UI 进程（`exv-ui`）在启动时
//! 作为**进程内后台任务**拉起。理由：①门户判定发生在 UI 已运行的认证流程内，进程内
//! 监听即可被浏览器命中；②不新增独立 helper 进程与跨进程状态机，也不新增未知门禁；
//! ③UI 退出即随进程释放端口，无需额外生命周期协调。
//!
//! # 边界
//!
//! * **只操作当前用户 Root 存储**，不触碰机器存储、不改系统代理/hosts/scheme；
//! * 不写任何门户/官方客户端读取的本机文件（`addr.conf`/`SpaMark`/hosts 均不涉及）；
//! * `antiMITM` 若被网关开启则纯软件仿真不可行（硬边界，规格 §6）。

pub mod cert;
pub mod consume;
pub mod contract;
pub mod probe;
pub mod server;
pub mod trust;

pub use consume::{
    ConsumeError, DirectGatewayUnderlay, GatewayConsumer, GatewayTicketConsumer, PreLoginPlan,
    SessionSnapshot, SessionStore, StartRequest,
};
pub use probe::{HostProbe, OfficialClientPresent, SystemProbe};
pub use server::{AgentSim, AgentSimConfig, AgentSimError, DEFAULT_GATEWAY, DEFAULT_PORTS};
pub use trust::{CertutilTrustStore, SystemCommandRunner, TrustStore};

/// 用默认证书目录与 `certutil -user Root` 一键卸载已安装的自建 CA。
///
/// 返回被删除的 SHA-1 指纹（大写十六进制）；无持久化 CA 时返回 `None`。
///
/// # Errors
/// 证书目录不可解析或 `certutil -delstore -user Root` 失败时返回 [`AgentSimError`]。
pub fn uninstall_trusted_ca() -> Result<Option<String>, AgentSimError> {
    let dir = cert::default_cert_dir().ok_or_else(|| {
        AgentSimError::Cert(cert::CertError::Io("无法解析默认证书目录".to_owned()))
    })?;
    let store = CertutilTrustStore::new(SystemCommandRunner);
    trust::uninstall_trusted_ca(&store, &dir).map_err(AgentSimError::Trust)
}

/// 用默认证书目录与 `certutil -user Root` 幂等确保 CA 已装入当前用户 Root。
///
/// 首次安装会弹出 Windows「安全警告」确认框（根证书保护机制），需用户点一次确认。
/// 返回是否本次真正执行了安装。
///
/// # Errors
/// 证书目录不可解析、证书生成失败或 `certutil` 失败时返回 [`AgentSimError`]。
pub fn ensure_trusted_ca() -> Result<bool, AgentSimError> {
    let dir = cert::default_cert_dir().ok_or_else(|| {
        AgentSimError::Cert(cert::CertError::Io("无法解析默认证书目录".to_owned()))
    })?;
    let (material, _) = cert::ensure_material(&dir).map_err(AgentSimError::Cert)?;
    let store = CertutilTrustStore::new(SystemCommandRunner);
    trust::ensure_trusted(&store, &dir, &material).map_err(AgentSimError::Trust)
}
