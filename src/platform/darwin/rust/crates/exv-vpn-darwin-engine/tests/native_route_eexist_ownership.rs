//! D 原生复现（2026-09-20 吸收计划）：隧道路由归属缺口的事实采集。
//!
//! 背景：`bootstrap_runtime` 的隧道路由循环（offer/校园/DNS `/32`）直接
//! `platform_route::add` 且未处理 EEXIST 分支，与 `ensure_control_route` 的
//! Reused/Added/Changed 归属判定不对称；`route::add` 文档声称「RTM_ADD 对已存在
//! 条目返回 EEXIST……视为成功」，但实现不转换 EEXIST（delete 的 ESRCH 有转换）。
//! 本文件在隔离 utun + 文档测试地址（TEST-NET-2/3）上实测三个事实，为
//! 「先原生复现再定改法」提供宿主证据：
//!   1. 对已存在的同目的地路由再次 add 是否真的返回 EEXIST（当前引擎循环会把它
//!      判成 ROUTE 失败并断开连接——即「外部并发同目的地路由」场景的可达性）；
//!   2. `read_exact` 能否把 EEXIST 行回读为与期望一致的精确行（归属判定
//!      「幂等复用 vs 借用」的判据，`ensure_control_route` 同款）；
//!   3. utun 接口销毁（全部 FD 释放，模拟进程死亡）后引用该接口的路由是否被
//!      内核自动回收（「崩溃残留路由」场景在 darwin 的可达性；既有
//!      native_egress_readback 测试观察到删除接口地址会连带删除依赖路由）。
//!
//! 不触碰默认路由、不连接 VPN；失败断言本身即复现结论，运行后按输出判定改法。

use std::ffi::CString;
use std::net::Ipv4Addr;

use exv_vpn_darwin_engine::platform::{interface, route, utun::Utun};

#[test]
#[ignore = "需要管理员授权；隔离 utun 与文档地址路由的原生复现，必须串行执行"]
fn native_tunnel_route_eexist_and_reclaim_facts() {
    // SAFETY: geteuid 无参数且只读取身份。
    assert_eq!(unsafe { libc::geteuid() }, 0, "需要管理员权限");
    // 与 native_egress_readback 的 .254 错开，避免同轮串行运行互相干扰。
    let destination = Ipv4Addr::new(198, 51, 100, 203);
    assert_eq!(
        route::read_exact(destination, 32).expect("查询测试地址"),
        None,
        "测试目的地址已有路由时不覆盖"
    );

    struct Fixture {
        row: Option<route::AppliedRoute>,
        device: Option<Utun>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(row) = self.row.take() {
                // 接口销毁后内核大概率已回收路由；ESRCH 按 delete 语义视为已清。
                let result = route::delete(&row);
                eprintln!("复现测试路由清理结果：{result:?}");
            }
            drop(self.device.take());
        }
    }
    let mut fixture = Fixture {
        row: None,
        device: Some(Utun::acquire().expect("创建测试 utun")),
    };
    let source = Ipv4Addr::new(192, 0, 2, 1);
    let device = fixture.device.as_ref().unwrap();
    let interface_name = CString::new(device.name()).unwrap();
    let interface_index = device.ifindex();
    interface::apply_ipv4(device, source, 24, 1400).expect("设置测试 utun 地址");
    let row = route::AppliedRoute {
        destination,
        prefix: 32,
        gateway: source,
        ifindex: interface_index,
    };
    fixture.row = Some(row);

    // ---- 事实 1：同目的地路由已存在时，add 返回 EEXIST（不被视为成功）。----
    route::add(&row).expect("首次添加文档地址路由");
    let duplicate = route::add(&row).expect_err("重复添加必须报错而非成功");
    let errno = match &duplicate {
        exv_vpn_darwin_engine::platform::PlatformError::Route(_, error) => {
            error.raw_os_error().unwrap_or(0)
        }
        other => panic!("重复添加应返回 Route 错误，实际 {other:?}"),
    };
    assert_eq!(errno, libc::EEXIST, "RTM_ADD 对已存在行应回 EEXIST，实际 {errno}");
    println!("事实1：重复 add 返回 EEXIST（errno 17）——当前隧道路由循环未接此分支，外部既有同目的地路由会令连接以 ROUTE 失败。");

    // ---- 事实 2：read_exact 回读 EEXIST 行为精确行，归属判定有判据。----
    let observed = route::read_exact(destination, 32)
        .expect("回读测试路由")
        .expect("已添加路由必须可回读");
    assert_eq!(observed, row, "回读行与请求行一致时按 Reused 语义复用");
    println!("事实2：read_exact 回读与请求一致的精确行（{observed:?}）——照搬 ensure_control_route 的 EEXIST→read_exact 归属判定可行。");

    // ---- 事实 3：接口销毁（模拟进程死亡）后路由是否被内核回收。----
    drop(fixture.device.take());
    std::thread::sleep(std::time::Duration::from_millis(300));
    let ifindex_after = unsafe { libc::if_nametoindex(interface_name.as_ptr()) };
    assert_eq!(ifindex_after, 0, "全部 FD 释放后测试 utun 必须消失");
    let residue = route::read_exact(destination, 32).expect("接口销毁后查询路由");
    println!("事实3：接口销毁后 read_exact = {residue:?}");
    assert!(
        residue.is_none(),
        "内核未回收已销毁接口上的路由：崩溃残留场景在 darwin 真实可达，吸收修复必须处理残留路由的借用判定"
    );
    fixture.row = None;
}
