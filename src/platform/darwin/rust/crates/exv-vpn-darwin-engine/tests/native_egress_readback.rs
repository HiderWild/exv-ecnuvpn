//! 原生宿主验收：出口读取与系统 route 工具独立比较；管理员测试另用临时 utun
//! 和文档地址验证数据泵、地址清除与设备复用。不修改默认路由，不连接 VPN。

use std::ffi::CString;
use std::net::Ipv4Addr;
use std::process::Command;

use exv_vpn_darwin_engine::egress::physical_route::find_physical_egress;

#[test]
#[ignore = "需要 macOS 宿主已配置 IPv4 默认网关，显式运行只读验收"]
fn real_default_gateway_matches_native_route_query() {
    let query = Command::new("/sbin/route")
        .args(["-n", "get", "default"])
        .output()
        .expect("执行系统 route 查询");
    assert!(query.status.success(), "系统没有可查询的默认路由");
    let text = String::from_utf8(query.stdout).expect("route 输出 UTF-8");
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key).map(str::trim))
            .unwrap_or_else(|| panic!("route 缺少字段 {key}"))
    };
    let gateway: Ipv4Addr = field("gateway:").parse().expect("默认网关为 IPv4");
    let interface = field("interface:");
    let interface_c = CString::new(interface).expect("接口名不含 NUL");
    // SAFETY: CString 提供有效且以 NUL 结尾的接口名。
    let ifindex = unsafe { libc::if_nametoindex(interface_c.as_ptr()) };
    assert_ne!(ifindex, 0, "系统接口必须存在");

    let observed = find_physical_egress().expect("产品读取物理出口");
    assert_eq!(observed.gateway, gateway, "产品网关必须与原生查询一致");
    assert_eq!(observed.ifindex, ifindex, "不能选到其他物理接口");
    assert_eq!(observed.interface, interface);
    assert!(!observed.address.is_unspecified());
    println!(
        "原生网关读取一致：interface={} ifindex={} gateway={} address={}",
        observed.interface, observed.ifindex, observed.gateway, observed.address
    );
}

#[test]
#[ignore = "需要管理员权限：独立测试 utun 与文档地址路由，不触碰默认路由或现用 VPN"]
fn native_utun_pumps_deliver_packets_and_reuse_after_idle_stop() {
    use exv_vpn_darwin_engine::log_sink::LogSink;
    use exv_vpn_darwin_engine::packet::pump::{self, PumpSet};
    use exv_vpn_darwin_engine::platform::{interface, route, utun::Utun};
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // SAFETY: geteuid 无参数且只读取身份。
    assert_eq!(unsafe { libc::geteuid() }, 0, "需要管理员权限");
    let destination = Ipv4Addr::new(198, 51, 100, 254);
    assert_eq!(
        route::read_exact(destination, 32).expect("查询测试地址"),
        None,
        "测试目的地址已有路由时不覆盖"
    );
    struct NativeFixture {
        pumps: Option<PumpSet>,
        route: Option<route::AppliedRoute>,
        device: Option<Utun>,
    }
    impl Drop for NativeFixture {
        fn drop(&mut self) {
            if let Some(pumps) = self.pumps.take() {
                pumps.shutdown();
            }
            if let Some(route) = self.route.take() {
                let result = route::delete(&route);
                eprintln!("测试路由清理结果：{result:?}");
            }
            drop(self.device.take());
        }
    }
    let mut fixture = NativeFixture {
        pumps: None,
        route: None,
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
    // 提前保留精确清理候选，内核写成功但回执失败也会在 Drop 时删除。
    fixture.route = Some(row);
    route::add(&row).expect("添加仅指向测试 utun 的文档地址路由");

    for generation in 0..2 {
        let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_read_tx, read_rx) = tokio::sync::mpsc::unbounded_channel();
        fixture.pumps = Some(pump::start(
            fixture.device.as_ref().unwrap().fd(),
            write_tx,
            read_rx,
            &Arc::new(LogSink::new()),
            None,
        ));
        let socket = UdpSocket::bind((source, 0)).expect("绑定测试源地址");
        // SAFETY: 有效 socket 与有效的接口索引，IP_BOUND_IF 只影响本测试 socket。
        let bound = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                25,
                std::ptr::from_ref(&interface_index).cast(),
                std::mem::size_of_val(&interface_index) as libc::socklen_t,
            )
        };
        assert_eq!(bound, 0, "绑定测试 utun");
        let payload = format!("exv-native-retained-utun-{generation}");
        socket
            .send_to(payload.as_bytes(), (destination, 9))
            .expect("向测试 utun 发送数据包");
        let until = Instant::now() + Duration::from_secs(3);
        let delivered = loop {
            if let Ok(frame) = write_rx.try_recv() {
                if frame
                    .windows(payload.len())
                    .any(|bytes| bytes == payload.as_bytes())
                {
                    break true;
                }
            }
            if Instant::now() >= until {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            delivered,
            "原生 utun 的 poll 必须收到第 {generation} 次会话数据"
        );
        fixture.pumps.take().unwrap().shutdown();
        interface::clear_ipv4(fixture.device.as_ref().unwrap(), source)
            .expect("停止数据泵后清除旧隧道地址");
        let facts = Command::new("/sbin/ifconfig")
            .arg(interface_name.to_str().unwrap())
            .output()
            .expect("回读保留设备地址");
        assert!(facts.status.success());
        assert!(
            !String::from_utf8_lossy(&facts.stdout).contains("inet 192.0.2.1 "),
            "保留设备不得保留旧 IPv4 地址"
        );
        assert_eq!(
            unsafe { libc::if_nametoindex(interface_name.as_ptr()) },
            interface_index
        );
        println!("原生 utun 第 {generation} 次数据转发与停止通过，同一设备仍存在");
        if generation == 0 {
            interface::apply_ipv4(fixture.device.as_ref().unwrap(), source, 24, 1400)
                .expect("下一次会话重新配置同一设备");
            // 删除接口地址可能让内核一并删除依赖它的路由，按真实回读补建测试行。
            if route::read_exact(destination, 32).unwrap().is_none() {
                route::add(&row).expect("恢复测试设备路由");
            }
        }
    }
    drop(fixture);
    assert_eq!(
        route::read_exact(destination, 32).expect("回读测试路由"),
        None
    );
    assert_eq!(
        unsafe { libc::if_nametoindex(interface_name.as_ptr()) },
        0,
        "全部描述符释放后测试网卡消失"
    );
}
