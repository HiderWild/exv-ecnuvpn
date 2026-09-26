

use std::net::Ipv4Addr;

use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceLuidToIndex, CreateIpForwardEntry2, DeleteIpForwardEntry2, FreeMibTable,
    GetBestRoute2, GetIpForwardEntry2, GetIpForwardTable2, InitializeIpForwardEntry,
    MIB_IPFORWARD_ROW2, MIB_IPFORWARD_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::Networking::WinSock::{AF_INET, MIB_IPPROTO_NETMGMT, NlroManual, SOCKADDR_INET};

use crate::native_error::NativeError;

/// IPv4 前缀长度上限（facts §3：前缀 33 -> `CreateIpForwardEntry2` 返回 87）。
const MAX_PREFIX_LEN: u8 = 32;
/// `ERROR_NOT_FOUND`（facts §3：已 absent 的路由是合法状态，Get 返回 1168）。
const ERROR_NOT_FOUND: u32 = 1168;
/// `ERROR_INVALID_PARAMETER`（facts §3：非法前缀 / 非法删除输入 -> 87）。
const ERROR_INVALID_PARAMETER: u32 = 87;
/// `ERROR_OBJECT_ALREADY_EXISTS`：同一精确路由已存在；新 owner 必须借用而非接管。
const ERROR_OBJECT_ALREADY_EXISTS: u32 = 5010;

/// 一条精确的 IPv4 路由行，与 WSP4 回读行一一对应。
///
/// 行相等是**全字段**相等（facts §3 / W20-T 契约：只按 CIDR 判等会漏删错行——
/// 'delete by CIDR' mutant 在此死）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    /// 网络地址（`new`/`dest_only` 已屏蔽主机位）。
    pub network: Ipv4Addr,
    /// 前缀长度（0..=32；>32 的非法行由安装路径以 87 拒绝）。
    pub prefix_len: u8,
    /// 下一跳地址。
    pub next_hop: Ipv4Addr,
    /// 接口 LUID（`NET_LUID_LH.Value`）。
    pub interface_luid: u64,
    /// 路由度量。
    pub metric: u32,
    /// 路由协议（安装行固定为 3 = `MIB_IPPROTO_NETMGMT`，facts §3 回读冻结值）。
    pub protocol: u32,
}

impl RouteRow {
    /// 从字段构造精确行。
    ///
    /// 规范化：`prefix_len <= 32` 时屏蔽主机位（前缀含主机位会让
    /// `CreateIpForwardEntry2` 返回 87——WSP4 字节序陷阱的宿主位化身，mutant 在此死）；
    /// `protocol` 固定为 3（`MIB_IPPROTO_NETMGMT`，facts §3 回读冻结值）。
    #[must_use]
    pub fn new(
        network: Ipv4Addr,
        prefix_len: u8,
        next_hop: Ipv4Addr,
        interface_luid: u64,
        metric: u32,
    ) -> Self {
        Self {
            network: masked_network(network, prefix_len),
            prefix_len,
            next_hop,
            interface_luid,
            metric,
            protocol: u32::try_from(MIB_IPPROTO_NETMGMT.0).unwrap_or(0),
        }
    }

    /// 构造 dest-only 通配 key（facts §3 冻结：`next_hop` 为 `0.0.0.0`，`interface_luid`、
    /// `metric`、`protocol` 均为 0）。
    ///
    /// `GetIpForwardEntry2` 对这类 key 按通配匹配并填满行；它**不是**精确行（与
    /// 精确行只共享 CIDR 身份），也**不是**合法删除输入——按 CIDR 删除是 mutant。
    #[must_use]
    pub fn dest_only(network: Ipv4Addr, prefix_len: u8) -> Self {
        Self {
            network: masked_network(network, prefix_len),
            prefix_len,
            next_hop: Ipv4Addr::UNSPECIFIED,
            interface_luid: 0,
            metric: 0,
            protocol: 0,
        }
    }

    /// 本行的 CIDR 身份 `(network, prefix_len)`——供查找的谓词，不是行身份。
    #[must_use]
    pub const fn dest_key(&self) -> RouteKey {
        RouteKey {
            network: self.network,
            prefix_len: self.prefix_len,
        }
    }
}

/// 路由行的 CIDR 身份：`(network, prefix_len)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RouteKey {
    /// 网络地址。
    pub network: Ipv4Addr,
    /// 前缀长度。
    pub prefix_len: u8,
}

/// `remove` 的结果；已 absent 是合法、幂等的状态（facts §3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// 精确行找到并被删除。
    Removed,
    /// 不存在该路由，无需处理（facts §3：already-absent 是合法状态，不是错误）。
    AlreadyAbsent,
}

/// 安装请求对路由所有权的实际结果。
///
/// `Created` 携带 Windows 回读的生效行；`CreatedPending` 保留尚待再次清理的提交行；
/// 两者都必须由本次连接登记为 owned。`Borrowed` 表示同一精确行在
/// `CreateIpForwardEntry2` 前已经存在，调用方只能借用，绝不能在退出时删除。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteInstallOutcome {
    /// 本次 `CreateIpForwardEntry2` 真正创建的行。
    Created(RouteRow),
    /// 本次已创建，但生效行回读与即时回滚均失败；携带提交行与两次错误，调用方必须
    /// 保留 cleanup obligation，并在 teardown 时通过 [`remove_created_pending`] 重试。
    CreatedPending {
        /// 本次成功提交给 `CreateIpForwardEntry2` 的行。
        submitted: RouteRow,
        /// 创建后的生效行回读错误。
        readback_error: NativeError,
        /// 回读失败后的即时回滚错误。
        rollback_error: NativeError,
    },
    /// 系统中已经存在、由本次连接借用的行。
    Borrowed(RouteRow),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateDisposition {
    Created,
    Borrowed,
}

/// 捕获 `interface_luid` 上的全部 IPv4 路由行（read-back 证明：API 成功不是证明，
/// 由调用方对捕获行断言）。
///
/// # Errors
///
/// `GetIpForwardTable2` 失败时返回 [`NativeError`]。
pub fn capture_rows(interface_luid: u64) -> Result<Vec<RouteRow>, NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.capture_rows");
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: table 由系统分配；用毕必须 FreeMibTable（facts §3 同款读表路径）。
    let rc = unsafe { GetIpForwardTable2(AF_INET, &raw mut table) }.0;
    if rc != 0 {
        return Err(NativeError::from_win32(rc, "GetIpForwardTable2 失败"));
    }
    if table.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: table 由系统填充；NumEntries 界内访问（显式 from_raw_parts）。
    let table_ref = unsafe { &*table };
    let rows = unsafe {
        std::slice::from_raw_parts(table_ref.Table.as_ptr(), table_ref.NumEntries as usize)
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        // SAFETY: 读 union 成员 InterfaceLuid.Value。
        if unsafe { r.InterfaceLuid.Value } != interface_luid {
            continue;
        }
        if let Some(row) = from_api_row(r) {
            out.push(row);
        }
    }
    // SAFETY: 释放系统分配的表。
    unsafe { FreeMibTable(table.cast()) };
    Ok(out)
}

/// 安装一条精确路由行（`CreateIpForwardEntry2`：Initialize + 显式
/// `SitePrefixLength=0` + 网络字节序前缀 + `MIB_IPPROTO_NETMGMT`，facts §3）。
///
/// 重复安装同一精确行失败并携带 5010（`ERROR_OBJECT_ALREADY_EXISTS`）。
///
/// # Errors
///
/// 行非法（前缀 > 32 -> 87）或 `CreateIpForwardEntry2` 失败（如重复精确行 -> 5010）
/// 时返回 [`NativeError`]。
pub fn install(row: &RouteRow) -> Result<(), NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.install");
    if row.prefix_len > MAX_PREFIX_LEN {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "install: 非法前缀（IPv4 前缀上限 32）",
        ));
    }
    let api_row = to_api_row(row);
    // SAFETY: api_row 已完整构造（Initialize + 全部关键字段）；失败由 rc 表达。
    let rc = unsafe { CreateIpForwardEntry2(&raw const api_row) }.0;
    if rc != 0 {
        return Err(NativeError::from_win32(
            rc,
            "CreateIpForwardEntry2 失败（重复精确行 -> 5010）",
        ));
    }
    Ok(())
}

/// 安装一条路由并返回本次连接是否真正取得其所有权。
///
/// `CreateIpForwardEntry2` 成功才返回 [`RouteInstallOutcome::Created`]；返回
/// `ERROR_OBJECT_ALREADY_EXISTS` 时回读已有精确行并返回
/// [`RouteInstallOutcome::Borrowed`]。两条路径都返回 Windows 生效行，避免请求 metric
/// 与接口自动 metric 合成后不一致而无法精确清理。
///
/// 创建成功后的回读若失败，本函数会立即用刚提交的完整 API 行回滚；回滚也失败时再
/// 走全表观察：找到稳定身份一致的生效行则返回 `Created`，确认不存在则返回原回读错误，
/// 全表观察也失败才返回 [`RouteInstallOutcome::CreatedPending`]，把提交行和两次错误一并
/// 交给调用方持有，避免可能残留的 created 行失去后续清理主体。
///
/// # Errors
///
/// 行非法、创建失败、已有行无法精确回读，或创建后回读失败但回滚/替代观察已确认无
/// cleanup obligation 时返回 [`NativeError`]。三个观察/回滚路径都无法收敛时由
/// `CreatedPending` 表达。
pub fn install_with_outcome(row: &RouteRow) -> Result<RouteInstallOutcome, NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.install_with_outcome");
    if row.prefix_len > MAX_PREFIX_LEN {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "install_with_outcome: 非法前缀（IPv4 前缀上限 32）",
        ));
    }

    let api_row = to_api_row(row);
    // SAFETY: api_row 已完整构造；返回码明确区分本次创建与既有行。
    let create_code = unsafe { CreateIpForwardEntry2(&raw const api_row) }.0;
    let disposition = classify_create_result(create_code)?;

    let actual = match read_back_effective_row(row) {
        Ok(actual) => actual,
        Err(readback_error) if disposition == CreateDisposition::Created => {
            // Create 已成功但尚未把行交给 RouteOwner；在返回 Err 前必须自行回滚，
            // 不能让一条未登记的 owned 路由遗留在系统中。
            // SAFETY: api_row 正是本调用刚提交成功的完整行；Delete 的 key 为
            // destination/interface/next-hop，metric 等非 key 字段不妨碍删除。
            let rollback_code = unsafe { DeleteIpForwardEntry2(&raw const api_row) }.0;
            if rollback_code == 0 || rollback_code == ERROR_NOT_FOUND {
                return Err(readback_error);
            }

            // 精确 GetIpForwardEntry2 与按提交行 Delete 都失败时，再用独立的全表观察
            // 收敛状态。若仍能找到稳定身份一致的生效行，直接把它作为 Created 交给
            // owner；若全表已确认不存在，则即时回滚虽返回错误，实际上已无 cleanup
            // obligation。只有全表观察本身也失败，才保留 CreatedPending。
            match capture_rows(row.interface_luid) {
                Ok(rows) => {
                    if let Some(actual) = rows.into_iter().find(|actual| {
                        same_route_identity(row, actual) && row.protocol == actual.protocol
                    }) {
                        return Ok(RouteInstallOutcome::Created(actual));
                    }
                    return Err(readback_error);
                }
                Err(_recovery_readback_error) => {}
            }
            return Ok(RouteInstallOutcome::CreatedPending {
                submitted: row.clone(),
                readback_error,
                rollback_error: NativeError::from_win32(
                    rollback_code,
                    "install_with_outcome: created 行即时回滚失败",
                ),
            });
        }
        Err(read_error) => return Err(read_error),
    };

    Ok(match disposition {
        CreateDisposition::Created => RouteInstallOutcome::Created(actual),
        CreateDisposition::Borrowed => RouteInstallOutcome::Borrowed(actual),
    })
}

/// 清理由 [`RouteInstallOutcome::CreatedPending`] 保留的 created 路由义务。
///
/// pending 行没有可信的生效 metric，因此改走全表捕获，按提交时稳定身份
/// `(destination, prefix, interface LUID, next-hop, protocol)` 找到生效行，再经普通精确
/// remove 删除。身份不一致时不删除，避免把后来出现的第三方路由当作本次 created 行。
///
/// # Errors
///
/// 回读失败、身份不一致或删除失败时返回 [`NativeError`]。
pub fn remove_created_pending(submitted: &RouteRow) -> Result<RemoveOutcome, NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.remove_created_pending");
    let actual = capture_rows(submitted.interface_luid)?
        .into_iter()
        .find(|actual| {
            same_route_identity(submitted, actual) && submitted.protocol == actual.protocol
        });
    match actual {
        Some(actual) => remove(&actual),
        None => Ok(RemoveOutcome::AlreadyAbsent),
    }
}

fn classify_create_result(code: u32) -> Result<CreateDisposition, NativeError> {
    match code {
        0 => Ok(CreateDisposition::Created),
        ERROR_OBJECT_ALREADY_EXISTS => Ok(CreateDisposition::Borrowed),
        other => Err(NativeError::from_win32(
            other,
            "CreateIpForwardEntry2 失败",
        )),
    }
}

fn read_back_effective_row(requested: &RouteRow) -> Result<RouteRow, NativeError> {
    let mut api_row = to_api_row(requested);
    // SAFETY: api_row 带 destination/interface/next-hop 精确身份；成功时系统填入 metric
    // 等生效字段。
    let code = unsafe { GetIpForwardEntry2(&raw mut api_row) }.0;
    if code != 0 {
        return Err(NativeError::from_win32(
            code,
            "install_with_outcome: GetIpForwardEntry2 回读失败",
        ));
    }
    let actual = from_api_row(&api_row).ok_or_else(|| {
        NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "install_with_outcome: 回读行不是 IPv4 路由",
        )
    })?;
    if !same_route_identity(requested, &actual) {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "install_with_outcome: 回读行身份与请求不一致",
        ));
    }
    Ok(actual)
}

#[must_use]
fn same_route_identity(requested: &RouteRow, actual: &RouteRow) -> bool {
    requested.dest_key() == actual.dest_key()
        && requested.next_hop == actual.next_hop
        && requested.interface_luid == actual.interface_luid
}

/// 删除一条精确路由行。
///
/// 删除前先用 `GetIpForwardEntry2` 取精确行（facts §3：dest-only 直接 Delete 返回
/// 2——'delete by CIDR' mutant 在此死）；通配填满的行必须与请求行全字段一致，
/// 不一致 -> Err 且绝不删除。已 absent -> `Ok(RemoveOutcome::AlreadyAbsent)`（幂等）。
///
/// # Errors
///
/// dest-only 通配 key 输入、填满行与请求行不一致、或 Get/Delete 失败时返回
/// [`NativeError`]。
pub fn remove(row: &RouteRow) -> Result<RemoveOutcome, NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.remove");
    // dest-only 通配 key（nexthop/luid 为零）不是合法删除输入：拒绝，绝不静默成功。
    if row.next_hop == Ipv4Addr::UNSPECIFIED && row.interface_luid == 0 {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "remove: dest-only key 不是合法删除输入（facts §3：按 CIDR 删除是 mutant）",
        ));
    }
    let mut api_row = to_api_row(row);
    // SAFETY: api_row 携带请求行的完整 key；Get 成功时按通配/精确匹配填满行
    // （facts §3：dest-only 通配填满、完整 key 精确命中）。
    let rc_get = unsafe { GetIpForwardEntry2(&raw mut api_row) }.0;
    if rc_get == ERROR_NOT_FOUND {
        // 已 absent：合法状态，幂等（facts §3）。
        return Ok(RemoveOutcome::AlreadyAbsent);
    }
    if rc_get != 0 {
        return Err(NativeError::from_win32(rc_get, "GetIpForwardEntry2 失败"));
    }
    // 通配填满的行必须与请求行全字段一致；不一致 -> Err，不得删除。
    let filled = from_api_row(&api_row).ok_or_else(|| {
        NativeError::from_win32(ERROR_NOT_FOUND, "remove: 填满的行不是 IPv4 行，拒绝删除")
    })?;
    if &filled != row {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "remove: GetIpForwardEntry2 填满的行与请求行不一致，拒绝删除",
        ));
    }
    // SAFETY: 行已填满精确行（facts §3 的正确路径 Get -> Delete）。
    let rc_del = unsafe { DeleteIpForwardEntry2(&raw const api_row) }.0;
    if rc_del != 0 {
        return Err(NativeError::from_win32(rc_del, "DeleteIpForwardEntry2 失败"));
    }
    Ok(RemoveOutcome::Removed)
}

/// 批量安装路由行，失败时整批不留半状态（'partial failure' mutant 在此死）。
///
/// 先预验证全部行（任一非法行 -> 整批 87，且不安装任何行）；随后逐行安装，任一
/// 安装失败时按**逆序**回滚已安装的行（cleanup 逆序语义）再返回错误。
///
/// # Errors
///
/// 首个失败行的 [`NativeError`]（非法前缀 -> 87；重复精确行 -> 5010）。
pub fn install_plan(rows: &[RouteRow]) -> Result<(), NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.install_plan");
    // 预验证：任一非法行 -> 整批 87，不安装任何行（无半状态）。
    if rows.iter().any(|r| r.prefix_len > MAX_PREFIX_LEN) {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "install_plan: 非法前缀（IPv4 前缀上限 32）",
        ));
    }
    let mut installed: Vec<&RouteRow> = Vec::with_capacity(rows.len());
    for row in rows {
        if let Err(e) = install(row) {
            // 回滚：已安装的行逆序删除（best-effort），不留半状态。
            for done in installed.iter().rev().copied() {
                let _ = remove(done);
            }
            return Err(e);
        }
        installed.push(row);
    }
    Ok(())
}

/// 网关 bypass `/32` 前缀长度：对网关 IP 的最长前缀覆盖行（W20 冻结事实——
/// 路由表无法从已装网段里"挖掉"单主机，唯一表达是更长的精确行）。
pub const BYPASS_ROUTE_PREFIX: u8 = 32;
/// 网关 bypass `/32` 度量（C++ `native_ip_config.cpp:783-792` 同源 metric 1）。
pub const BYPASS_ROUTE_METRIC: u32 = 1;

/// 由物理网卡发现结果与 VGDC 解析出的网关地址构造 bypass `/32` 精确行
/// （2026-09-08 计划 T1；数据源唯一化：DoH 解析结果，拒绝手填）。
///
/// next-hop = 物理网卡网关、接口 = 物理 LUID——与 VGDC 解析器 / `IP_UNICAST_IF`
/// socket binder 共用同一物理出口发现结果（`find_physical_nics` 一次发现）。
/// **不经无源 `GetBestRoute2`**：上游 TUN（Mihomo 类）默认路由在表时，无源查找
/// 会捕获 TUN 而非物理网卡（facts §3 增补冻结事实，2026-09-08 实测），照抄会把
/// bypass 行装到上游 TUN 上——方向恰好装反。
///
/// fake-ip（`198.18.0.0/15`）网关防御拒绝（fail-closed）：VGDC 双线解析已过滤，
/// 此处纵深防御，绝不给 fake-ip 装"物理直连"行。
///
/// # Errors
///
/// `gateway_ip` 落在 fake-ip 段 → [`NativeError`]（87，拒绝构造）。
pub fn gateway_bypass_row(
    nic: &crate::vgdc_dns::NicInfo,
    gateway_ip: Ipv4Addr,
) -> Result<RouteRow, NativeError> {
    if crate::vgdc_dns::is_fake_ip_v4(gateway_ip) {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "gateway_bypass_row: 拒绝 fake-ip 网关（198.18.0.0/15；VGDC 双线不应产出）",
        ));
    }
    Ok(RouteRow::new(
        gateway_ip,
        BYPASS_ROUTE_PREFIX,
        nic.gateway,
        nic.luid,
        BYPASS_ROUTE_METRIC,
    ))
}

/// 依据已连接 CSTP socket 的真实 `local_ip`，观察 Windows 对 `gateway_ip` 的当前
/// 最佳出口，并构造保持同一 next-hop/interface/metric 的网关 `/32` 行。
///
/// 该入口用于 `SystemSelected`：source 参数不是未指定地址，而是 socket 完成 TCP
/// 建链后回读的本地 IPv4。这样 `GetBestRoute2` 的裁决绑定实际 source，避免无源查询
/// 在多 NIC / 上游 TUN 环境中选择另一条路径。返回的 `RouteRow` 是待安装的 `/32`
/// NETMGMT 行；第二个值是同一 LUID 转换出的接口索引，供调用方记录 expected/actual
/// 出口并识别 EXV 自回环。
///
/// 本函数只观察 Windows 路由事实，不推断 WinINet/WinHTTP/SOCKS 代理，也不取得所选
/// 接口的生命周期所有权。
///
/// # Errors
///
/// `GetBestRoute2` 失败、回报的 best source 与 socket source 不一致、返回非 IPv4
/// next-hop，或 LUID 无法转换为接口索引时返回 [`NativeError`]。
pub fn observe_gateway_egress(
    gateway_ip: Ipv4Addr,
    local_ip: Ipv4Addr,
) -> Result<(RouteRow, u32), NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.observe_gateway_egress");
    let source = to_sockaddr(local_ip);
    let destination = to_sockaddr(gateway_ip);
    let mut best_route = MIB_IPFORWARD_ROW2::default();
    let mut best_source = SOCKADDR_INET::default();
    // SAFETY: source/destination 是存活的 IPv4 SOCKADDR_INET；两个输出缓冲区均由
    // 调用方分配并在调用期间独占。LUID/index 均不预设，让 Windows 按给定 source
    // 与 destination 选择当前实际最佳路由。
    let code = unsafe {
        GetBestRoute2(
            None,
            0,
            Some(&raw const source),
            &raw const destination,
            0,
            &raw mut best_route,
            &raw mut best_source,
        )
    }
    .0;
    if code != 0 {
        return Err(NativeError::from_win32(
            code,
            "observe_gateway_egress: GetBestRoute2 失败",
        ));
    }

    let selected_source = ipv4_of(&best_source).ok_or_else(|| {
        NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "observe_gateway_egress: best source 不是 IPv4",
        )
    })?;
    if selected_source != local_ip {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "observe_gateway_egress: best source 与已连接 socket source 不一致",
        ));
    }

    let next_hop = ipv4_of(&best_route.NextHop).ok_or_else(|| {
        NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "observe_gateway_egress: best route next-hop 不是 IPv4",
        )
    })?;
    // SAFETY: GetBestRoute2 成功后 InterfaceLuid 是已填充的输出字段。
    let interface_luid = unsafe { best_route.InterfaceLuid.Value };
    let mut ifindex = 0u32;
    // SAFETY: best_route 的 LUID 来自成功的 GetBestRoute2；ifindex 是有效输出地址。
    let index_code = unsafe {
        ConvertInterfaceLuidToIndex(&best_route.InterfaceLuid, &raw mut ifindex)
    }
    .0;
    if index_code != 0 {
        return Err(NativeError::from_win32(
            index_code,
            "observe_gateway_egress: ConvertInterfaceLuidToIndex 失败",
        ));
    }

    Ok((
        gateway_egress_row_from_observation(
            gateway_ip,
            next_hop,
            interface_luid,
            best_route.Metric,
        ),
        ifindex,
    ))
}

#[must_use]
fn gateway_egress_row_from_observation(
    gateway_ip: Ipv4Addr,
    next_hop: Ipv4Addr,
    interface_luid: u64,
    metric: u32,
) -> RouteRow {
    RouteRow::new(
        gateway_ip,
        BYPASS_ROUTE_PREFIX,
        next_hop,
        interface_luid,
        metric,
    )
}

/// 全表只读扫描 `(dest, prefix_len)` 的全部 IPv4 精确行（**跨接口**）。
///
/// 结果只供诊断/盘点；路由 owner 不得据此按 destination 批量删除，因为返回行可能
/// 属于系统、上游 TUN 或其它 VPN。
///
/// # Errors
///
/// `GetIpForwardTable2` 失败时返回 [`NativeError`]。
pub fn find_rows_for_dest(dest: Ipv4Addr, prefix_len: u8) -> Result<Vec<RouteRow>, NativeError> {
    let _t = crate::timing::Timed::new("resource.routes.find_rows_for_dest");
    let wanted = masked_network(dest, prefix_len);
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: table 由系统分配；用毕必须 FreeMibTable（capture_rows 同款读表路径）。
    let rc = unsafe { GetIpForwardTable2(AF_INET, &raw mut table) }.0;
    if rc != 0 {
        return Err(NativeError::from_win32(rc, "GetIpForwardTable2 失败"));
    }
    if table.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: table 由系统填充；NumEntries 界内访问（显式 from_raw_parts）。
    let table_ref = unsafe { &*table };
    let rows = unsafe {
        std::slice::from_raw_parts(table_ref.Table.as_ptr(), table_ref.NumEntries as usize)
    };
    let mut out = Vec::new();
    for r in rows {
        if let Some(row) = from_api_row(r) {
            if row.network == wanted && row.prefix_len == prefix_len {
                out.push(row);
            }
        }
    }
    // SAFETY: 释放系统分配的表。
    unsafe { FreeMibTable(table.cast()) };
    Ok(out)
}

/// 构建安装计划：bypass 行永远排在全部隧道路由**之前**（先 bypass 再 tunnel，
/// 否则控制流量会被隧道路由劫持——'bypass after default route' mutant 在此死）。
/// 无 bypass 时计划就是隧道路由本身。
#[must_use]
pub fn build_install_plan(bypass: Option<&RouteRow>, tunnel: &[RouteRow]) -> Vec<RouteRow> {
    let mut plan = Vec::with_capacity(tunnel.len() + usize::from(bypass.is_some()));
    if let Some(b) = bypass {
        plan.push(b.clone());
    }
    plan.extend_from_slice(tunnel);
    plan
}

/// 构建清理计划：安装顺序的**逆序**（最后安装的先删除——'cleanup in forward
/// order' mutant 在此死）。行保持精确身份（全字段行，不按 CIDR 重新配对）。
#[must_use]
pub fn build_cleanup_order(installed: &[RouteRow]) -> Vec<RouteRow> {
    installed.iter().rev().cloned().collect()
}

/// IPv4 `SOCKADDR_INET`（`S_addr` 用 `from_le_bytes`：内存中按网络字节序存储，
/// facts §3 实测修正；用 `from_be_bytes` 会把 10.99.99.0 写成 0.99.99.10）。
#[must_use]
pub(crate) fn to_sockaddr(ip: Ipv4Addr) -> SOCKADDR_INET {
    let mut sa = SOCKADDR_INET::default();
    // edition 2024：局部变量上的 union Copy 字段写是安全操作（WSP4 spike 同款）。
    sa.Ipv4.sin_family = AF_INET;
    sa.Ipv4.sin_addr.S_un.S_addr = u32::from_le_bytes(ip.octets());
    sa
}

/// 把系统 `MIB_IPFORWARD_ROW2` 转为 `RouteRow`（非 IPv4 行返回 None——无法表达）。
/// 供本模块（`GetIpForwardTable2` 读表路径）共用。
#[must_use]
pub(crate) fn from_api_row(api: &MIB_IPFORWARD_ROW2) -> Option<RouteRow> {
    let network = ipv4_of(&api.DestinationPrefix.Prefix)?;
    let next_hop = ipv4_of(&api.NextHop)?;
    // SAFETY: 读 union 成员 InterfaceLuid.Value。
    let interface_luid = unsafe { api.InterfaceLuid.Value };
    Some(RouteRow {
        network,
        prefix_len: api.DestinationPrefix.PrefixLength,
        next_hop,
        interface_luid,
        metric: api.Metric,
        protocol: u32::try_from(api.Protocol.0).unwrap_or(0),
    })
}

/// 屏蔽主机位；前缀 > 32 的行保持原样（非法行由安装路径以 87 拒绝）。
#[must_use]
fn masked_network(network: Ipv4Addr, prefix_len: u8) -> Ipv4Addr {
    let mask = match prefix_len {
        0 => 0,
        1..=32 => u32::MAX << (32 - prefix_len),
        _ => u32::MAX,
    };
    Ipv4Addr::from(u32::from(network) & mask)
}

/// 构造 `CreateIpForwardEntry2` 用的精确行（WSP4 实测成功变体 `init+full`：
/// Initialize + dest/next-hop/luid/metric/lifetime + 显式 `SitePrefixLength=0` +
/// `MIB_IPPROTO_NETMGMT`；直接 default 的行带 SitePrefixLength=255 等哨兵会让
/// Create 返回 87）。
#[must_use]
fn to_api_row(row: &RouteRow) -> MIB_IPFORWARD_ROW2 {
    let mut api = MIB_IPFORWARD_ROW2::default();
    // SAFETY: Initialize 把行初始化为合法基线（facts §3 实测修正）。
    unsafe { InitializeIpForwardEntry(&raw mut api) };
    api.DestinationPrefix.Prefix = to_sockaddr(row.network);
    api.DestinationPrefix.PrefixLength = row.prefix_len;
    api.NextHop = to_sockaddr(row.next_hop);
    api.InterfaceLuid = NET_LUID_LH { Value: row.interface_luid };
    api.Metric = row.metric;
    api.Protocol = MIB_IPPROTO_NETMGMT;
    api.SitePrefixLength = 0; // IPv4 必须显式 0（facts §3 实测修正）
    api.ValidLifetime = u32::MAX;
    api.PreferredLifetime = u32::MAX;
    api.Origin = NlroManual;
    api
}

/// `SOCKADDR_INET` -> IPv4 地址（非 `AF_INET` 返回 None）。
#[must_use]
fn ipv4_of(sa: &SOCKADDR_INET) -> Option<Ipv4Addr> {
    // SAFETY: si_family 是 union 的公共初始成员。
    if unsafe { sa.si_family } != AF_INET {
        return None;
    }
    // SAFETY: AF_INET 分支下 sin_addr 有效；S_addr 按网络字节序存于内存
    // （x86 LE：to_le_bytes 还原字节顺序，facts §3 实测修正）。
    let octets = unsafe { sa.Ipv4.sin_addr.S_un.S_addr }.to_le_bytes();
    Some(Ipv4Addr::from(octets))
}

// ---------------------------------------------------------------------------
// 单元测试（纯构造，零 syscall）：bypass 行形状、fake-ip 防御、安装计划排序。
// 真实表扫描（find_rows_for_dest）与安装/移除契约由 tests/routes.rs（admin 门禁）
// 与真机业务流门禁覆盖；source-bound GetBestRoute2 用本机 loopback 测试覆盖。
// ---------------------------------------------------------------------------

