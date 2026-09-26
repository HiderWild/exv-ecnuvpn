//! T1 latency design v2 — 手动刷新延迟标记（前端「立即刷新延迟」）。
//!
//! 该模块是本地**旁路**：wire 零变更（延迟仍经既有 `RuntimeSnapshot.stats.latency_ms`
//! 到达前端），且 host `kernel_control_service.rs` / engine `grpc_server.rs` 在 T1
//! 文件边界之外（S1/S2 领地），无法新增「立即 ping」RPC。因此前端命令写一个本地
//! 标记文件（内容 = epoch 毫秒），engine 数据面探测循环（`data_plane::ProbeState`）
//! 每秒轮询该文件，发现新值即立即执行一次隧道 ping → `registry.record_latency`。
//!
//! 路径与 engine `tunnel_runtime.rs::LATENCY_REFRESH_FILE` 保持一致（两侧独立
//! 硬编码；tauri 不依赖 engine/config crate）：
//!   `EXV_CONFIG_DIR` → `%USERPROFILE%\.exv` → `$HOME/.exv` → `./.exv`。
//!
//! service 模式下 engine 虽以 SYSTEM 运行，但服务安装会把当前用户的配置目录固化进
//! SCM 启动参数；因此 engine 与这里写标记的用户目录保持一致。服务仍是安装用户绑定的
//! 机器级服务，切换 Windows 用户时应由当前用户执行「修复」重新登记配置目录。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 与 engine `tunnel_runtime.rs::LATENCY_REFRESH_FILE` 同名的标记文件名。
const LATENCY_REFRESH_FILE: &str = "latency_refresh";

/// 解析配置目录（镜像 `exv-vpn-win32-config::paths::config_dir` 的查找顺序；tauri
/// 不依赖该 crate，故本地复刻同一语义）。
#[must_use]
fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EXV_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(profile).join(".exv");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home).join(".exv");
    }
    PathBuf::from(".exv")
}

/// 写一次手动刷新标记（内容 = 当前 epoch 毫秒；engine 探测循环轮询到新值即立即
/// 执行一次隧道 ping）。
///
/// # Errors
/// 目录创建或写文件失败 → `String`（UI 显示为内部错误，不阻塞连接）。
pub fn write_refresh_marker() -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("latency marker mkdir: {e}"))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let path = dir.join(LATENCY_REFRESH_FILE);
    std::fs::write(&path, ts.to_string()).map_err(|e| format!("latency marker write: {e}"))
}

// ---------------------------------------------------------------------------
// 单元测试：路径派生 + 标记写入可读。环境变量测试用互斥锁串行化——`EXV_CONFIG_DIR`
// 是进程级全局态，并行修改会互相干扰。
// ---------------------------------------------------------------------------
