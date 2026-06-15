#include "platform/darwin/platform_network_ops_darwin.hpp"

#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

struct MockUtun {
  int fd = 42;
  int close_count = 0;
  int open_count = 0;
  int set_mtu_count = 0;
  std::string interface_name = "utun7";
};

ecnuvpn::platform::NativeUtunApi make_utun_api(MockUtun &mock) {
  ecnuvpn::platform::NativeUtunApi api;
  api.open_socket = [&mock](int, int, int) {
    ++mock.open_count;
    return mock.fd;
  };
  api.resolve_control_id = [](int, const std::string &, std::uint32_t &id) {
    id = 99;
    return 0;
  };
  api.connect_utun = [](int, std::uint32_t, std::uint32_t) { return 0; };
  api.get_interface_name = [&mock](int, std::string &interface_name) {
    interface_name = mock.interface_name;
    return 0;
  };
  api.set_mtu = [&mock](const std::string &, int) {
    ++mock.set_mtu_count;
    return 0;
  };
  api.close_fd = [&mock](int) {
    ++mock.close_count;
    return 0;
  };
  api.last_error = [] { return 0; };
  return api;
}

struct MockRouteApi {
  int add_route_error_after = -1;
  std::vector<ecnuvpn::platform::NativeDarwinRoute> added_routes;
  std::vector<ecnuvpn::platform::NativeDarwinRoute> deleted_routes;
  std::vector<int> mtu_values;
};

struct MockDnsApi {
  std::vector<std::string> applied_interfaces;
  std::vector<exv::platform::NativeDarwinDnsSettings> applied_settings;
  std::vector<std::string> restored_interfaces;
  std::vector<std::string> disabled_interfaces;
  int apply_error = 0;
  int restore_error = 0;
  int disable_error = 0;
};

ecnuvpn::platform::NativeDarwinRouteApi make_route_api(MockRouteApi &mock) {
  ecnuvpn::platform::NativeDarwinRouteApi api;
  api.set_interface_mtu = [&mock](const std::string &, int mtu) {
    mock.mtu_values.push_back(mtu);
    return 0;
  };
  api.get_best_route =
      [](const std::string &, ecnuvpn::platform::NativeDarwinUpstreamRoute &route) {
        route.interface_name = "en0";
        route.interface_index = 4;
        route.gateway = "192.0.2.1";
        return 0;
      };
  api.add_route = [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
    mock.added_routes.push_back(route);
    if (mock.add_route_error_after >= 0 &&
        static_cast<int>(mock.added_routes.size()) >
            mock.add_route_error_after) {
      return 65;
    }
    return 0;
  };
  api.delete_route =
      [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
        mock.deleted_routes.push_back(route);
        return 0;
      };
  api.interface_index_from_name = [](const std::string &) {
    return std::uint32_t{77};
  };
  return api;
}

exv::platform::NativeDarwinDnsApi make_dns_api(MockDnsApi &mock) {
  exv::platform::NativeDarwinDnsApi api;
  api.apply_dns =
      [&mock](const std::string &interface_name,
              const exv::platform::NativeDarwinDnsSettings &settings) {
        mock.applied_interfaces.push_back(interface_name);
        mock.applied_settings.push_back(settings);
        return mock.apply_error;
      };
  api.restore_dns = [&mock](const std::string &interface_name) {
    mock.restored_interfaces.push_back(interface_name);
    return mock.restore_error;
  };
  api.disable_interface = [&mock](const std::string &interface_name) {
    mock.disabled_interfaces.push_back(interface_name);
    return mock.disable_error;
  };
  return api;
}

bool darwin_platform_ops_apply_routes_and_cleanup_in_order() {
  bool ok = true;
  MockUtun utun;
  MockRouteApi routes;
  auto ops = exv::platform::create_darwin_platform_network_ops(
      make_utun_api(utun), make_route_api(routes));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  ok = expect(device.is_open, "prepare should open a utun session") && ok;
  ok = expect(device.fd == 42, "prepare should return the utun fd") && ok;
  ok = expect(device.adapter_name == "utun7",
              "prepare should return the kernel utun interface name") &&
       ok;
  ok = expect(device.path == "utun://utun7",
              "prepare should return an opaque utun descriptor") &&
       ok;

  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.mtu = 1320;
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.routes.push_back({"59.78.176.0/20", "", 10, false});
  ok = expect(ops->apply_tunnel_config(device, config),
              "apply should configure Darwin routes") &&
       ok;
  ok = expect(routes.added_routes.size() == 2,
              "apply should add requested routes") &&
       ok;

  auto cleanup =
      ops->cleanup(device.adapter_name, exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success, "full cleanup should succeed") && ok;
  ok = expect(routes.deleted_routes.size() == 2,
              "cleanup should delete owned routes before closing utun") &&
       ok;
  ok = expect(utun.close_count == 1, "full cleanup should close utun") && ok;
  ok = expect(cleanup.routes_removed == 2,
              "cleanup should report removed route count") &&
       ok;
  ok = expect(cleanup.adapter_removed,
              "full cleanup should report adapter removal") &&
       ok;
  return ok;
}

bool darwin_platform_ops_rolls_back_routes_when_apply_fails() {
  bool ok = true;
  MockUtun utun;
  MockRouteApi routes;
  routes.add_route_error_after = 1;
  auto ops = exv::platform::create_darwin_platform_network_ops(
      make_utun_api(utun), make_route_api(routes));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.routes.push_back({"59.78.176.0/20", "", 10, false});

  ok = expect(!ops->apply_tunnel_config(device, config),
              "apply should fail when route creation fails") &&
       ok;
  ok = expect(routes.added_routes.size() == 2,
              "apply should attempt the failing route") &&
       ok;
  ok = expect(routes.deleted_routes.size() == 1 &&
                  routes.deleted_routes[0].cidr == "10.0.0.0/8",
              "apply failure should roll back routes created before failure") &&
       ok;
  return ok;
}

bool darwin_platform_ops_applies_dns_and_restores_on_cleanup() {
  bool ok = true;
  MockUtun utun;
  MockRouteApi routes;
  MockDnsApi dns;
  auto ops = exv::platform::create_darwin_platform_network_ops(
      make_utun_api(utun), make_route_api(routes), make_dns_api(dns));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53"};
  config.dns.search_domain = "vpn.example";
  config.dns.suffixes = {"ecnu.edu.cn"};

  ok = expect(ops->apply_tunnel_config(device, config),
              "apply should configure Darwin DNS through the native DNS API") &&
       ok;
  ok = expect(dns.applied_interfaces.size() == 1 &&
                  dns.applied_interfaces[0] == "utun7",
              "DNS apply should target the utun interface") &&
       ok;
  ok = expect(dns.applied_settings.size() == 1 &&
                  dns.applied_settings[0].servers.size() == 1 &&
                  dns.applied_settings[0].search_domain == "vpn.example" &&
                  dns.applied_settings[0].suffixes.size() == 1,
              "DNS apply should preserve servers, search domain, and suffixes") &&
       ok;

  auto resources = ops->managed_resources();
  bool saw_route = false;
  bool saw_dns = false;
  bool saw_adapter = false;
  for (const auto &resource : resources) {
    saw_adapter = saw_adapter || resource.type == "adapter";
    saw_route = saw_route || resource.type == "darwin_route";
    saw_dns = saw_dns || resource.type == "darwin_dns";
  }
  ok = expect(saw_adapter && saw_route && saw_dns,
              "managed_resources should expose adapter, route, and DNS facts") &&
       ok;

  auto cleanup =
      ops->cleanup(device.adapter_name, exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success, "full cleanup should succeed") && ok;
  ok = expect(cleanup.dns_removed, "cleanup should report DNS restoration") &&
       ok;
  ok = expect(dns.restored_interfaces.size() == 1 &&
                  dns.restored_interfaces[0] == "utun7",
              "cleanup should restore DNS on the utun interface") &&
       ok;
  return ok;
}

bool darwin_platform_ops_fresh_backend_cleans_imported_resource_facts() {
  bool ok = true;
  std::vector<exv::platform::ManagedNetworkResource> resources;
  {
    MockUtun utun;
    MockRouteApi routes;
    MockDnsApi dns;
    auto ops = exv::platform::create_darwin_platform_network_ops(
        make_utun_api(utun), make_route_api(routes), make_dns_api(dns));

    auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
    exv::platform::TunnelConfig config;
    config.interface_address = "10.255.0.10/24";
    config.routes.push_back({"10.0.0.0/8", "", 10, false});
    config.dns.servers = {"10.0.0.53"};

    ok = expect(ops->apply_tunnel_config(device, config),
                "initial backend should apply route and DNS") &&
         ok;
    resources = ops->managed_resources();
  }

  MockUtun fresh_utun;
  MockRouteApi fresh_routes;
  MockDnsApi fresh_dns;
  auto fresh_ops = exv::platform::create_darwin_platform_network_ops(
      make_utun_api(fresh_utun), make_route_api(fresh_routes),
      make_dns_api(fresh_dns));

  auto cleanup =
      fresh_ops->cleanup_resources(resources, exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success,
              "fresh backend should clean imported Darwin resource facts") &&
       ok;
  ok = expect(cleanup.routes_removed == 1 &&
                  fresh_routes.deleted_routes.size() == 1 &&
                  fresh_routes.deleted_routes[0].cidr == "10.0.0.0/8",
              "fresh backend should reconstruct and delete imported route") &&
       ok;
  ok = expect(cleanup.dns_removed &&
                  fresh_dns.restored_interfaces.size() == 1 &&
                  fresh_dns.restored_interfaces[0] == "utun7",
              "fresh backend should restore imported DNS fact") &&
       ok;
  ok = expect(cleanup.adapter_removed &&
                  fresh_dns.disabled_interfaces.size() == 1 &&
                  fresh_dns.disabled_interfaces[0] == "utun7",
              "fresh backend should disable imported adapter fact") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = darwin_platform_ops_apply_routes_and_cleanup_in_order() && ok;
  ok = darwin_platform_ops_rolls_back_routes_when_apply_fails() && ok;
  ok = darwin_platform_ops_applies_dns_and_restores_on_cleanup() && ok;
  ok = darwin_platform_ops_fresh_backend_cleans_imported_resource_facts() && ok;
  return ok ? 0 : 1;
}
