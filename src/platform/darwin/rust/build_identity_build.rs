//! core/engine 共用的构建身份采集，不修改发行版或打包参数。
//!
//! 覆盖语义：构建命令可显式设置 EXV_BUILD_RELEASE_VERSION、EXV_BUILD_GIT_COMMIT、
//! EXV_BUILD_GIT_DIRTY、EXV_BUILD_PROFILE、EXV_BUILD_TARGET。设置即覆盖自动值；
//! 空值、非法值及显式 unknown 都记录 unknown，绝不悄悄回退。日志列出覆盖字段名。
//! PROFILE 是 Cargo 的 debug/release 继承族；精确自定义名称由 EXV_BUILD_PROFILE 声明。
//! Git 只读取本地 HEAD 和已跟踪改动；无仓库或命令失败时记录 unknown。
//! 追踪源码、HEAD、index、当前 ref 和 packed-refs；不扫描 target，不注入墙钟时间，
//! 保持相同源码/环境下输出稳定，以便 sccache 重用。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION_PATH: &str = "src/platform/darwin/rust/tauri/app/tauri.conf.json";
const OVERRIDES: [(&str, &str); 5] = [
    ("RELEASE_VERSION", "release_version"),
    ("GIT_COMMIT", "git_commit"),
    ("GIT_DIRTY", "git_dirty"),
    ("PROFILE", "profile"),
    ("TARGET", "target"),
];

pub fn run() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let root = manifest.as_deref().and_then(repository_root);
    let mut values = automatic_values(root.as_deref());
    values.insert(
        "PROFILE",
        validated("PROFILE", std::env::var("PROFILE").ok().as_deref()),
    );
    values.insert(
        "TARGET",
        validated("TARGET", std::env::var("TARGET").ok().as_deref()),
    );
    values.insert(
        "OPT_LEVEL",
        validated("OPT_LEVEL", std::env::var("OPT_LEVEL").ok().as_deref()),
    );
    let overrides: BTreeMap<&str, String> = OVERRIDES
        .iter()
        .filter_map(|(key, _)| {
            let name = format!("EXV_BUILD_{key}");
            println!("cargo:rerun-if-env-changed={name}");
            std::env::var_os(name).map(|value| (*key, value.to_str().unwrap_or("").to_owned()))
        })
        .collect();
    apply_overrides(&mut values, &overrides);
    values.insert(
        "PROFILE_SCOPE",
        if overrides.contains_key("PROFILE") {
            "explicit_override"
        } else {
            "cargo_inheritance"
        }
        .to_owned(),
    );
    for (key, value) in values {
        println!("cargo:rustc-env=EXV_COMPILED_{key}={value}");
    }
    // 即使源码打包不含 Git，也追踪同一发行文件和共享 helper。
    if let Some(manifest) = manifest {
        let host_root = manifest.join("../..");
        watch(&host_root.join("build_identity.rs"));
        watch(&host_root.join("build_identity_build.rs"));
        watch(&host_root.join("tauri/app/tauri.conf.json"));
    }
    if let Some(root) = root {
        for path in git_watch_paths(&root) {
            watch(&path);
        }
    }
}

fn watch(path: &Path) {
    if let Some(path) = path.to_str().filter(|path| !path.contains(['\n', '\r'])) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn repository_root(manifest: &Path) -> Option<PathBuf> {
    manifest
        .ancestors()
        .find(|path| path.join(VERSION_PATH).is_file())
        .map(Path::to_path_buf)
}

fn release_from_json(text: &str) -> String {
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => return "unknown".to_owned(),
    };
    validated(
        "RELEASE_VERSION",
        json.get("version").and_then(serde_json::Value::as_str),
    )
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn valid_version(value: &str) -> bool {
    if !valid_token(value) {
        return false;
    }
    let (without_build, build) = value
        .split_once('+')
        .map_or((value, None), |(core, build)| (core, Some(build)));
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    let valid_identifiers = |part: &str| {
        part.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    };
    if build.is_some_and(|part| !valid_identifiers(part))
        || prerelease.is_some_and(|part| !valid_identifiers(part))
    {
        return false;
    }
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn validated(key: &str, raw: Option<&str>) -> String {
    let value = raw.unwrap_or("unknown").trim();
    let valid = match key {
        "RELEASE_VERSION" => valid_version(value),
        "GIT_COMMIT" => {
            matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }
        "GIT_DIRTY" => matches!(value, "true" | "false"),
        "PROFILE" | "TARGET" => valid_token(value),
        "OPT_LEVEL" => matches!(value, "0" | "1" | "2" | "3" | "s" | "z"),
        _ => false,
    };
    if valid {
        value.to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn apply_overrides(
    values: &mut BTreeMap<&'static str, String>,
    overrides: &BTreeMap<&str, String>,
) {
    let mut overridden = Vec::new();
    for (key, label) in OVERRIDES {
        if let Some(value) = overrides.get(key) {
            values.insert(key, validated(key, Some(value)));
            overridden.push(label);
        }
    }
    values.insert(
        "IDENTITY_SOURCE",
        if overridden.is_empty() {
            "auto"
        } else {
            "explicit_override"
        }
        .to_owned(),
    );
    values.insert(
        "OVERRIDE_FIELDS",
        if overridden.is_empty() {
            "none".to_owned()
        } else {
            overridden.join(",")
        },
    );
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command.current_dir(root).args(args);
    // 仅认当前源码 checkout，避免外层命令的 Git 重定位设置指向别的仓库。
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(name);
    }
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    String::from_utf8(git(root, args)?)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn automatic_values(root: Option<&Path>) -> BTreeMap<&'static str, String> {
    let release = root
        .and_then(|root| std::fs::read_to_string(root.join(VERSION_PATH)).ok())
        .map_or_else(|| "unknown".to_owned(), |text| release_from_json(&text));
    let (commit, dirty) = root.filter(|root| root.join(".git").exists()).map_or_else(
        || ("unknown".to_owned(), "unknown".to_owned()),
        |root| {
            let commit = validated(
                "GIT_COMMIT",
                git_text(root, &["rev-parse", "--verify", "HEAD"]).as_deref(),
            );
            let dirty = if commit == "unknown" {
                "unknown".to_owned()
            } else {
                git(root, &["status", "--porcelain=v1", "--untracked-files=no"]).map_or_else(
                    || "unknown".to_owned(),
                    |status| (!status.is_empty()).to_string(),
                )
            };
            (commit, dirty)
        },
    );
    BTreeMap::from([
        ("RELEASE_VERSION", release),
        ("GIT_COMMIT", commit),
        ("GIT_DIRTY", dirty),
    ])
}

fn git_watch_paths(root: &Path) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::from([root.join(".git")]);
    // .git 为目录时不能监听整个目录（日志/锁会导致无关重建）；工作树 .git 文件需监听。
    if root.join(".git").is_dir() {
        paths.clear();
    }
    for name in ["HEAD", "index", "packed-refs", "refs", "config"] {
        if let Some(path) = git_text(root, &["rev-parse", "--git-path", name]) {
            let path = PathBuf::from(path);
            let resolved = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if resolved.exists() {
                paths.insert(resolved);
            }
        }
    }
    if let Some(reference) = git_text(root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_text(root, &["rev-parse", "--git-path", &reference]) {
            let path = PathBuf::from(path);
            let resolved = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if resolved.exists() {
                paths.insert(resolved);
            }
        }
    }
    if let Some(files) = git(root, &["ls-files", "-z"]) {
        for file in files
            .split(|byte| *byte == 0)
            .filter(|file| !file.is_empty())
        {
            if let Ok(file) = std::str::from_utf8(file) {
                paths.insert(root.join(file));
            }
        }
    }
    paths
}
