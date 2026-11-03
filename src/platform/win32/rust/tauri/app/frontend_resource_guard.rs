use std::path::{Path, PathBuf};
use tauri_utils::{config::{Config, FrontendDist}, platform::Target};

pub fn validate_resources(
    app_dir: &Path,
    target: Target,
    overlay: Option<&str>,
    require_entry: bool,
) -> Result<PathBuf, String> {
    let (mut value, _) = tauri_utils::config::parse::read_from(target, app_dir)
        .map_err(|error| error.to_string())?;
    if let Some(overlay) = overlay {
        let overlay = serde_json::from_str(overlay).map_err(|error| error.to_string())?;
        json_patch::merge(&mut value, &overlay);
    }
    let config: Config = serde_json::from_value(value).map_err(|error| error.to_string())?;
    // 使用框架实际反序列化类型，而非猜测字符串是否像文件路径。
    // Windows盘符字符串会先匹配Url；这种配置不会嵌入任何前端资产。
    let relative = match config.build.frontend_dist {
        Some(FrontendDist::Directory(path))
            if !path.has_root()
                && !path.components().any(|part| matches!(part, std::path::Component::Prefix(_))) => path,
        _ => return Err("EXV frontendDist must be a relative directory; URLs, absolute paths and file lists cannot enter the Windows product".into()),
    };
    let directory = app_dir.join(relative);
    if require_entry && !directory.join("index.html").is_file() {
        return Err(format!("EXV embedded frontend entry is missing: {}", directory.join("index.html").display()));
    }
    Ok(directory)
}
