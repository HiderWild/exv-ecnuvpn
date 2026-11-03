//! Darwin 服务修订号观测面（`exv-vpn-darwin-ipc::service_revision` 的 Core 读侧）。
//!
//! 修订号作为二进制元数据嵌在 engine 可执行文件的 `__TEXT,__exv_revision`
//! section；读侧不运行进程、不发请求，解析 Mach-O 头即可拿到。
//! 已装文件与当前运行进程是不同事实；本模块只报告安装文件，运行版本由实际
//! Engine 的构建身份日志核验。服务更新会释放旧控制会话，下一次连接重新拨号。
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
//! 修订相等只证明计数器相等；完整安装一致性另比较 Engine 与服务代理文件摘要。
//! `installed` 缺席/无 section = 未安装或装的旧版（尚无元数据的 engine）。
//!
//! 修订号读取只检查文件头与目标 section；摘要诊断会完整读取 Engine 和服务代理，
//! 因此版本视图应在阻塞 worker 中采集，不阻塞异步执行线程。
//!
//! ## 不做什么
//!
//! * **不是升级决策器**：只读观测，不触发安装/启动/覆盖副作用。
//! * **非协议协商**：wire 冻结，不进任何协议字段。
//! * **不含 win32 侧**。
//! * **不参与健康五态判定**：`derive_health` 不读修订事实。

use std::{path::Path, io::Read};
use sha2::{Digest, Sha256};

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

/// Darwin 壳侧的服务版本视图；只读本机文件，不扩展认证或 Common wire 契约。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceBundleStatus {
    /// 当前 App 期望修订。
    pub expected: u32,
    /// 随附 Engine 修订。
    pub bundled: Option<u32>,
    /// 已装 Engine 修订；不声称是正在运行进程的版本。
    pub installed: Option<u32>,
    /// Engine 与服务代理两件文件是否都与 App 相同；无法读取时未知。
    pub matches_bundle: Option<bool>,
}

fn file_digest(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16384];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 { return Some(digest.finalize().to_vec()); }
        digest.update(&buffer[..count]);
    }
}

fn bundle_matches(bundled: &Path, installed: &Path) -> Option<bool> {
    let mut unknown = false;
    for name in [INSTALLED_ENGINE_BINARY_NAME, "exv-vpn-darwin-service-agent"] {
        match (file_digest(&bundled.join(name)), file_digest(&installed.join(name))) {
            (Some(left), Some(right)) if left != right => return Some(false),
            (Some(_), Some(_)) => {},
            _ => unknown = true,
        }
    }
    if unknown { None } else { Some(true) }
}

/// 同目录随附组件与正式安装组件的只读比较。调用者应在阻塞 worker 中执行。
#[must_use]
pub fn service_bundle_status() -> ServiceBundleStatus {
    let engine = bundled_engine_path();
    let facts = collect_revision_facts(engine.as_deref());
    ServiceBundleStatus {
        expected: facts.expected, bundled: facts.bundled, installed: facts.installed,
        matches_bundle: engine.as_deref().and_then(Path::parent)
            .and_then(|dir| bundle_matches(dir, Path::new(SERVICE_AGENT_INSTALL_DIR))),
    }
}

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
