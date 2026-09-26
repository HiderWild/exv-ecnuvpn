//! Darwin 服务修订号观测面（`exv-vpn-darwin-ipc::service_revision` 的 Core 读侧）。
//!
//! 修订号作为二进制元数据嵌在 engine 可执行文件的 `__TEXT,__exv_revision`
//! section；读侧**不运行进程、不发请求**，解析 Mach-O 头即可拿到。因为 launchd
//! 总是从固定安装路径启动 engine，「已装文件」的修订号同时就是「正在/下一次
//! 运行」的修订号（文件与运行坍缩成一个事实），不需要运行期叶、不需要 socket
//! 门控。
//!
//! 与 [`crate::service_status`] 的关系：五态健康映射不含修订号——修订是正交的
//! 诊断事实，不参与健康判定、不改变 wire 词汇。本模块只做无特权只读观测
//! （fail-soft：任一读失败按「未知」处理）。
//!
//! 三个字段（真源见 ipc 模块头注释）：
//!   * `expected`：编译期常量（Core 与 engine 同一编译期修订）；
//!   * `bundled`：随附 engine（Core 同目录兄弟二进制）的嵌入式修订号；
//!   * `installed`：已安装 engine（固定安装路径）的嵌入式修订号。
//!
//! 覆盖验证读法：`bundled == installed == expected` → 装的就是本构建这版。
//! `installed` 缺席/无 section = 未安装或装的旧版（尚无元数据的 engine）。
//!
//! 观测成本：只读每个文件头 4KiB + 目标 section 4 字节，与二进制大小无关
//! （debug 上百 MB 也不受影响），无阻塞线程顾虑。
//!
//! ## 不做什么
//!
//! * **不是升级决策器**：只读观测，不触发安装/启动/覆盖副作用。
//! * **非协议协商**：wire 冻结，不进任何协议字段。
//! * **不含 win32 侧**。
//! * **不参与健康五态判定**：`derive_health` 不读修订事实。

use std::path::Path;

use exv_vpn_darwin_ipc::service_revision::{
    DARWIN_SERVICE_REVISION, read_embedded_revision,
};

/// 服务代理固定安装目录（已安装 engine 的父目录；镜像服务代理
/// `platform.rs::SERVICE_AGENT_INSTALL_DIR`——先例同模式 Core 侧双写，代理侧因零依赖
/// 约束保留其字面量）。
const SERVICE_AGENT_INSTALL_DIR: &str = "/Library/Application Support/EXV/ServiceAgent";
/// 已安装 engine 二进制文件名（镜像服务代理 `platform.rs` 的
/// `ENGINE_BINARY_PATH` 目录内名）。
const INSTALLED_ENGINE_BINARY_NAME: &str = "exv-vpn-darwin-engine";

/// 服务修订观测快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServiceRevisionFacts {
    /// 本构建期望的服务修订号（编译期常量）。
    pub(crate) expected: u32,
    /// 随附 engine（Core 同目录兄弟）的嵌入式修订号；缺失/非 Mach-O → `None`。
    pub(crate) bundled: Option<u32>,
    /// 已安装 engine（固定安装路径）的嵌入式修订号；未安装/旧版无元数据 → `None`。
    pub(crate) installed: Option<u32>,
}

impl ServiceRevisionFacts {
    /// 观测整体不可得（如 worker 异常）时的保守回落：`expected` 仍来自编译期真源，
    /// 其余全「未知」（绝不伪造观测）。
    #[must_use]
    pub(crate) fn unavailable() -> Self {
        Self {
            expected: DARWIN_SERVICE_REVISION,
            bundled: None,
            installed: None,
        }
    }

    /// 人读摘要（Query message 追加段；不含路径，与既有 message 风格一致）。
    #[must_use]
    pub(crate) fn summarize(&self) -> String {
        let installed = match self.installed {
            Some(value) => value.to_string(),
            None => "缺失或旧版".to_string(),
        };
        let bundled = match self.bundled {
            Some(value) => value.to_string(),
            None => "未知".to_string(),
        };
        let verdict = match (self.bundled, self.installed) {
            (Some(bundled), Some(installed)) if bundled == installed => "已装版本与本构建一致",
            (Some(_), Some(_)) => "已装版本与本构建不一致",
            (None, _) => "随附组件缺失",
            (_, None) => "未检测到已装组件",
        };
        format!(
            "服务版本：期望 {}，随附 {}，已装 {}（{}）",
            self.expected, bundled, installed, verdict
        )
    }
}

/// 生产观测入口：读随附（Core 同目录兄弟二进制）与已安装 engine 的嵌入式修订号。
pub(crate) fn collect_revision_facts(bundled_engine: Option<&Path>) -> ServiceRevisionFacts {
    let bundled = bundled_engine.and_then(read_embedded_revision);
    let installed = read_embedded_revision(
        &Path::new(SERVICE_AGENT_INSTALL_DIR).join(INSTALLED_ENGINE_BINARY_NAME),
    );
    ServiceRevisionFacts {
        expected: DARWIN_SERVICE_REVISION,
        bundled,
        installed,
    }
}

/// 随附 engine 的解析（`resolve_service_agent_paths().engine` 的薄别名，供 Query 面
/// 单点调用；开发检出与 bundle 布局下均为 Core 同目录的 `exv-vpn-darwin-engine`）。
pub(crate) fn bundled_engine_path() -> Option<std::path::PathBuf> {
    crate::service_lifecycle::resolve_service_agent_paths().engine
}
