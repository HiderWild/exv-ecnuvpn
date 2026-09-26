//! Darwin Core/Engine 的编译身份；只输出固定字段，不在运行时读取环境或 Git。
//!
//! 发行版来自同宿主 `tauri/app/tauri.conf.json`，不是 Cargo crate 的版本号。
//! `git_dirty` 仅覆盖已跟踪文件（含暂存改动），明确由 `git_dirty_scope=tracked` 标注。
//! `profile` 默认是 Cargo 提供的继承族 debug/release，自定义名称仅接受显式覆盖。

use std::collections::BTreeMap;

#[must_use]
pub fn release_version() -> &'static str {
    option_env!("EXV_COMPILED_RELEASE_VERSION").unwrap_or("unknown")
}

/// 可直接合并到 core 字段；engine 转为 `(&str, &str)` 后传给 `LogSink::emit`。
#[must_use]
pub fn fields() -> BTreeMap<String, String> {
    [
        ("build.release_version", release_version()),
        (
            "build.git_commit",
            option_env!("EXV_COMPILED_GIT_COMMIT").unwrap_or("unknown"),
        ),
        (
            "build.git_dirty",
            option_env!("EXV_COMPILED_GIT_DIRTY").unwrap_or("unknown"),
        ),
        ("build.git_dirty_scope", "tracked"),
        (
            "build.profile",
            option_env!("EXV_COMPILED_PROFILE").unwrap_or("unknown"),
        ),
        (
            "build.profile_scope",
            option_env!("EXV_COMPILED_PROFILE_SCOPE").unwrap_or("unknown"),
        ),
        (
            "build.target",
            option_env!("EXV_COMPILED_TARGET").unwrap_or("unknown"),
        ),
        (
            "build.opt_level",
            option_env!("EXV_COMPILED_OPT_LEVEL").unwrap_or("unknown"),
        ),
        (
            "build.identity_source",
            option_env!("EXV_COMPILED_IDENTITY_SOURCE").unwrap_or("unknown"),
        ),
        (
            "build.override_fields",
            option_env!("EXV_COMPILED_OVERRIDE_FIELDS").unwrap_or("none"),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}
