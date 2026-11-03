
/// 外部浏览器种类。只表达"哪一种浏览器"，不承载路径、版本或驱动细节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BrowserKind {
    /// macOS 系统浏览器；可驱动性取决于 `safaridriver` 在位与远程自动化授权。
    Safari,
    Chrome,
    Edge,
}

/// darwin 支持集，数组序即优先序：Chrome > Edge（Safari 自动化通道已按用户裁定摘除）。
pub const MACOS_SUPPORTED: &[BrowserKind] = &[
    BrowserKind::Chrome,
    BrowserKind::Edge,
];

/// win32 支持集，数组序即优先序：Edge > Chrome。
pub const WINDOWS_SUPPORTED: &[BrowserKind] = &[BrowserKind::Edge, BrowserKind::Chrome];

/// 阶梯结论：用哪个外部浏览器，或静默回退内嵌浏览器。
///
/// [`Decision::FallBackEmbedded`] 只表达"本次连接用内嵌浏览器完成"，不携带错误与
/// 用户可见提示——回退是静默的，不改用户认证设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 选中某个外部浏览器；宿主据此驱动对应通道。
    Use(BrowserKind),
    /// 可用集为空：本次连接静默回退内嵌浏览器。
    FallBackEmbedded,
}

/// 纯函数选择阶梯。平台优先序经 `supported_order` 注入以保持平台无关（宿主传
/// [`MACOS_SUPPORTED`] 或 [`WINDOWS_SUPPORTED`]）。
///
/// 全部分支与优先级见模块文档。`available` 允许宿主按任意顺序传入，函数内部按
/// `supported_order` 的优先序取最优。防御性约定：不在 `supported_order` 内的项即使
/// 混入 `available` 也不会被选中（宿主本应先过滤，这里不放大宿主错误）。
#[must_use]
pub fn select_browser(
    supported_order: &[BrowserKind],
    default: Option<BrowserKind>,
    available: &[BrowserKind],
    chosen: Option<BrowserKind>,
    cached: Option<BrowserKind>,
) -> Decision {
    let usable = |kind: BrowserKind| available.contains(&kind) && supported_order.contains(&kind);
    if let Some(kind) = chosen.filter(|k| usable(*k)) {
        return Decision::Use(kind);
    }
    if let Some(kind) = cached.filter(|k| usable(*k)) {
        return Decision::Use(kind);
    }
    if let Some(kind) = default.filter(|k| usable(*k)) {
        return Decision::Use(kind);
    }
    supported_order
        .iter()
        .copied()
        .find(|k| usable(*k))
        .map_or(Decision::FallBackEmbedded, Decision::Use)
}

/// 一次完整探测的产物，由宿主探测闭包产出并喂给 [`select_browser`]。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    /// 系统默认浏览器映射到支持集后的种类；不在支持集（如 Firefox）传 [`None`]。
    pub default: Option<BrowserKind>,
    /// 探测并按"可用"过滤后的支持浏览器列表（顺序任意，阶梯内部按优先序取最优）。
    pub available: Vec<BrowserKind>,
    /// 会话内显式偏好（darwin 遗留 wire 兼容）；无写入方时恒为 [`None`]。
    pub chosen: Option<BrowserKind>,
}

impl Detection {
    /// 无会话显式偏好的探测产物（win32 常规形态，`chosen` 为 [`None`]）。
    #[must_use]
    pub fn new(default: Option<BrowserKind>, available: Vec<BrowserKind>) -> Self {
        Self {
            default,
            available,
            chosen: None,
        }
    }
}

/// 浏览器选择的进程内缓存：值类型，无内部可变静态，缓存槽由各宿主持有实例
/// （win32 宿主进程内静态持有、darwin 壳进程持有，各自 new 一个实例）。
///
/// 缓存规则（与计划裁定一致）：
/// - 无缓存才运行完整探测；
/// - 探测结论只要不是回退内嵌（[`Decision::Use`]）就写入缓存；
///   [`Decision::FallBackEmbedded`] 永不写入——下次仍重新探测；
/// - 有缓存只做轻量复核（`is_available` 注入布尔，调用方保证轻量：二进制在位/
///   默认浏览器读取一类，禁止会话级探测），复核可用即短路返回，接受
///   "更优新装感知不到"的陈旧性；复核不可用则清缓存并重跑完整探测。
///
/// 生命周期为进程内：应用重启即清空。宿主若需要响应会话内显式偏好（`chosen`）变化，
/// 应在变化点调用 [`SelectionCache::invalidate`]——缓存命中路径不重读 `chosen`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionCache {
    slot: Option<BrowserKind>,
}

impl SelectionCache {
    /// 空缓存实例。
    #[must_use]
    pub const fn new() -> Self {
        Self { slot: None }
    }

    /// 当前缓存的上次结论；未缓存返回 [`None`]。
    #[must_use]
    pub const fn cached(&self) -> Option<BrowserKind> {
        self.slot
    }

    /// 显式失效：清空缓存，下次 [`SelectionCache::resolve`] 将重跑完整探测。
    pub fn invalidate(&mut self) {
        self.slot = None;
    }

    /// 带缓存的阶梯求值：缓存命中且轻量复核可用即短路；否则跑 `detect` 并按缓存
    /// 规则落槽。
    ///
    /// `is_available` 仅在缓存命中路径被调用（入参为缓存种类）；探测路径的可用性
    /// 已由 `detect` 产出的 `available` 列表表达，不再复核。缓存种类不针对
    /// `supported_order` 重验——同一进程内宿主优先序恒定。
    pub fn resolve<D, F>(
        &mut self,
        supported_order: &[BrowserKind],
        detect: D,
        is_available: F,
    ) -> Decision
    where
        D: FnOnce() -> Detection,
        F: Fn(BrowserKind) -> bool,
    {
        if let Some(kind) = self.slot {
            if is_available(kind) {
                return Decision::Use(kind);
            }
            self.slot = None;
        }
        let detection = detect();
        let decision = select_browser(
            supported_order,
            detection.default,
            &detection.available,
            detection.chosen,
            None,
        );
        if let Decision::Use(kind) = decision {
            self.slot = Some(kind);
        }
        decision
    }
}
