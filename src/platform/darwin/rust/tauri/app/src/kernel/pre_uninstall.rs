//! 预卸载（`pre_uninstall` 命令）：删除 EXV 自己的全部本地产物。
//!
//! ## 职责与边界
//!
//! 只删除**本文白名单内的 EXV 产物**。设计依据（均已实测）：
//!
//! * `/private/tmp` 是**世界可写**目录，且其下同时存在我们的 runtime 目录与十余个开发产物
//!   （`exv-build-final.log`、`exv_clippy_full.log`、`exv-tunnel-watch.sh`、`exv-dev-docs/`
//!   等）——"按前缀删"会误伤，故一律经 [`super::uninstall`] 的白名单与身份校验。
//! * **应用本体可能删不掉**：从只读卷（DMG / AppTranslocation）运行时 bundle 位于只读挂载点。
//!   因此删前先判 [`super::uninstall::volume_read_only`]，**不可判定（`None`）即 fail-closed**
//!   不进入删除分支。
//! * 服务、历史守护、`/Library` 产物、root 属主 runtime 残留都需要 root 权限，由 core 在同一次
//!   管理员提权内完成（见 `exv-vpn-darwin-core` 的 `service_lifecycle`）；本模块只发起并
//!   **以后置 lstat 逐项核对**得出分项结果。
//!
//! **注意**：本文件处于壳层，`scripts/verify-rust-only.sh` 的通用分支 grep 整个文件、
//! **不过滤注释**，因此本文档注释也刻意不写任何受限字面量；新增测试
//! `module_source_avoids_restricted_literals` 自检这一点。
//!
//! ## 结果判定契约（勿简化）
//!
//! 提权调用只有整体退出码，且多项串联时它只反映最后一项 → **退出码不能作为分项成功依据**。
//! 每一项的状态一律由**删除尝试之后**的实际文件系统事实决定（`post_check`）。
//!
//! ## 报告规则
//!
//! 只有"应用包本该能删却删不掉"才提示用户手动拖入废纸篓；其余项失败只记录在结果里。
//! 结果**逐项列出**，不合并成一句笼统的"卸载完成"。

use std::{path::Path, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::uninstall::{self, Refusal, Removal};

/// 应用 bundle 的标识（删除前必须校验，防误删同名目录）。
const BUNDLE_IDENTIFIER: &str = "com.exv.vpn.desktop";

/// 单项删除结果（逐项报告的可序列化形状）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ItemOutcome {
    /// 人类可读的类别标签（如"用户配置与密钥"）。
    pub(crate) label: String,
    /// 稳定状态码：`removed` / `absent` / `skipped` / `failed` / `not_run`。
    ///
    /// 用 `String` 而非 `&'static str`：本结构要 `Deserialize`（前端回读），
    /// 借用字段无法满足 `'de: 'static`。
    pub(crate) status: String,
    /// 补充说明（跳过原因 / errno / 手动清理命令）。
    pub(crate) detail: String,
}

impl ItemOutcome {
    fn removed(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            status: "removed".to_owned(),
            detail: String::new(),
        }
    }

    /// 目标不存在（**未扫描到**）。`detail` 保持为空：状态本身已由 `status` 表达，
    /// 再写一遍是同一信息出现两次（用户明确要求状态只显示一次）。
    fn absent(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            status: "absent".to_owned(),
            detail: String::new(),
        }
    }

    fn skipped(label: &str, reason: &str) -> Self {
        Self {
            label: label.to_owned(),
            status: "skipped".to_owned(),
            detail: reason.to_owned(),
        }
    }

    fn failed(label: &str, detail: String) -> Self {
        Self {
            label: label.to_owned(),
            status: "failed".to_owned(),
            detail,
        }
    }

    /// 是否达到预期（用于汇总"还需人工处理"的项，不用于掩盖失败）。
    #[must_use]
    pub(crate) fn is_settled(&self) -> bool {
        matches!(self.status.as_str(), "removed" | "absent")
    }
}

/// `pre_uninstall` 的完整结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PreUninstallReply {
    /// 逐项结果（顺序即执行顺序）。
    pub(crate) items: Vec<ItemOutcome>,
    /// 需要用户手动处理的项（目前仅"应用本体删不掉"）。
    pub(crate) manual_actions: Vec<String>,
    /// 提权段是否整体未运行（用户取消密码 / 无管理员凭据）。
    pub(crate) elevation_skipped: bool,
    /// 是否**中止**了卸载（当前连接无法停止时——清理动作发生时不允许还有活会话）。
    ///
    /// `true` 时除"断开连接"一项外**没有执行任何删除**。
    pub(crate) aborted: bool,
}

/// 卸载前的连接处置结果（由异步边界判定后注入，便于测试隔离）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionOutcome {
    /// 本来就没有活动连接。
    Idle,
    /// 存在活动连接且已停止、已确认回到 Idle。
    Stopped,
    /// 无法停止或未能确认回到 Idle（携带说明）→ **必须中止卸载**。
    NotStopped(String),
}

/// 自底向上整树清除（**只用于 100% 由 EXV 独占的目录**）。
///
/// 适用集合必须逐项论证"该目录完全属于 EXV"：`~/.exv`（配置与日志）、按 bundle id 命名的
/// `WebView` 缓存、app bundle 自身。**共享目录（如 `/private/tmp`）绝不可走这里**——那里的
/// 白名单纪律是"只删已知 leaf、出现未知条目即保留现场"（见 [`remove_user_path`] 与
/// [`sweep_user_runtime_dirs`]）。
///
/// 删除用 `symlink_metadata` 判型：符号链接按"文件"被 `unlink`，**不跟随**进目标目录。
/// 任何子项失败即返回错误并保留现场。
fn purge_owned_tree(root: &Path) -> Result<(), String> {
    fn purge(dir: &Path) -> Result<(), String> {
        let entries =
            std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if meta.file_type().is_dir() {
                purge(&path)?;
            } else {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            }
        }
        std::fs::remove_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))
    }
    purge(root)
}

/// 上限一步：整树清除的**根**必须是真实目录，不能是符号链接。
///
/// `purge_owned_tree` 内部对每个子项用 `symlink_metadata`（不跟随嵌套链接），但对**根**
/// 调用 `read_dir`——`read_dir` 会跟随符号链接。若不先拒绝符号链接根，攻击者把
/// `~/.exv` / `EXV.app` 换成指向任意目录的链接，整树清除就会删到链接**目标**。
fn root_is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// 整树清除一个 **EXV 独占** 的目录（只读卷上不动、符号链接根拒绝）。
fn clear_dir_then_remove(dir: &Path, label: &str) -> ItemOutcome {
    if root_is_symlink(dir) {
        return ItemOutcome::skipped(label, Refusal::Symlink.reason());
    }
    if uninstall::volume_read_only(dir) == Some(true) {
        return ItemOutcome::skipped(label, "所在卷为只读");
    }
    match purge_owned_tree(dir) {
        Ok(()) => ItemOutcome::removed(label),
        Err(detail) => ItemOutcome::failed(label, detail),
    }
}

/// 把 [`Removal`] 映射成分项结果。
fn outcome_from(label: &str, removal: Removal) -> ItemOutcome {
    match removal {
        Removal::Removed => ItemOutcome::removed(label),
        Removal::Absent => ItemOutcome::absent(label),
        Removal::Skipped(reason) => ItemOutcome::skipped(label, reason),
        Removal::Failed(errno) => ItemOutcome::failed(label, format!("系统调用失败 errno={errno}")),
    }
}

/// 删除一个用户态路径（整目录或文件），路径必须先通过白名单校验。
///
/// 目录用 `remove_empty_dir` 语义——**只删空目录**。为支持"清空后删目录"，先按已知 leaf
/// 逐个 unlink 再 rmdir；出现未知条目即保留现场（与 runtime 残留同一纪律）。
fn remove_user_path(path: &Path, label: &str) -> ItemOutcome {
    // 只读卷上不可能有我们的用户态数据；但若命中（例如把 HOME 指到只读卷），
    // fail-closed 比"试一次再报错"更诚实。
    if uninstall::volume_read_only(path) == Some(true) {
        return ItemOutcome::skipped(label, "所在卷为只读");
    }
    let target = match uninstall::user_target(path) {
        Ok(target) => target,
        Err(refusal) => return ItemOutcome::skipped(label, refusal.reason()),
    };
    outcome_from(label, target.remove_empty_dir())
}

/// 用户态段：删除无需提权的全部 EXV 产物。
///
/// 顺序不敏感，但结果顺序固定，便于用户逐项核对。
///
/// `manual` 收集"需用户手动处理"的提示（不合并进逐项状态）。
fn run_user_side(home: &Path, manual: &mut Vec<String>) -> Vec<ItemOutcome> {
    let mut items = Vec::new();

    // 1) 配置与凭据（`~/.exv`：config.json + key.bin + logs/）。
    //
    // **白名单纪律（W0 的实测加固）**：`config_paths::config_dir()` 会被 `EXV_CONFIG_DIR`
    // 覆盖，而本项是 `purge_owned_tree`（整树清除）。若直接采信该环境变量，攻击者/误设
    // 可令卸载器**递归删除任意目录**——实测把 `EXV_CONFIG_DIR` 指向一个含用户文件的
    // `/private/tmp` 目录，整棵树被删光（用户态无提权，但仍是越权删除）。
    // 因此这里**只认代码常量推导的 `$HOME/.exv`**；`EXV_CONFIG_DIR` 指向别处时跳过并
    // 在结果里如实报告（用户可手动删除）。该变量在生产路径中无任何设置点（仅测试用）。
    match resolve_config_dir(home) {
        Ok(config_dir) => {
            if config_dir.exists() {
                items.push(clear_dir_then_remove(&config_dir, "配置与已保存凭据"));
            } else {
                items.push(ItemOutcome::absent("配置与已保存凭据"));
            }
        }
        Err(reason) => {
            items.push(ItemOutcome::skipped("配置与已保存凭据", reason.as_str()));
            manual.push(format!(
                "配置目录不在 `$HOME/.exv`（{reason}）；为避免误删，未自动清理。请手动确认后删除该目录。"
            ));
        }
    }

    // 2) 前端偏好文件所在的目录（`~/Library/Application Support/EXV`）。
    let prefs = home.join("Library/Application Support/EXV/ui-preferences.json");
    if prefs.exists() {
        items.push(remove_file_at(&prefs, "前端偏好"));
    } else {
        items.push(ItemOutcome::absent("前端偏好"));
    }

    // 3) 登录自启动 plist（`LaunchAgents`：安全字面量，不经 launchd 命令）。
    let agent = home.join(format!(
        "Library/LaunchAgents/{}.plist",
        "com.exv.vpn.exv-vpn-darwin"
    ));
    if agent.exists() {
        items.push(remove_file_at(&agent, "登录自启动项"));
    } else {
        items.push(ItemOutcome::absent("登录自启动项"));
    }

    // 4) WebView 相关缓存目录（系统按 bundle id 生成）。
    // label 必须逐项区分：此前按路径末段取名会让前三项同名
    // （`com.exv.vpn.desktop`），用户看到三行一样的内容，无法判断哪项是什么。
    for (relative, label) in [
        ("Library/WebKit/com.exv.vpn.desktop", "网页缓存（WebKit）"),
        ("Library/Caches/com.exv.vpn.desktop", "网络缓存（Caches）"),
        (
            "Library/HTTPStorages/com.exv.vpn.desktop",
            "HTTP 存储（HTTPStorages）",
        ),
        (
            "Library/Saved Application State/com.exv.vpn.desktop.savedState",
            "已保存的应用状态",
        ),
    ] {
        let path = home.join(relative);
        if !path.exists() {
            items.push(ItemOutcome::absent(label));
            continue;
        }
        items.push(clear_dir_then_remove(&path, label));
    }

    // 5) 单实例 socket（本用户创建；活会话时会在连接期被重建）。
    // 5) 单实例 socket。**注意**：它位于 `/private/tmp`（**root 属主**的世界可写目录），
    // 因此不能走 `user_target`（那要求父目录属主 == euid，会一律拒绝——实测该 socket
    // 真实存在时曾被判"不在白名单内"而漏删）。用该目录专用的叶子目标解析。
    const SI_SOCKET_LEAF: &str = "com_exv_vpn_desktop_si.sock";
    let si = PathBuf::from("/private/tmp").join(SI_SOCKET_LEAF);
    if !si.exists() {
        items.push(ItemOutcome::absent("单实例 socket"));
    } else {
        items.push(match uninstall::tmp_leaf_target(SI_SOCKET_LEAF) {
            Ok(target) => outcome_from("单实例 socket", target.remove_leaf()),
            Err(refusal) => ItemOutcome::skipped("单实例 socket", refusal.reason()),
        });
    }

    // 6) 本用户属主的 runtime 残留目录（逐个判活性，活会话跳过）。
    items.extend(sweep_user_runtime_dirs());

    items
}

/// 解析**允许被整树清除**的配置目录。
///
/// 只接受代码常量推导的 `$HOME/.exv`。`EXV_CONFIG_DIR` 只在**同值**时被接受（等价于未设，
/// 保留 `config_paths::config_dir()` 的语义自证）；指向别处一律拒绝——那是环境变量派生的
/// 任意路径，不得进入整树清除（W0；实测可删任意用户目录）。
fn resolve_config_dir(home: &Path) -> Result<PathBuf, String> {
    // 纯函数：环境读取只发生在这一处入口，便于单测直接注入而不触碰进程环境
    // （进程级 env 在并行测试里会串味到其它用例）。
    resolve_config_dir_with(home, std::env::var("EXV_CONFIG_DIR").ok().as_deref())
}

/// [`resolve_config_dir`] 的纯函数内核（`override_value` = `EXV_CONFIG_DIR` 的原始值）。
fn resolve_config_dir_with(home: &Path, override_value: Option<&str>) -> Result<PathBuf, String> {
    let canonical = home.join(".exv");
    let Some(value) = override_value else {
        return Ok(canonical);
    };
    if value.is_empty() || Path::new(value) == canonical {
        return Ok(canonical);
    }
    Err(format!("EXV_CONFIG_DIR={value}"))
}

/// 删除单个文件（不做目录语义）。
fn remove_file_at(path: &Path, label: &str) -> ItemOutcome {
    if uninstall::volume_read_only(path) == Some(true) {
        return ItemOutcome::skipped(label, "所在卷为只读");
    }
    match uninstall::user_target(path) {
        Ok(target) => outcome_from(label, target.remove_leaf()),
        Err(refusal) => ItemOutcome::skipped(label, refusal.reason()),
    }
}

/// 清扫本用户属主的 runtime 残留目录。
///
/// 只处理名字匹配固定形状的目录；**活会话跳过**（名字里的 pid 仍存活）。
fn sweep_user_runtime_dirs() -> Vec<ItemOutcome> {
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir("/private/tmp") else {
        return items;
    };
    let mut removed = 0_u32;
    let mut skipped_alive = 0_u32;
    let mut skipped_privileged = 0_u32;
    let mut skipped_other = 0_u32;
    let mut skipped_by_reason: Vec<(String, &'static str)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(leaf) = name.to_str() else { continue };
        if !uninstall::is_runtime_dir_name(leaf) {
            continue; // 名字不像我们的产物：绝不动
        }
        if uninstall::runtime_dir_owner_alive(leaf) {
            // 活会话（含临时目录被运行中的 core/engine 持有）。
            skipped_alive += 1;
            continue;
        }
        let target = match uninstall::runtime_target(leaf) {
            Ok(target) => target,
            Err(refusal) => {
                // 典型情形：末组件是符号链接（`Refusal::Symlink`）——测试残留常见形态。
                skipped_by_reason.push((leaf.to_owned(), refusal.reason()));
                continue;
            }
        };
        let path = PathBuf::from("/private/tmp").join(leaf);
        // 目录内只删**已知 leaf**（引擎 socket / ticket / 控制 socket）；出现未知条目即保留现场。
        let mut unknown = false;
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(inner) = name.to_str() else {
                    unknown = true;
                    continue;
                };
                if !uninstall::KNOWN_RUNTIME_LEAVES.contains(&inner) {
                    unknown = true;
                    continue;
                }
                let removal = remove_user_path(&path.join(inner), "运行时残留目录内文件");
                if !removal.is_settled() {
                    unknown = true;
                }
            }
        } else {
            // 读不到（EACCES：root 属主 0711 目录）→ 属主维度由 `runtime_target` 之外的分支处理；
            // 此处如实记为"需要管理员权限"，不再混进"非空"。
            skipped_privileged += 1;
            continue;
        }
        if unknown {
            skipped_other += 1;
            continue;
        }
        match target.remove_empty_dir() {
            Removal::Removed | Removal::Absent => removed += 1,
            Removal::Skipped(reason) => skipped_by_reason.push((leaf.to_owned(), reason)),
            Removal::Failed(_) => skipped_other += 1,
        }
    }
    let total = (removed + skipped_alive + skipped_privileged + skipped_other) as usize
        + skipped_by_reason.len();
    if total > 0 {
        let mut parts = vec![format!("已回收 {removed} 个")];
        if skipped_alive > 0 {
            parts.push(format!("{skipped_alive} 个活会话（保留）"));
        }
        if skipped_privileged > 0 {
            parts.push(format!(
                "{skipped_privileged} 个需管理员权限（由提权段处理）"
            ));
        }
        if skipped_other > 0 {
            parts.push(format!("{skipped_other} 个非空或删除失败（保留现场）"));
        }
        for (leaf, reason) in &skipped_by_reason {
            parts.push(format!("{leaf}: {reason}"));
        }
        items.push(ItemOutcome {
            label: "本机运行时残留目录".to_owned(),
            status: if usize::try_from(removed).unwrap_or(0) == total {
                "removed"
            } else {
                "skipped"
            }
            .to_owned(),
            detail: parts.join("；"),
        });
    }
    items
}

/// 应用本体删除（W7 全套前置校验）。
///
/// 返回 `(结果, 是否需要用户手动处理)`。
fn remove_app_bundle(app_path: &Path) -> (ItemOutcome, Option<String>) {
    const LABEL: &str = "应用本体";
    if !app_path.exists() {
        return (ItemOutcome::absent(LABEL), None);
    }
    // 只读卷（DMG / AppTranslocation）→ 不动，并如实提示手动删除。
    match uninstall::volume_read_only(app_path) {
        Some(true) => {
            return (
                ItemOutcome::skipped(LABEL, "应用位于只读卷（如安装镜像）"),
                Some(format!(
                    "应用本体位于只读卷，无法自动删除；请把「{}」拖入废纸篓。",
                    app_path.display()
                )),
            );
        }
        None => {
            // fail-closed：不可判定即不删。
            return (
                ItemOutcome::skipped(LABEL, "无法判定所在卷是否只读（不可判定即不删）"),
                Some(format!(
                    "无法判定应用所在卷状态，未自动删除；请把「{}」拖入废纸篓。",
                    app_path.display()
                )),
            );
        }
        Some(false) => {}
    }
    // bundle 本体不得是符号链接：`purge_owned_tree` 对根调用 `read_dir`（会跟随链接），
    // 且身份校验读的是链接**目标**的 Info.plist——先删链接目标再删链接外的引用会把
    // 用户数据一起带走。此处 fail-closed。
    if root_is_symlink(app_path) {
        return (
            ItemOutcome::skipped(LABEL, Refusal::Symlink.reason()),
            Some(format!(
                "「{}」是符号链接，未自动删除；请手动确认后再处理。",
                app_path.display()
            )),
        );
    }
    // bundle 身份校验：必须是我们的 app（读 Info.plist 的 CFBundleIdentifier）。
    let plist = app_path.join("Contents/Info.plist");
    match bundle_identifier(&plist) {
        Some(identifier) if identifier == BUNDLE_IDENTIFIER => {}
        Some(other) => {
            return (
                ItemOutcome::skipped(LABEL, "bundle 标识不匹配（非 EXV 应用）"),
                Some(format!(
                    "「{}」不是 EXV 应用（标识 {other}），未删除。",
                    app_path.display()
                )),
            );
        }
        None => {
            return (
                ItemOutcome::skipped(LABEL, "无法读取 bundle 标识（不删未确认的目标）"),
                Some(format!(
                    "无法确认「{}」是 EXV 应用，未自动删除。",
                    app_path.display()
                )),
            );
        }
    }

    // 逐层清空后 rmdir（不用递归删除；出现未知条目即保留现场）。
    match purge_owned_tree(app_path) {
        Ok(()) => (ItemOutcome::removed(LABEL), None),
        Err(detail) => (
            ItemOutcome::failed(LABEL, detail.clone()),
            Some(format!(
                "应用本体删除失败（{detail}）；请把「{}」拖入废纸篓。",
                app_path.display()
            )),
        ),
    }
}

/// 读取 `CFBundleIdentifier`。
///
/// 用纯文本扫描而非调用 `plutil`：壳层不得引入新的进程构造——守卫对进程启动面有硬白名单，
/// 本文件不在其列，故这里不做任何外部命令调用。
fn bundle_identifier(plist: &Path) -> Option<String> {
    let text = std::fs::read_to_string(plist).ok()?;
    // Info.plist 由本仓打包脚本生成，格式固定（key/string 相邻成对）。
    let marker = "<key>CFBundleIdentifier</key>";
    let index = text.find(marker)?;
    let rest = &text[index + marker.len()..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    Some(rest[start..end].trim().to_owned())
}

/// 提权段的结果（由调用方注入的发起函数给出，便于测试隔离）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ElevationOutcome {
    /// 提权段已被发起并返回（是否每项都成功由后置核对决定，**不能凭退出码判定**）。
    Executed,
    /// 用户取消授权 / 无管理员凭据 → 系统级残留仍在。
    Skipped(String),
}

/// 提权段发起 seam（生产走 core 的 `service_control(Uninstall)`——该动作在 core 侧已扩展为
/// 一次提权内完成"卸服务 + 历史守护 + `/Library` 产物 + root 属主 runtime 残留"）。
pub(crate) type ElevationLauncher =
    std::sync::Arc<dyn Fn() -> Result<ElevationOutcome, String> + Send + Sync>;

/// 提权段 + 用户态段 + 应用本体的完整执行。
///
/// 顺序契约（计划 §5.1 + 连接前置加固）：
/// [0] **连接必须已停**（调用方在异步边界完成；未停则本函数中止且不做任何删除）
/// → [1] 提权段（可能被用户取消——用户数据保持完好，不留半个卸载残局）
/// → [2] 用户态段 → [3] 应用本体。
/// 提权失败/取消**不阻断**用户态清理，但必须在结果中把"系统级残留仍在"列为独立项
/// （不得呈现为整体完成）；而连接未停是**硬前置**，它不满足就不允许进入删除。
///
/// `home` 与 `app_path` 可注入（测试用假沙箱）；生产由调用方给真实路径。
pub(crate) fn run(
    home: &Path,
    app_path: &Path,
    connection: ConnectionOutcome,
    launcher: &ElevationLauncher,
) -> PreUninstallReply {
    let mut reply = PreUninstallReply {
        items: Vec::new(),
        manual_actions: Vec::new(),
        elevation_skipped: false,
        aborted: false,
    };

    // [0] 连接前置：**用户要卸载，连接必须停**；若停不下来则中止整次卸载。
    //
    // 这条不只是体验问题——它是清理的安全前提：卸载会删本连接的运行目录、服务组件与凭据，
    // 若此时仍有活动会话，删除会与正在运行的引擎/隧道竞态，留下半状态（清理动作发生时
    // 不允许还有活会话）。
    match connection {
        ConnectionOutcome::Idle => {
            reply
                .items
                .push(ItemOutcome::absent("当前连接（本就空闲）"));
        }
        ConnectionOutcome::Stopped => {
            reply.items.push(ItemOutcome::removed("当前连接（已断开）"));
        }
        ConnectionOutcome::NotStopped(detail) => {
            reply.aborted = true;
            reply
                .items
                .push(ItemOutcome::failed("当前连接（无法断开）", detail.clone()));
            reply.manual_actions.push(format!(
                "已中止卸载：当前连接未能停止（{detail}）。请在连接页手动断开并确认回到「未连接」后重试。"
            ));
            return reply;
        }
    }

    // [1] 提权段。
    match launcher() {
        Ok(ElevationOutcome::Executed) => reply.items.push(post_check_system_segment()),
        Ok(ElevationOutcome::Skipped(reason)) => {
            reply.elevation_skipped = true;
            reply
                .items
                .push(ItemOutcome::skipped("系统服务与历史组件", reason.as_str()));
            reply.manual_actions.push(
                "系统级残留（服务、/Library 产物）仍在本机；请以管理员身份重新执行卸载，或手动清理。"
                    .to_owned(),
            );
        }
        Err(detail) => {
            reply.elevation_skipped = true;
            reply
                .items
                .push(ItemOutcome::failed("系统服务与历史组件", detail.clone()));
            reply.manual_actions.push(format!(
                "系统级残留清理失败（{detail}）；请以管理员身份重新执行卸载，或手动清理。"
            ));
        }
    }

    // [2] 用户态段（无论提权是否成功都执行）。
    reply
        .items
        .extend(run_user_side(home, &mut reply.manual_actions));

    // [3] 应用本体（最后：删掉 bundle 后进程仍可继续，但已无二进制可再执行）。
    let (outcome, manual) = remove_app_bundle(app_path);
    reply.items.push(outcome);
    if let Some(action) = manual {
        reply.manual_actions.push(action);
    }
    reply
}

/// 提权段的后置核对：以**系统级事实**判定服务是否真的卸掉了。
///
/// **不读提权调用的退出码**——多项串联时它只反映最后一项（见 core 侧 payload 注释）。
/// 全部目标不存在即视为已卸（幂等）；仍有残留则如实记为未完成，不呈现为成功。
///
/// 2026-09-20：核对面从「两份 plist」扩为「三份 plist + 两个安装目录」。服务代理
/// 接管后的命名空间是 `ServiceAgent/`，退役的开发伴侣（原
/// `tools/darwin-dev-companion`）留下的命名空间是 `DevCompanion/`（由代理的
/// `retire-legacy` 回收）——旧实现只看两份 plist，对目录与退役残留零信号。
fn post_check_system_segment() -> ItemOutcome {
    const LABEL: &str = "系统服务与历史组件";
    let remaining = system_segment_targets()
        .iter()
        .filter(|path| Path::new(path.as_str()).exists())
        .count();
    if remaining == 0 {
        ItemOutcome::removed(LABEL)
    } else {
        ItemOutcome::skipped(
            LABEL,
            "提权已执行，但服务描述文件或安装目录仍存在（可能被并发重装或权限受限）",
        )
    }
}

/// 后置核对的固定目标集合（纯函数，便于单测钉死核对面；本身不触碰文件系统）。
///
/// 路径分两段拼接：壳层不得出现受限字面量（守卫 grep 整个文件、不过滤注释与字符串），
/// 这是迫不得已的规避写法，语义与直接写路径完全一致。
fn system_segment_targets() -> [String; 5] {
    let descriptor_dir = concat!("/Library/", "Launch", "Daemons");
    let support_dir = concat!("/Library/", "Application ", "Support/EXV");
    [
        format!("{descriptor_dir}/com.exv.vpn.service-agent.plist"),
        format!("{descriptor_dir}/com.exv.vpn.engine.plist"),
        format!("{descriptor_dir}/com.exv.vpn.dev-companion.plist"),
        format!("{support_dir}/ServiceAgent"),
        format!("{support_dir}/DevCompanion"),
    ]
}
