//! 连接前检查 Wintun；安装目录副本用于恢复失效的旧路径。

use exv_vpn_win32_resource::{native_error::NativeError, wintun_api::WintunLibrary};
use std::path::{Path, PathBuf};

pub struct PreparedWintun {
    pub library: WintunLibrary,
    pub path: PathBuf,
    pub recovered_from: Option<NativeError>,
}

#[derive(Debug)]
pub struct DependencyError {
    pub configured: PathBuf,
    pub installed: PathBuf,
    pub original: NativeError,
    pub fallback: Option<NativeError>,
}

impl DependencyError {
    pub fn actionable_error(&self) -> &NativeError {
        // 优先给出安装副本自身的损坏/权限问题；副本仅缺失时保留原始原因。
        self.fallback
            .as_ref()
            .filter(|e| !matches!(e.code, 2 | 3))
            .unwrap_or(&self.original)
    }
}

pub fn prepare_from(
    configured: &Path,
    installed: &Path,
    on_recovery: impl FnOnce(&NativeError, &Path),
) -> Result<PreparedWintun, DependencyError> {
    let original = match WintunLibrary::load(configured) {
        Ok(library) => {
            return Ok(PreparedWintun {
                library,
                path: configured.into(),
                recovered_from: None,
            });
        }
        Err(error) => error,
    };
    let fallback = if configured != installed {
        on_recovery(&original, installed);
        match WintunLibrary::load(installed) {
            Ok(library) => {
                return Ok(PreparedWintun {
                    library,
                    path: installed.into(),
                    recovered_from: Some(original),
                });
            }
            Err(error) => Some(error),
        }
    } else {
        None
    };
    Err(DependencyError {
        configured: configured.into(),
        installed: installed.into(),
        original,
        fallback,
    })
}
