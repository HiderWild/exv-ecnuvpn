
use std::io::Read;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use exv_vpn_win32_resource::native_error::NativeError;
use exv_vpn_win32_resource::routes::{
    build_cleanup_order, build_install_plan, capture_rows, find_rows_for_dest, install,
    install_plan, remove, RemoveOutcome, RouteRow,
};
use exv_vpn_win32_resource::wintun_adapter::{AdapterOpen, WintunAdapter};
use exv_vpn_win32_resource::wintun_api::WintunLibrary;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// 冻结的 amd64 wintun-0.14.1 DLL 精确路径（WSP3 冻结值；PATH DLL 是 mutant）。
const FROZEN_DLL_PATH: &str = "C:\\Users\\user\\.exv\\wintun\\wintun\\bin\\amd64\\wintun.dll";
/// 与 WSP3/WSP4 探针一致的 tunnel type。
const TUNNEL_TYPE: &str = "EXV VPN";
/// scratch adapter 的测试地址（WSP4 spike 同款；nexthop 指向它）。
const PROBE_IP: Ipv4Addr = Ipv4Addr::new(10, 88, 88, 1);
/// 隧道路由目标网络（WSP4 spike 同款；GetBestRoute2 探测目的 10.99.99.2 在 facts §3）。
const TUNNEL_NET: Ipv4Addr = Ipv4Addr::new(10, 99, 99, 0);
const TUNNEL_PREFIX: u8 = 24;
/// WSP4 facts §3 的 GetBestRoute2 探测目的地址。
const CONTROL_DEST: Ipv4Addr = Ipv4Addr::new(10, 99, 99, 2);
/// WSP4 冻结错误码：重复精确行（ERROR_OBJECT_ALREADY_EXISTS）、非法前缀（ERROR_INVALID_PARAMETER）。
const ERROR_OBJECT_ALREADY_EXISTS: u32 = 5010;
const ERROR_INVALID_PARAMETER: u32 = 87;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 取 Err 并 panic 掉意外 Ok（不给 T 强加 Debug bound）。
fn expect_native_error<T>(r: Result<T, NativeError>, ctx: &str) -> NativeError {
    match r {
        Err(e) => e,
        Ok(_) => panic!("{ctx}: 必须返回 Err(NativeError)"),
    }
}

/// 取 Ok 并 panic 掉意外 Err（不给 T 强加 Debug bound）。
fn expect_ok<T>(r: Result<T, NativeError>, ctx: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{ctx}: 失败 {e:?}"),
    }
}

/// 当前进程是否 elevated（admin token）——模式与 W16-T / W17-T / WSP4 spike 一致。
fn is_elevated() -> bool {
    use std::ffi::c_void;
    // SAFETY: GetCurrentProcess 返回伪句柄，无需关闭。
    let proc_h = unsafe { GetCurrentProcess() };
    let mut token = HANDLE::default();
    // SAFETY: OpenProcessToken 写入 token 句柄；成功后需关闭。
    let ok = unsafe { OpenProcessToken(proc_h, TOKEN_QUERY, &mut token) };
    if ok.is_err() {
        return false;
    }
    let mut elevated = false;
    let mut size = 0u32;
    // SAFETY: 首次查询请求所需缓冲区大小（输出为 size）。
    unsafe {
        let _ = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::null_mut()),
            0,
            &mut size,
        );
    }
    let mut buff = vec![0u8; size as usize];
    // SAFETY: buff 是有效缓冲；TokenElevation 返回 TOKEN_ELEVATION { TokenIsElevated: BOOL }。
    let ok2 = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(buff.as_mut_ptr().cast::<c_void>()),
            buff.len() as u32,
            &mut size,
        )
    };
    if ok2.is_ok() && buff.len() >= 4 {
        elevated = u32::from_ne_bytes(buff[0..4].try_into().unwrap_or([0u8; 4])) != 0;
    }
    // SAFETY: token 是本进程新打开句柄，使用后关闭。
    unsafe { let _ = CloseHandle(token); }
    elevated
}

/// 动态断言的前置：非 elevated 时输出显式 `not_run / blocked_by_environment` 并短路。
fn require_admin(test: &str) -> bool {
    if is_elevated() {
        return true;
    }
    eprintln!(
        "[not_run/blocked_by_environment] {test}: 创建/修改真实路由需要提权（elevated admin \
         token）；当前进程非 elevated，跳过动态断言"
    );
    false
}

/// 冻结 DLL 路径，允许 WSP4 facts §7 的环境变量覆盖。
fn dll_path() -> PathBuf {
    match std::env::var_os("EXV_RUST_VPN_WINTUN_DLL") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(FROZEN_DLL_PATH),
    }
}

/// 加载冻结 DLL 的便捷入口（LoadLibraryW 引用计数安全）。
fn load_frozen() -> WintunLibrary {
    expect_ok(WintunLibrary::load(&dll_path()), "load 冻结 wintun.dll")
}

/// 创建 scratch adapter（创建者 owned；drop 即移除 adapter 及其全部路由）。
fn create_scratch_adapter(lib: &WintunLibrary, prefix: &str) -> WintunAdapter {
    let name = format!("{prefix}-{}", std::process::id());
    let (adapter, opened) =
        expect_ok(WintunAdapter::create(lib, &name, TUNNEL_TYPE), "create scratch adapter");
    assert!(
        matches!(opened, AdapterOpen::Created),
        "create 必须返回 AdapterOpen::Created"
    );
    adapter
}

/// 带 watchdog 的子进程调用：超时后 kill 子进程并返回 None（W17-T / WSP3 spike 同款；
/// 本机实测 netsh 在 Wintun 接口上可能无限阻塞，必须 watchdog）。
/// 返回 (退出成功, stdout+stderr 文本)。
fn run_cmd_checked(program: &str, args: &[String], timeout: Duration) -> Option<(bool, String)> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None; // 超时
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(_) => return None,
        }
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    let _ = child.wait();
    Some((status.success(), format!("{out}{err}")))
}

/// scratch 接口前置（WSP4 spike 同款配置，全部带 watchdog）：赋地址 10.88.88.1/24 +
/// 启用接口。**路由一律经被测试 seam 安装**，netsh 只做接口前置（生产实现不得 netsh）。
fn configure_scratch_interface(ifname: &str) -> Result<(), String> {
    let mut set_ok = false;
    for _attempt in 0..4 {
        let args = [
            "interface".to_string(),
            "ip".to_string(),
            "set".to_string(),
            "address".to_string(),
            format!("name={ifname}"),
            "source=static".to_string(),
            "addr=10.88.88.1".to_string(),
            "mask=255.255.255.0".to_string(),
        ];
        if let Some((true, _)) = run_cmd_checked("netsh", &args, Duration::from_secs(12)) {
            set_ok = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    if !set_ok {
        return Err("netsh set address 失败/超时（watchdog 已 kill）".to_string());
    }
    let enable_args = [
        "interface".to_string(),
        "set".to_string(),
        "interface".to_string(),
        format!("name={ifname}"),
        "admin=enabled".to_string(),
    ];
    let _enable = run_cmd_checked("netsh", &enable_args, Duration::from_secs(10));
    Ok(())
}

/// 按 CIDR 身份（network + prefix_len）过滤行——read-back 断言用的查找谓词。
fn has_cidr(rows: &[RouteRow], network: Ipv4Addr, prefix_len: u8) -> bool {
    rows.iter()
        .any(|r| r.network == network && r.prefix_len == prefix_len)
}

// ---------------------------------------------------------------------------
// 纯逻辑契约（任何宿主都必须通过；行身份 / 顺序 / 计划构建）
// ---------------------------------------------------------------------------

/// RouteRow::new 必须规范化：屏蔽主机位（网络字节序/主机位 mutant 在此死——WSP4 facts §3：
/// 前缀含主机位会让 CreateIpForwardEntry2 返回 87）；protocol 必须为 3（MIB_IPPROTO_NETMGMT）。
/// 行相等必须是**全字段**相等（只按 CIDR 比较 = mutant）。
#[test]
fn route_row_new_normalizes_network_and_protocol() {
    let row = RouteRow::new(
        Ipv4Addr::new(10, 99, 99, 7), // 带主机位
        24,
        PROBE_IP,
        0x1234,
        5,
    );
    assert_eq!(
        row.network,
        TUNNEL_NET,
        "new() 必须屏蔽主机位（10.99.99.7/24 -> 10.99.99.0），否则 Create 返回 87"
    );
    assert_eq!(row.prefix_len, 24);
    assert_eq!(row.next_hop, PROBE_IP);
    assert_eq!(row.interface_luid, 0x1234);
    assert_eq!(row.metric, 5);
    assert_eq!(
        row.protocol, 3,
        "协议必须为 3（MIB_IPPROTO_NETMGMT，facts §3 回读冻结值）"
    );

    // 全字段相等：同一 CIDR 不同 next-hop / metric 的行不得相等（按 CIDR 判等会漏删错行）。
    let other_nh = RouteRow::new(TUNNEL_NET, 24, Ipv4Addr::new(10, 88, 88, 2), 0x1234, 5);
    assert_ne!(row, other_nh, "相同 CIDR 不同 next-hop 必须不相等");
    let other_metric = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, 0x1234, 6);
    assert_ne!(row, other_metric, "相同 CIDR 不同 metric 必须不相等");
    let other_luid = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, 0x5678, 5);
    assert_ne!(row, other_luid, "相同 CIDR 不同 interface_luid 必须不相等");
}

/// dest_only 构造的是通配 key（next_hop/luid 为零，facts §3 冻结），绝不等于精确行；
/// 两者只共享 CIDR 身份。这钉死 'delete by CIDR'：删除路径只能接受精确行。
#[test]
fn dest_only_row_is_not_an_exact_row() {
    let exact = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, 0x7777, 5);
    let dest_only = RouteRow::dest_only(TUNNEL_NET, 24);
    assert_eq!(
        dest_only.next_hop,
        Ipv4Addr::UNSPECIFIED,
        "dest-only key 的 next_hop 必须为零（facts §3 通配匹配）"
    );
    assert_eq!(
        dest_only.interface_luid, 0,
        "dest-only key 的 interface_luid 必须为零（facts §3 通配匹配）"
    );
    assert_ne!(
        dest_only, exact,
        "dest-only（nexthop/luid 为零）是通配 key，不是精确行——按 CIDR 删除是 mutant"
    );
    assert_eq!(
        dest_only.dest_key(),
        exact.dest_key(),
        "dest-only 与精确行共享 CIDR 身份（供查找），但行身份必须不同"
    );
}

/// 安装计划：bypass 行必须排在全部隧道路由**之前**（'bypass after default route' /
/// 'GetBestRoute2 after tunnel route' mutant 在此死）。无 bypass 时计划就是隧道路由本身。
#[test]
fn install_plan_orders_bypass_before_tunnel_routes() {
    let bypass = RouteRow::new(Ipv4Addr::UNSPECIFIED, 0, Ipv4Addr::new(198, 18, 0, 2), 0xABC, 0);
    let t1 = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, 0x7777, 5);
    let t2 = RouteRow::new(Ipv4Addr::new(10, 88, 88, 0), 24, PROBE_IP, 0x7777, 5);

    let with_bypass = [bypass.clone(), t1.clone(), t2.clone()];
    assert_eq!(
        build_install_plan(Some(&bypass), &[t1.clone(), t2.clone()]),
        with_bypass,
        "bypass 路由必须排在隧道路由之前（先 bypass 再 tunnel，否则控制流量被隧道路由劫持）"
    );

    let without_bypass = [t1.clone(), t2.clone()];
    assert_eq!(
        build_install_plan(None, &[t1.clone(), t2.clone()]),
        without_bypass,
        "无 bypass 时计划必须就是隧道路由本身"
    );
}

/// cleanup 顺序必须是安装顺序的**逆序**（最后安装的先删）；行身份保持精确（全字段）。
/// 'cleanup in forward order' mutant 在此死。
#[test]
fn cleanup_plan_reverses_install_order() {
    let bypass = RouteRow::new(Ipv4Addr::UNSPECIFIED, 0, Ipv4Addr::new(198, 18, 0, 2), 0xABC, 0);
    let t1 = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, 0x7777, 5);
    let t2 = RouteRow::new(Ipv4Addr::new(10, 88, 88, 0), 24, PROBE_IP, 0x7777, 5);
    let installed = [bypass.clone(), t1.clone(), t2.clone()];

    let cleanup = build_cleanup_order(&installed);
    assert_eq!(
        cleanup,
        vec![t2.clone(), t1.clone(), bypass.clone()],
        "cleanup 必须按安装顺序的逆序执行（最后安装的先删除）"
    );
    // 逆序必须是精确行（全字段）而非只按 CIDR 重新配对。
    assert!(cleanup.iter().all(|r| installed.contains(r)), "cleanup 必须只含已安装的精确行");
}

// ---------------------------------------------------------------------------
// 真实路由契约（需要 admin；非 elevated 记为 not_run）
// ---------------------------------------------------------------------------

/// `find_rows_for_dest` 跨接口全表扫描（2026-09-08 计划 I5 崩溃清扫的读表基础）：
/// scratch adapter 上安装的 `/32` 行必须被跨接口扫描找到（不按调用方 luid 过滤），
/// 移除后不再出现。杀死 'sweep scoped to current nic only' mutant（上次崩溃遗留
/// 行挂在旧物理网卡上时，只扫当前 luid 会漏掉遗留行）。
///
/// （原 `capture_bypass_before_tunnel_route_install`（无源 `GetBestRoute2` 捕获证据）
/// 随 `bypass_route` 模块退役删除：2026-09-08 计划——无源捕获在上游 TUN 默认路由
/// 在表时会抓到 TUN，不再作为生产原语；证据转入 facts 文档与真机门禁。）
#[test]
fn find_rows_for_dest_scans_across_interfaces() {
    if !require_admin("find_rows_for_dest_scans_across_interfaces") {
        return;
    }
    let lib = load_frozen();
    let adapter = create_scratch_adapter(&lib, "ExvW20Bypass");
    configure_scratch_interface(adapter.alias())
        .expect("scratch 接口配置失败（无法安装路由）：netsh 前置失败");

    let dest = CONTROL_DEST;
    let row = RouteRow::new(dest, 32, PROBE_IP, adapter.luid(), 1);
    expect_ok(install(&row), "install /32 scratch route");

    // 跨接口扫描：行挂在 scratch adapter luid 上，调用方不传任何 luid——必须找到。
    let found = expect_ok(find_rows_for_dest(dest, 32), "find rows for dest");
    assert!(
        found.contains(&row),
        "跨接口扫描必须找到 scratch adapter 上的 /32 行：found={found:?} row={row:?}"
    );

    // cleanup：经 seam 精确删除，再扫必须不含。
    match expect_ok(remove(&row), "cleanup remove /32 route") {
        RemoveOutcome::Removed | RemoveOutcome::AlreadyAbsent => {}
    }
    let after = expect_ok(find_rows_for_dest(dest, 32), "re-scan after remove");
    assert!(
        !after.contains(&row),
        "移除后跨接口扫描不得再出现该行：after={after:?}"
    );
    drop(adapter);
}

/// install -> 回读精确行 -> remove -> 回读 absent 的完整往返；重复 Create 同一精确行 ->
/// 5010；再删已 absent 路由 -> Ok(AlreadyAbsent)（幂等）。
/// 杀死 'install without effect / wrong row capture' 与 'already-absent treated as error'。
#[test]
fn install_readback_roundtrip_exact_row() {
    if !require_admin("install_readback_roundtrip_exact_row") {
        return;
    }
    let lib = load_frozen();
    let adapter = create_scratch_adapter(&lib, "ExvW20Roundtrip");
    configure_scratch_interface(adapter.alias())
        .expect("scratch 接口配置失败（无法安装路由）：netsh 前置失败");

    let tunnel = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, adapter.luid(), 5);

    // install：精确行必须创建成功。
    expect_ok(install(&tunnel), "install exact row");

    // already-exists：重复 Create 同一精确行 -> 5010（facts §3 冻结）。
    let dup = expect_native_error(install(&tunnel), "duplicate install");
    assert_eq!(
        dup.code, ERROR_OBJECT_ALREADY_EXISTS,
        "重复 Create 同一精确行必须 5010（ERROR_OBJECT_ALREADY_EXISTS）, got {}",
        dup.code
    );

    // read-back：scratch LUID 上恰好一行该 CIDR，且全字段等于安装的精确行
    // （network/prefix/nexthop/luid/metric/protocol 全部回读——字节序/主机位/假行 mutant 在此死）。
    let rows = expect_ok(capture_rows(adapter.luid()), "capture_rows after install");
    let matched: Vec<&RouteRow> = rows
        .iter()
        .filter(|r| r.network == TUNNEL_NET && r.prefix_len == TUNNEL_PREFIX)
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "scratch LUID 上必须恰好一行 10.99.99.0/24, got {rows:?}"
    );
    assert_eq!(
        *matched[0], tunnel,
        "回读必须是安装的精确行（nexthop/luid/metric/protocol 全部匹配）"
    );

    // remove -> Removed；回读 absent。
    assert_eq!(
        expect_ok(remove(&tunnel), "remove exact row"),
        RemoveOutcome::Removed
    );
    let after = expect_ok(capture_rows(adapter.luid()), "capture_rows after remove");
    assert!(
        !has_cidr(&after, TUNNEL_NET, TUNNEL_PREFIX),
        "删除后回读必须 absent（API success 不是 proof）"
    );

    // already-absent：再删同一行必须 Ok(AlreadyAbsent)，不是 Err。
    assert_eq!(
        expect_ok(remove(&tunnel), "remove again (already absent)"),
        RemoveOutcome::AlreadyAbsent,
        "已 absent 的路由再删除必须幂等为 Ok(AlreadyAbsent)"
    );

    drop(adapter);
}

/// dest-only（CIDR-only）行不是合法删除输入：必须 Err，且精确行必须原样保留
/// （绝不静默成功、绝不删除）。facts §3：dest-only 行直接 Delete 返回 2。
/// 杀死 'delete by CIDR（删错行/半状态）' mutant。
#[test]
fn remove_dest_only_key_is_typed_error_and_leaves_route() {
    if !require_admin("remove_dest_only_key_is_typed_error_and_leaves_route") {
        return;
    }
    let lib = load_frozen();
    let adapter = create_scratch_adapter(&lib, "ExvW20Cidr");
    configure_scratch_interface(adapter.alias())
        .expect("scratch 接口配置失败（无法安装路由）：netsh 前置失败");

    let tunnel = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, adapter.luid(), 5);
    expect_ok(install(&tunnel), "install exact row");

    // 按 CIDR 删除：dest-only 通配 key（next_hop/luid 为零）必须被拒绝（typed Err），
    // 不得静默成功。
    let dest_only = RouteRow::dest_only(TUNNEL_NET, TUNNEL_PREFIX);
    let err = expect_native_error(remove(&dest_only), "remove by dest-only key");
    assert_ne!(
        err.code, 0,
        "dest-only 删除必须 Err（facts §3：dest-only 直接 Delete 返回 2），不得静默成功"
    );

    // 精确行必须仍在（无半状态、无删错行）。
    let rows = expect_ok(capture_rows(adapter.luid()), "capture_rows after dest-only remove");
    assert!(
        has_cidr(&rows, TUNNEL_NET, TUNNEL_PREFIX),
        "dest-only 删除不得移除任何行——'delete by CIDR' mutant 在此死"
    );

    // 拒绝后系统状态必须干净：精确行删除仍能正常工作。
    assert_eq!(
        expect_ok(remove(&tunnel), "remove exact row afterwards"),
        RemoveOutcome::Removed
    );

    drop(adapter);
}

/// 批量安装任一失败时不得留下半状态：先合法后非法、先非法后合法两个顺序都必须
/// 净结果为零行（pre-validate 或回滚，两者皆可；顺序执行留下合法行 = mutant）。
/// 杀死 'partial failure leaves half state'。
#[test]
fn partial_plan_failure_leaves_no_half_state() {
    if !require_admin("partial_plan_failure_leaves_no_half_state") {
        return;
    }
    let lib = load_frozen();
    let adapter = create_scratch_adapter(&lib, "ExvW20Partial");
    configure_scratch_interface(adapter.alias())
        .expect("scratch 接口配置失败（无法安装路由）：netsh 前置失败");

    let valid = RouteRow::new(TUNNEL_NET, 24, PROBE_IP, adapter.luid(), 5);
    // 非法行：前缀 33（IPv4 上限 32）-> 87（facts §3 partial 冻结值）。
    let invalid = RouteRow::new(Ipv4Addr::new(10, 77, 77, 77), 33, PROBE_IP, adapter.luid(), 5);

    // 顺序 1：合法在前——顺序执行式实现会留下合法行（半状态）。
    let e1 = expect_native_error(
        install_plan(&[valid.clone(), invalid.clone()]),
        "install_plan [valid, invalid]",
    );
    assert_eq!(
        e1.code, ERROR_INVALID_PARAMETER,
        "非法前缀 33 必须 87（ERROR_INVALID_PARAMETER）, got {}",
        e1.code
    );
    let rows1 = expect_ok(capture_rows(adapter.luid()), "capture_rows after [valid, invalid]");
    assert!(
        !has_cidr(&rows1, TUNNEL_NET, TUNNEL_PREFIX),
        "失败后合法行不得残留（no half state）：顺序 1 留下 {rows1:?}"
    );

    // 顺序 2：非法在前——净结果同样必须为零。
    let _e2 = expect_native_error(
        install_plan(&[invalid.clone(), valid.clone()]),
        "install_plan [invalid, valid]",
    );
    let rows2 = expect_ok(capture_rows(adapter.luid()), "capture_rows after [invalid, valid]");
    assert!(
        !has_cidr(&rows2, TUNNEL_NET, TUNNEL_PREFIX),
        "任一顺序失败后都不得留下半状态：顺序 2 留下 {rows2:?}"
    );

    drop(adapter);
}

