//! 资源操作使用原返回，不在成功后追加读回；只用现有 PlatformSurface 假实现。

use super::*;
use crate::platform::{PlatformError, surface::PlatformSurface};
use std::collections::VecDeque;
use std::io;

#[derive(Default)]
struct Routes {
    reads: Mutex<VecDeque<Option<AppliedRoute>>>,
    writes: Mutex<Vec<&'static str>>,
    add_error: Option<i32>,
    change_error: Option<i32>,
    delete_error: Option<i32>,
}

impl Routes {
    fn write(&self, action: &'static str, errno: Option<i32>) -> Result<(), PlatformError> {
        self.writes.lock().unwrap().push(action);
        errno.map_or(Ok(()), |errno| {
            Err(PlatformError::Route(
                action,
                io::Error::from_raw_os_error(errno),
            ))
        })
    }
}

impl PlatformSurface for Routes {
    fn acquire_device(&self) -> Result<Utun, PlatformError> {
        panic!("不创建宿主设备")
    }
    fn apply_ipv4(&self, _: &Utun, _: Ipv4Addr, _: u8, _: u16) -> Result<(), PlatformError> {
        panic!("不修改宿主接口")
    }
    fn clear_ipv4(&self, _: &Utun, _: Ipv4Addr) -> Result<(), PlatformError> {
        panic!("不修改宿主接口")
    }
    fn route_add(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        self.write("ACK", self.add_error)
    }
    fn route_change(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        self.write("CHANGE", self.change_error)
    }
    fn route_delete(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        self.write("DELETE", self.delete_error)
    }
    fn route_read_exact(&self, _: Ipv4Addr, _: u8) -> Result<Option<AppliedRoute>, PlatformError> {
        self.reads.lock().unwrap().pop_front().ok_or_else(|| {
            PlatformError::Route("EXTRA_READ", io::Error::from_raw_os_error(libc::EIO))
        })
    }
    fn install_resolvers(
        &self,
        _: &[String],
        _: &[String],
        _: &LogSink,
    ) -> (
        Vec<crate::campus_resolver::InstalledResolver>,
        Option<crate::campus_resolver::ResolverError>,
    ) {
        panic!("不修改宿主 DNS")
    }
    fn remove_resolvers(
        &self,
        _: &[crate::campus_resolver::InstalledResolver],
        _: &LogSink,
    ) -> usize {
        panic!("不修改宿主 DNS")
    }
}

fn desired() -> AppliedRoute {
    AppliedRoute {
        destination: Ipv4Addr::new(192, 0, 2, 7),
        prefix: 32,
        gateway: Ipv4Addr::new(192, 0, 2, 1),
        ifindex: 7,
    }
}

fn run(runtime: &EngineRuntime, bypass: bool) -> Result<ControlRouteUpdate, PlatformError> {
    let route = desired();
    let egress = crate::egress::physical_route::PhysicalEgress {
        gateway: route.gateway,
        ifindex: route.ifindex,
        interface: "en0",
        address: Ipv4Addr::new(192, 0, 2, 2),
    };
    if bypass {
        runtime
            .reconcile_bypass_routes(egress, &[route.destination])
            .map(|updates| updates[0])
    } else {
        runtime.ensure_control_route(egress, route.destination)
    }
}

fn owned(runtime: &EngineRuntime, bypass: bool) -> Vec<AppliedRoute> {
    let ledger = runtime.resources.lock().unwrap();
    if bypass {
        ledger.bypass_routes.clone()
    } else {
        ledger.control_route.into_iter().collect()
    }
}

#[test]
fn successful_route_operations_have_no_post_success_reads() {
    let wanted = desired();
    let other = AppliedRoute {
        gateway: Ipv4Addr::new(192, 0, 2, 3),
        ..wanted
    };
    for bypass in [false, true] {
        for (observations, add_error, expected, is_owned) in [
            (vec![None], None, ControlRouteUpdate::Added, true),
            (vec![Some(other)], None, ControlRouteUpdate::Changed, true),
            (vec![Some(wanted)], None, ControlRouteUpdate::Reused, false),
            (
                vec![None, Some(wanted)],
                Some(libc::EEXIST),
                ControlRouteUpdate::Reused,
                false,
            ),
            (
                vec![None, Some(other)],
                Some(libc::EEXIST),
                ControlRouteUpdate::Changed,
                true,
            ),
        ] {
            let surface = Arc::new(Routes {
                reads: Mutex::new(observations.into()),
                add_error,
                ..Routes::default()
            });
            let runtime = EngineRuntime::new();
            runtime.install_surface_for_test(surface.clone());
            assert_eq!(run(&runtime, bypass).unwrap(), expected);
            assert!(surface.reads.lock().unwrap().is_empty());
            assert_eq!(
                owned(&runtime, bypass),
                if is_owned { vec![wanted] } else { vec![] }
            );
        }
    }
}

#[test]
fn original_route_errors_and_failed_cleanup_ownership_survive() {
    let wanted = desired();
    let other = AppliedRoute {
        gateway: Ipv4Addr::new(192, 0, 2, 3),
        ..wanted
    };
    for bypass in [false, true] {
        for (observation, add_error, change_error, step) in [
            (None, Some(libc::EPERM), None, "ACK"),
            (Some(other), None, Some(libc::EPERM), "CHANGE"),
        ] {
            let surface = Arc::new(Routes {
                reads: Mutex::new([observation].into()),
                add_error,
                change_error,
                ..Routes::default()
            });
            let runtime = EngineRuntime::new();
            runtime.install_surface_for_test(surface);
            match run(&runtime, bypass).unwrap_err() {
                PlatformError::Route(actual_step, error) => {
                    assert_eq!(actual_step, step);
                    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
                }
                other => panic!("原路由错误被替换：{other:?}"),
            }
            assert!(owned(&runtime, bypass).is_empty());
        }
        let old = AppliedRoute {
            destination: Ipv4Addr::new(192, 0, 2, 99),
            ..wanted
        };
        let runtime = EngineRuntime::new();
        {
            let mut ledger = runtime.resources.lock().unwrap();
            if bypass {
                ledger.bypass_routes.push(old);
            } else {
                ledger.control_route = Some(old);
            }
        }
        let surface = Arc::new(Routes {
            delete_error: Some(libc::EBUSY),
            ..Routes::default()
        });
        runtime.install_surface_for_test(surface.clone());
        match run(&runtime, bypass).unwrap_err() {
            PlatformError::Route("DELETE", error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EBUSY))
            }
            other => panic!("原删除错误被替换：{other:?}"),
        }
        assert_eq!(owned(&runtime, bypass), vec![old]);
        assert_eq!(*surface.writes.lock().unwrap(), ["DELETE"]);
    }
}
