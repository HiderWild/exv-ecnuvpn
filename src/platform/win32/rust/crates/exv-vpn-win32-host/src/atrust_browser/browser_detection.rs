
use exv_vpn_atrust::browser_selection::{BrowserKind, Detection};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// StartMenuInternet 枚举视图：HKLM/HKCU × WOW64 64/32。
///
/// HKCU 本身不受 WOW64 重定向（两视图结果一致），仍按同两视图枚举、靠键级去重
/// 收敛，与计划"`/reg:64` 与 `/reg:32` 两视图循环"的文本一致。
const START_MENU_VIEWS: &[(&str, &str)] = &[
    (r"HKLM\SOFTWARE\Clients\StartMenuInternet", "/reg:64"),
    (r"HKLM\SOFTWARE\Clients\StartMenuInternet", "/reg:32"),
    (r"HKCU\SOFTWARE\Clients\StartMenuInternet", "/reg:64"),
    (r"HKCU\SOFTWARE\Clients\StartMenuInternet", "/reg:32"),
];

/// HTTPS UserChoice（系统默认浏览器）读取键。
const HTTPS_USER_CHOICE_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice";

/// 一次探测的注册表结论：可用浏览器 exe 表 + 默认浏览器。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BrowserRegistry {
    /// 已装可用（exe 在位）的支持浏览器 → 可执行路径。
    available: BTreeMap<BrowserKind, PathBuf>,
    /// UserChoice ProgId 映射到支持集后的默认浏览器；读不到/不受支持为 [`None`]。
    default: Option<BrowserKind>,
}

impl BrowserRegistry {
    /// 供 `browser_selection` 阶梯消费的一次探测产物（无会话显式偏好）。
    #[must_use]
    pub(super) fn detection(&self) -> Detection {
        Detection::new(self.default, self.available.keys().copied().collect())
    }

    /// 可用浏览器 exe 表（阶梯选中后驱动浏览器、缓存轻量复核共用）。
    #[must_use]
    pub(super) fn into_executables(self) -> BTreeMap<BrowserKind, PathBuf> {
        self.available
    }

}

/// StartMenuInternet 客户端键名 → 支持集浏览器。
///
/// 键名含稳定标识：标准与测试渠道共用 `Google Chrome` / `Microsoft Edge` 前缀
/// （如 `Google Chrome Beta`、`Microsoft Edge SxS`），枚举序（字母序）保证标准
/// 渠道先于渠道变体被采纳。
fn kind_for_client_key_name(name: &str) -> Option<BrowserKind> {
    if name.contains("Google Chrome") {
        Some(BrowserKind::Chrome)
    } else if name.contains("Microsoft Edge") {
        Some(BrowserKind::Edge)
    } else {
        None
    }
}

/// HTTPS UserChoice ProgId → 支持集浏览器；其余 ProgId（Firefox、IE 等）为 [`None`]。
fn kind_for_progid(progid: &str) -> Option<BrowserKind> {
    match progid {
        "MSEdgeHTM" => Some(BrowserKind::Edge),
        "ChromeHTML" | "GoogleChromeHTML" => Some(BrowserKind::Chrome),
        _ => None,
    }
}

/// 解析 `reg query <StartMenuInternet>` 的子键清单 → 客户端键名列表。
///
/// 每行形如 `HKEY_LOCAL_MACHINE\SOFTWARE\Clients\StartMenuInternet\Google Chrome`；
/// 父键行（无尾部键名）与非直接子键（含更深层级）跳过。
#[must_use]
fn parse_client_key_names(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let name = line.split_once(r"StartMenuInternet\")?.1.trim();
            if name.is_empty() || name.contains('\\') {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .collect()
}

/// 解析 `reg query <key> /ve` 输出 → 默认值命令串（`REG_SZ` 之后整段）。
///
/// 值名（`(Default)` 等）随系统语言本地化，`REG_SZ` 才是稳定锚点。
#[must_use]
fn parse_default_command(query_output: &str) -> Option<&str> {
    query_output.lines().find_map(|line| {
        let value = line.split_once("REG_SZ")?.1.trim();
        (!value.is_empty()).then_some(value)
    })
}

/// 解析 `reg query <UserChoice> /v ProgId` 输出 → ProgId 值。
#[must_use]
fn parse_prog_id(query_output: &str) -> Option<&str> {
    query_output.lines().find_map(|line| {
        let value = line.split_once("ProgId")?.1.split_once("REG_SZ")?.1.trim();
        (!value.is_empty()).then_some(value)
    })
}

/// 从 `shell\open\command` 命令串提取浏览器可执行路径。
///
/// 形态：`"C:\Program Files\...\chrome.exe" --single-argument %1`（带引号）或
/// 无引号路径 + 参数；无引号时按 `.exe` 边界截取（不取首空格段，路径可含空格）。
#[must_use]
fn executable_from_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        let path = rest.split('"').next()?;
        return if path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        };
    }
    let end = command.to_ascii_lowercase().find(".exe")? + ".exe".len();
    Some(PathBuf::from(&command[..end]))
}

/// 由注册表快照构建一次探测结论（纯函数接缝：生产由 [`detect_registry`] 供数，
/// 单测由夹具文本供数）。`listings` 为各视图（根键，枚举输出）按探测顺序排列；
/// `commands` 把（根键，客户端键名）映射到该键 `shell\open\command` 查询输出
/// （查询失败为 [`None`]）；`user_choice` 为 UserChoice 查询输出。
fn registry_from_snapshot(
    listings: &[(String, String)],
    commands: &dyn Fn(&str, &str) -> Option<String>,
    user_choice: Option<&str>,
) -> BrowserRegistry {
    let mut available = BTreeMap::new();
    for (root, listing) in listings {
        for name in parse_client_key_names(listing) {
            let Some(kind) = kind_for_client_key_name(&name) else {
                continue;
            };
            // 两视图/多渠道去重：首个在位命中即定格本种类。
            if available.contains_key(&kind) {
                continue;
            }
            let executable = commands(root, &name)
                .as_deref()
                .and_then(parse_default_command)
                .and_then(executable_from_command);
            if let Some(executable) = executable.filter(|path| path.is_file()) {
                available.insert(kind, executable);
            }
        }
    }
    let default = user_choice
        .and_then(parse_prog_id)
        .and_then(kind_for_progid);
    BrowserRegistry { available, default }
}

/// 缓存轻量复核：上次探测 exe 在位即仍可用；无记录时退已知安装目录在位校验。
///
/// 只做二进制在位判法（不做会话级探测），与共享模块缓存规则一致。
pub(super) fn executable_in_place(
    executables: &BTreeMap<BrowserKind, PathBuf>,
    kind: BrowserKind,
) -> bool {
    executables
        .get(&kind)
        .is_some_and(|path| path.is_file())
        || known_executable(kind).is_some()
}

/// 已知安装目录猜测（仅作复核兜底；探测主路径是 StartMenuInternet，替换
/// 既有"已知目录猜路径"探测）。
#[must_use]
fn known_executable(kind: BrowserKind) -> Option<PathBuf> {
    let relative = match kind {
        BrowserKind::Edge => "Microsoft/Edge/Application/msedge.exe",
        BrowserKind::Chrome => "Google/Chrome/Application/chrome.exe",
        BrowserKind::Safari => return None,
    };
    ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"].into_iter().find_map(|variable| {
        std::env::var_os(variable)
            .map(|root| PathBuf::from(root).join(relative))
            .filter(|path| path.is_file())
    })
}

/// 运行 `reg.exe query`；成功返回 stdout 文本，任何失败返回 [`None`]。
fn run_reg_query(key: &str, extra_args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let mut args = vec!["query", key];
    args.extend_from_slice(extra_args);
    let output = Command::new("reg.exe")
        .args(&args)
        .creation_flags(0x08000000)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 真实注册表完整探测：StartMenuInternet 四视图枚举 + UserChoice 默认。
///
/// 任一视图查询失败只影响该视图（不放大为整体失败）；全部视图零命中即零可用，
/// 由共享阶梯给出回退内嵌结论。
pub(super) fn detect_registry() -> BrowserRegistry {
    let listings: Vec<(String, String)> = START_MENU_VIEWS
        .iter()
        .filter_map(|&(root, view)| {
            run_reg_query(root, &[view]).map(|text| (root.to_owned(), text))
        })
        .collect();
    let commands = |root: &str, name: &str| -> Option<String> {
        // 客户端键名来自枚举输出，作 reg.exe 参数数组直传（不经 shell 解释）；
        // 命令键沿用所属根键（HKLM/HKCU 分别登记），视图先 64 后 32（标准安装主视图优先）。
        let key = format!(r"{root}\{name}\shell\open\command");
        run_reg_query(&key, &["/reg:64", "/ve"])
            .or_else(|| run_reg_query(&key, &["/reg:32", "/ve"]))
    };
    registry_from_snapshot(
        &listings,
        &commands,
        run_reg_query(HTTPS_USER_CHOICE_KEY, &["/v", "ProgId"]).as_deref(),
    )
}
