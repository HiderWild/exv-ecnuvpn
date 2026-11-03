//! Cisco 分域 DNS 接线的局部测试；平台面为内存替身，不操作宿主。

use super::*;
use crate::campus_resolver::{InstalledResolver, ResolverError};
use crate::platform::{PlatformError, surface::PlatformSurface};

#[derive(Default)]
struct ResolverSurface {
    requests: Mutex<Vec<(Vec<String>, Vec<String>)>>,
    removed: Mutex<Vec<String>>,
    proxy_requests: Mutex<Vec<(Vec<String>, Vec<String>)>>,
    install_failure: AtomicBool,
    remove_failure: AtomicBool,
}

impl PlatformSurface for ResolverSurface {
    fn apply_proxy_policy(
        &self,
        routes: &[String],
        domains: &[String],
    ) -> (
        Option<crate::system_proxy::Owner>,
        crate::system_proxy::Result<crate::system_proxy::ApplyStatus>,
    ) {
        self.proxy_requests
            .lock()
            .unwrap()
            .push((routes.to_vec(), domains.to_vec()));
        (None, Ok(crate::system_proxy::ApplyStatus::Unchanged))
    }
    fn acquire_device(&self) -> Result<Utun, PlatformError> {
        panic!("no host device")
    }
    fn apply_ipv4(&self, _: &Utun, _: Ipv4Addr, _: u8, _: u16) -> Result<(), PlatformError> {
        panic!("no host address")
    }
    fn clear_ipv4(&self, _: &Utun, _: Ipv4Addr) -> Result<(), PlatformError> {
        panic!("no host address")
    }
    fn route_add(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        panic!("no host route")
    }
    fn route_change(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        panic!("no host route")
    }
    fn route_delete(&self, _: &AppliedRoute) -> Result<(), PlatformError> {
        panic!("no host route")
    }
    fn route_read_exact(&self, _: Ipv4Addr, _: u8) -> Result<Option<AppliedRoute>, PlatformError> {
        panic!("no host route")
    }
    fn install_resolvers(
        &self,
        domains: &[String],
        servers: &[String],
        logs: &LogSink,
    ) -> (Vec<InstalledResolver>, Option<ResolverError>) {
        self.requests
            .lock()
            .unwrap()
            .push((domains.to_vec(), servers.to_vec()));
        if servers.is_empty() {
            let directory = tempfile::tempdir().unwrap();
            return crate::campus_resolver::install_all(
                directory.path(),
                domains,
                servers,
                false,
                &|_, _| panic!("NO_SERVERS must not run host commands"),
                logs,
            );
        }

        let owned = domains
            .iter()
            .map(|domain| InstalledResolver {
                domain: domain.clone(),
            })
            .collect();
        let error = self
            .install_failure
            .load(Ordering::SeqCst)
            .then(|| ResolverError::Write {
                step: "WRITE",
                source: io::Error::other("fixture write failure"),
            });
        (owned, error)
    }
    fn remove_resolvers(&self, owned: &[InstalledResolver], _: &LogSink) -> usize {
        if self.remove_failure.load(Ordering::SeqCst) {
            return owned.len();
        }
        self.removed
            .lock()
            .unwrap()
            .extend(owned.iter().map(|entry| entry.domain.clone()));
        0
    }
}

#[test]
fn cisco_dns_uses_offer_servers_and_normal_cleanup_removes_owned_domains() {
    let runtime = EngineRuntime::new();
    let surface = Arc::new(ResolverSurface::default());
    runtime.install_surface_for_test(surface.clone());
    let domains = vec!["example.edu".into(), "www.example.edu".into()];
    let offer_dns = [Ipv4Addr::new(192, 0, 2, 53), Ipv4Addr::new(192, 0, 2, 54)];
    runtime
        .install_cisco_resolvers(&domains, &offer_dns)
        .unwrap();
    assert_eq!(
        *surface.requests.lock().unwrap(),
        [(
            domains.clone(),
            vec!["192.0.2.53".into(), "192.0.2.54".into()]
        )]
    );
    assert_eq!(runtime.resources.lock().unwrap().resolvers.len(), 2);
    assert!(runtime.cleanup_resolvers());
    assert_eq!(*surface.removed.lock().unwrap(), domains);
    assert!(!runtime.has_cleanup_obligations());
}

#[test]
fn cisco_dns_explicit_empty_does_not_install_even_without_servers() {
    let runtime = EngineRuntime::new();
    let surface = Arc::new(ResolverSurface::default());
    runtime.install_surface_for_test(surface.clone());
    runtime.install_cisco_resolvers(&[], &[]).unwrap();
    assert!(surface.requests.lock().unwrap().is_empty());
    assert!(!runtime.has_cleanup_obligations());
}

#[test]
fn cisco_dns_failed_install_keeps_pending_owner_until_cleanup_succeeds() {
    let runtime = EngineRuntime::new();
    let surface = Arc::new(ResolverSurface::default());
    runtime.install_surface_for_test(surface.clone());
    surface.install_failure.store(true, Ordering::SeqCst);
    surface.remove_failure.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .install_cisco_resolvers(&["example.edu".into()], &[Ipv4Addr::new(192, 0, 2, 53)])
            .is_err()
    );
    assert!(runtime.complete_resource_cleanup().is_err());
    assert!(runtime.has_cleanup_obligations());
    surface.remove_failure.store(false, Ordering::SeqCst);
    runtime.complete_resource_cleanup().unwrap();
    assert!(!runtime.has_cleanup_obligations());
    assert_eq!(*surface.removed.lock().unwrap(), ["example.edu"]);
}

#[test]
fn cisco_dns_adoption_and_resident_r1_materials_keep_the_admitted_domains() {
    let runtime = EngineRuntime::new();
    let domains = vec!["example.edu".into()];
    let held = PipelineIdentity {
        server: "vpn.example.edu".into(),
        username: "fixture".into(),
        protocol: ProtocolKind::Cisco,
        resolver_domains: domains.clone(),
    };
    let changed = PipelineIdentity {
        resolver_domains: vec!["other.example.edu".into()],
        ..held.clone()
    };
    assert_eq!(
        adoption_decision(&held, &changed, false),
        AdoptionDecision::Mismatch
    );
    runtime.store_reconnect_materials(ReconnectMaterials::Cisco {
        server: held.server,
        username: held.username,
        password: zeroize::Zeroizing::new("fixture-secret".into()),
        user_agent: "fixture-agent".into(),
        routes: vec![],
        resolver_domains: domains.clone(),
        mtu_override: None,
    });
    let Some(ReconnectMaterials::Cisco {
        mut resolver_domains,
        ..
    }) = runtime.reconnect_materials()
    else {
        panic!("Cisco materials");
    };
    resolver_domains.clear();
    let Some(ReconnectMaterials::Cisco {
        resolver_domains, ..
    }) = runtime.reconnect_materials()
    else {
        panic!("Cisco materials");
    };
    assert_eq!(resolver_domains, domains);
}

#[test]
fn cisco_dns_nonempty_domains_without_offer_dns_use_the_real_no_servers_error() {
    let runtime = EngineRuntime::new();
    let surface = Arc::new(ResolverSurface::default());
    runtime.install_surface_for_test(surface.clone());
    let error = runtime
        .install_cisco_resolvers(&["example.edu".into()], &[])
        .unwrap_err();
    assert!(matches!(
        error,
        ResolverError::Write {
            step: "NO_SERVERS",
            ..
        }
    ));
    assert!(!runtime.has_cleanup_obligations());
    assert!(surface.removed.lock().unwrap().is_empty());
}

#[test]
fn cisco_proxy_uses_admitted_domains_and_preserves_empty_domain_policy() {
    let runtime = EngineRuntime::new();
    let surface = Arc::new(ResolverSurface::default());
    runtime.install_surface_for_test(surface.clone());
    let admitted = PipelineIdentity {
        server: "vpn.example.edu".into(),
        username: "fixture".into(),
        protocol: ProtocolKind::Cisco,
        resolver_domains: vec!["example.edu".into(), "www.example.edu".into()],
    };
    let mut next = admitted.clone();
    next.resolver_domains = vec!["next.example.edu".into()];
    let routes = [AppliedRoute {
        destination: Ipv4Addr::new(192, 0, 2, 0),
        prefix: 24,
        gateway: Ipv4Addr::new(192, 0, 2, 1),
        ifindex: 22,
    }];
    runtime.apply_cisco_proxy_policy(&routes, &admitted);
    next.resolver_domains.clear();
    runtime.apply_cisco_proxy_policy(&routes, &next);
    assert_eq!(
        *surface.proxy_requests.lock().unwrap(),
        [
            (vec!["192.0.2.0/24".into()], admitted.resolver_domains),
            (vec!["192.0.2.0/24".into()], vec![]),
        ]
    );
}
