//! 预卸载的白名单删除器（darwin）。
//!
//! 职责边界：本模块**只提供"如何安全地删一个已确认为我们产物的目标"的原语与白名单**，
//! 不做流程编排（编排在 [`super::commands`] 的 `pre_uninstall`）。所有删除都必须是
//! "白名单内的目标 + 身份校验通过"两条同时成立。
//!
//! ## 为什么需要白名单（设计依据，勿简化）
//!
//! `/private/tmp` 是**世界可写**目录，且实测其下同时存在我们的 runtime 目录与开发产物
//! （`exv-build-final.log`、`exv_clippy_full.log`、`exv-tunnel-watch.sh`、`exv-dev-docs/`
//! 等十余个）。一句 `rm -rf /private/tmp/exv*` 会把它们全删掉。因此：
//!
//! * 只接受**代码常量推导的绝对路径**，或**严格正则校验的单层目录名**；
//! * 删除前必须 `lstat`（`AT_SYMLINK_NOFOLLOW`）确认**不是符号链接**，且**属主在允许集合内**；
//! * 父目录身份用 **dirfd** 校验（`open_fixed_runtime_parent` 纪律），**路径字符串永不参与删除**；
//! * **禁止 `rm -rf` / `std::fs::remove_dir_all`**：只有 `unlinkat`（已知 leaf）与
//!   `unlinkat(AT_REMOVEDIR)`（目录）两个原语；**非空即失败并保留现场**；
//! * 不"先查空再删"（消除 TOCTOU）：直接 `rmdir`，`ENOTEMPTY` 即保留并报告。
//!
//! ## 两段分层（W0）
//!
//! * **用户态段**：可含 `HOME` 派生路径（`~/.exv` 等）；`/private/tmp` 下只允许
//!   属主 == 当前 euid 的目录。
//! * **提权段（root）**：只允许**编译期字面量**路径进入 payload。严禁 `env::var` 派生值
//!   （`config_paths::config_dir()` 读 `EXV_CONFIG_DIR`/`USERPROFILE`/`HOME`——若它们进入
//!   root payload，控制环境变量即可让 root 删任意路径），故提权目标在
//!   [`elevated_targets`] 中以字面量给出、且该模块不读取任何环境变量。

use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
};

/// runtime 目录的固定父目录（与 `exv-vpn-darwin-ipc::path` 的 `RUNTIME_DIR_PARENT` 一致）。
const RUNTIME_DIR_PARENT: &str = "/private/tmp";

// runtime 目录名的固定形状 `exv-vpn-<pid:x>-<16 位随机 hex>` 由 [`is_runtime_dir_name`]
// 逐字节实现（不引入正则依赖）。
//
// **pid 位数下限必须是 1 而不是 4**：`create_runtime_dir_name` 用 `{:x}` 格式化 pid，
// pid 小时只有 3 位十六进制。实测残留中 `exv-vpn-5e6-…`（pid 1510）与 `exv-vpn-641-…`
// （pid 1601）用 4 位下界会**静默漏删**——与"尽力删除"直接冲突。

/// runtime 目录内**允许被删除的已知 leaf**（引擎 socket / 一次性 ticket / 控制 socket）。
///
/// 与服务代理 `sweep-runtime-residue` 的白名单同源；两处刻意各自定义（壳层卸载必须在
/// 该组件缺失或不可用时仍能工作，故不依赖其 crate；改动需同步两边）。名单之外的条目即保留现场。
pub(crate) const KNOWN_RUNTIME_LEAVES: [&str; 3] = ["engine.sock", "engine.ticket", "control.sock"];

/// 一次删除尝试的结果。**逐项报告**，不合并成笼统的成功/失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    /// 目标存在且已删除。
    Removed,
    /// 目标本就不存在（幂等）。
    Absent,
    /// 明确跳过并给出**可读原因**（白名单拒绝 / 属主不符 / 活会话 / 非空 / 只读卷……）。
    Skipped(&'static str),
    /// 尝试删除但系统调用失败；携带 `errno` 供诊断。
    Failed(i32),
}

/// 删除尝试失败的原因分类（写入结果供用户与诊断使用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// 路径不在白名单内（父目录不符 / 名字不匹配 / 非法组件）。
    NotWhitelisted,
    /// 末组件是符号链接。
    Symlink,
    /// 属主不在允许集合内。
    Owner,
    /// 目录非空（`rmdir` 语义下保留现场）。
    NotEmpty,
}

impl Refusal {
    #[must_use]
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::NotWhitelisted => "不在白名单内",
            Self::Symlink => "目标是符号链接",
            Self::Owner => "属主不符",
            Self::NotEmpty => "目录非空（保留现场）",
        }
    }
}

/// 白名单校验 + 身份校验通过后的**可删除目标**。
///
/// 类型设计要点（W6）：目标以 `(parent_fd, leaf)` 表达，而不是一个路径字符串。
/// 调用方拿不到"以 `/Applications`/`~`/`/private/tmp` 为叶"的目标，也无法把任意路径
/// 直接交给删除函数——这使"绝不删父目录"成为类型层面的约束而非文字承诺。
pub(crate) struct WhitelistedTarget {
    parent: OwnedFd,
    leaf: CString,
}

impl WhitelistedTarget {
    /// 删除已知 leaf（`unlinkat`，不带 `AT_REMOVEDIR`）。用于 socket / 普通文件。
    ///
    /// # Errors
    ///
    /// 目标不存在返回 [`Removal::Absent`]；系统调用失败返回 [`Removal::Failed`]。
    pub(crate) fn remove_leaf(self) -> Removal {
        // SAFETY: parent 是以 O_DIRECTORY|O_NOFOLLOW 打开的受控父目录 fd，leaf 是白名单
        // 内的单层名字（无内部 NUL，构造时已校验）。
        let result = unsafe { libc::unlinkat(self.parent.as_raw_fd(), self.leaf.as_ptr(), 0) };
        errno_removal(result)
    }

    /// 删除**空**目录（`unlinkat(AT_REMOVEDIR)`）。
    ///
    /// **不做"先查空"预检**（消除 TOCTOU）：直接 `rmdir`，内核在非空时返回 `ENOTEMPTY`，
    /// 此时保留现场并报告——若预检与删除之间有人放入新文件，预检会误判为可删。
    ///
    /// # Errors
    ///
    /// 目标不存在返回 [`Removal::Absent`]；非空返回 [`Removal::Skipped`]；其余失败
    /// 返回 [`Removal::Failed`]。
    pub(crate) fn remove_empty_dir(self) -> Removal {
        // SAFETY: 同 `remove_leaf`；AT_REMOVEDIR 使其仅删除空目录。
        let result = unsafe {
            libc::unlinkat(
                self.parent.as_raw_fd(),
                self.leaf.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        match errno_removal(result) {
            Removal::Failed(errno) if errno == libc::ENOTEMPTY => {
                Removal::Skipped(Refusal::NotEmpty.reason())
            }
            other => other,
        }
    }
}

fn errno_removal(result: i32) -> Removal {
    if result == 0 {
        return Removal::Removed;
    }
    let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if errno == libc::ENOENT {
        return Removal::Absent;
    }
    Removal::Failed(errno)
}

/// `lstat` 得到的身份（与 `ipc::path` 的纪律同源：只看末组件，不跟随符号链接）。
struct Identity {
    uid: u32,
    /// 文件类型位（`st_mode & S_IFMT`）；macOS 上 `S_IFMT` 是 `u16`。
    kind: u16,
    device: u64,
    inode: u64,
}

/// 打开固定父目录并校验其身份（root 所有、非符号链接、`(dev,ino)` 与路径一致）。
///
/// 这是 W3 的落点：**父目录身份用 dirfd 校验**，而不是比较路径字符串——`/private/tmp`
/// 本身被替换成符号链接时，字符串比较会被绕过。
fn open_controlled_parent(parent: &Path, expected: ParentPolicy) -> Result<OwnedFd, Refusal> {
    let path =
        CString::new(parent.as_os_str().as_encoded_bytes()).map_err(|_| Refusal::NotWhitelisted)?;
    // SAFETY: path 是 NUL 结尾常量，flags 请求目录 fd 且不跟随末组件符号链接。
    let raw = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(Refusal::NotWhitelisted);
    }
    // SAFETY: open 返回的非负 fd 归本次调用所有，恰好转移一次给 OwnedFd。
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: fd 有效，stat 是 libc 可写的有效存储。
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(Refusal::NotWhitelisted);
    }
    // SAFETY: fstat 返回 0 保证 stat 已初始化。
    let stat = unsafe { stat.assume_init() };
    let matches = match expected {
        // `/private/tmp`：0755/0177 风格的世界可写粘滞目录，属主 root。
        ParentPolicy::WorldWritableRuntimeParent => {
            stat.st_mode & libc::S_IFMT == libc::S_IFDIR && stat.st_uid == 0
        }
        // 用户态私有父目录（`~/.exv`、`~/Library/...`）：属主必须是当前 euid。
        ParentPolicy::OwnedByCurrentUser => {
            stat.st_mode & libc::S_IFMT == libc::S_IFDIR
                && stat.st_uid == unsafe { libc::geteuid() }
        }
    };
    if !matches {
        return Err(Refusal::NotWhitelisted);
    }
    Ok(fd)
}

/// 父目录的身份策略。
#[derive(Clone, Copy)]
pub(crate) enum ParentPolicy {
    /// `/private/tmp`：世界可写、root 属主；叶子必须再加属主约束（见 [`open_target`]）。
    WorldWritableRuntimeParent,
    /// 用户态私有目录：属主 == 当前 euid。
    OwnedByCurrentUser,
}

/// 在受控父目录下解析并校验一个白名单叶子，得到可删除目标。
///
/// 全部校验（W1 名字形状、W2 属主、W3 父目录身份、末组件非符号链接）在此收口。
fn open_target(
    parent: &Path,
    leaf: &str,
    policy: ParentPolicy,
) -> Result<WhitelistedTarget, Refusal> {
    if leaf.is_empty() || leaf == "." || leaf == ".." || leaf.contains('/') {
        return Err(Refusal::NotWhitelisted);
    }
    let parent_fd = open_controlled_parent(parent, policy)?;
    let name = CString::new(leaf).map_err(|_| Refusal::NotWhitelisted)?;

    let Some(identity) = stat_at(parent_fd.as_raw_fd(), &name)? else {
        // 目标不存在：仍返回一个"目标"以便上层得到 Absent（而不是视为失败）。
        return Ok(WhitelistedTarget {
            parent: parent_fd,
            leaf: name,
        });
    };
    // 末组件符号链接：`fstatat` 用 AT_SYMLINK_NOFOLLOW，因此符号链接会以 S_IFLNK 出现。
    if identity.kind == libc::S_IFLNK {
        return Err(Refusal::Symlink);
    }
    let euid = unsafe { libc::geteuid() };
    let owner_ok = match policy {
        // 世界可写父目录：只允许 root 或自己的产物——否则会删掉其他本地用户建的同名目录。
        ParentPolicy::WorldWritableRuntimeParent => identity.uid == 0 || identity.uid == euid,
        ParentPolicy::OwnedByCurrentUser => identity.uid == euid,
    };
    if !owner_ok {
        return Err(Refusal::Owner);
    }

    // 二次确认：解析后、删除前，用**同一个** parent fd 复核身份未变（收窄 TOCTOU）。
    let Some(current) = stat_at(parent_fd.as_raw_fd(), &name)? else {
        return Ok(WhitelistedTarget {
            parent: parent_fd,
            leaf: name,
        });
    };
    if current.device != identity.device || current.inode != identity.inode {
        return Err(Refusal::NotWhitelisted);
    }

    Ok(WhitelistedTarget {
        parent: parent_fd,
        leaf: name,
    })
}

fn stat_at(parent_fd: RawFd, leaf: &CString) -> Result<Option<Identity>, Refusal> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: parent_fd 是打开的目录 fd，leaf NUL 结尾且无内部 NUL，stat 是有效存储。
    let result = unsafe {
        libc::fstatat(
            parent_fd,
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(Refusal::NotWhitelisted);
    }
    // SAFETY: fstatat 返回 0 保证 stat 已初始化。
    let stat = unsafe { stat.assume_init() };
    Ok(Some(Identity {
        uid: stat.st_uid,
        kind: stat.st_mode & libc::S_IFMT, // u16（macOS）
        device: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
        inode: stat.st_ino,
    }))
}

/// 判定 runtime 目录名是否匹配固定形状。
///
/// 用**逐字节**校验而非正则引擎（避免引入依赖，也让形状在代码里一眼可读）：
/// `exv-vpn-` + 1..=8 位小写 hex（pid）+ `-` + 恰好 16 位小写 hex。
#[must_use]
pub(crate) fn is_runtime_dir_name(leaf: &str) -> bool {
    let Some(rest) = leaf.strip_prefix("exv-vpn-") else {
        return false;
    };
    let Some((pid, suffix)) = rest.split_once('-') else {
        return false;
    };
    let hex = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    (1..=8).contains(&pid.len()) && suffix.len() == 16 && hex(pid) && hex(suffix)
}

/// 解析 `/private/tmp/exv-vpn-*` 目标（W1(b) + W2 + W3 + 末组件校验）。
pub(crate) fn runtime_target(leaf: &str) -> Result<WhitelistedTarget, Refusal> {
    if !is_runtime_dir_name(leaf) {
        return Err(Refusal::NotWhitelisted);
    }
    open_target(
        Path::new(RUNTIME_DIR_PARENT),
        leaf,
        ParentPolicy::WorldWritableRuntimeParent,
    )
}

/// 解析 `/private/tmp` 下**本用户属主的已知单层叶子**（如单实例 socket）。
///
/// 与 [`runtime_target`] 同一父目录策略（`/private/tmp` 是 **root 属主**的世界可写目录，
/// 不能用 [`user_target`]——那要求父目录属主 == euid，会一律拒绝）；调用方负责保证
/// `leaf` 是代码常量推导的固定名字（不接受外部输入）。
pub(crate) fn tmp_leaf_target(leaf: &str) -> Result<WhitelistedTarget, Refusal> {
    open_target(
        Path::new(RUNTIME_DIR_PARENT),
        leaf,
        ParentPolicy::WorldWritableRuntimeParent,
    )
}

/// 解析用户态目录目标（父目录必须是当前用户所有）。
pub(crate) fn user_target(path: &Path) -> Result<WhitelistedTarget, Refusal> {
    let parent = path.parent().ok_or(Refusal::NotWhitelisted)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Refusal::NotWhitelisted)?;
    open_target(parent, leaf, ParentPolicy::OwnedByCurrentUser)
}

/// 判定路径所在卷是否为只读（`statfs` 的 `MNT_RDONLY`）。
///
/// **fail-closed 纪律**：`statfs` 返回非 0（路径不存在 / 不可读 / 其它失败）时返回 `None`
/// 表示**不可判定**，调用方必须按"不可判定"处理（不执行删除）。把不可判定当"可写"会让
/// 只读卷上的删除尝试产生误导性失败。
#[must_use]
pub(crate) fn volume_read_only(path: &Path) -> Option<bool> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: c_path 是 NUL 结尾路径，stat 是 libc 可写的有效存储。
    if unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statfs 返回 0 保证 stat 已初始化。
    let stat = unsafe { stat.assume_init() };
    Some(stat.f_flags & u32::try_from(libc::MNT_RDONLY).unwrap_or(1) != 0)
}

/// 判定 runtime 目录是否被运行中的进程持有（W10）。
///
/// 实现：`/private/tmp/exv-vpn-<pid:x>-*` 的名字里带**创建者 pid**。目录仍被持有通常意味着
/// 该进程仍在运行或刚退出未回收；用 `kill(pid, 0)` 探测其存在（`EPERM` 也算存在）。
/// 名字不合形状时返回 `false`（由 [`is_runtime_dir_name`] 另行拒绝）。
#[must_use]
pub(crate) fn runtime_dir_owner_alive(leaf: &str) -> bool {
    let Some(rest) = leaf.strip_prefix("exv-vpn-") else {
        return false;
    };
    let Some((pid_hex, _)) = rest.split_once('-') else {
        return false;
    };
    let Ok(pid) = i32::from_str_radix(pid_hex, 16) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill 只读进程存在性；pid 已校验为正数。
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    // EPERM：进程存在但无权限发信号（root 属主的 runtime 对普通用户即此情形）。
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
