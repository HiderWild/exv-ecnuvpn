#include "platform/win32/platform_network_ops_win32.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace ecnuvpn::platform {
std::string get_bundled_wintun_path() { return ""; }
} // namespace ecnuvpn::platform

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

struct MockWintun {
  std::wstring dll_path = L"C:\\bundle\\wintun.dll";
  std::uint64_t luid = 0x1234;
  std::uint32_t if_index = 42;
  int sessions_started = 0;
  int sessions_ended = 0;
  int adapters_deleted = 0;
  int adapters_closed = 0;
  int adapter = 0;
  int session = 0;
  bool delete_adapter_success = true;
  std::vector<std::string> *ops = nullptr;
};

ecnuvpn::platform::NativeWintunApi make_wintun_api(MockWintun &mock) {
  ecnuvpn::platform::NativeWintunApi api;
  api.open_adapter = [](const std::wstring &) {
    return ecnuvpn::platform::NativeWintunApi::AdapterHandle{};
  };
  api.create_adapter = [&mock](const std::wstring &, const std::wstring &) {
    return static_cast<ecnuvpn::platform::NativeWintunApi::AdapterHandle>(
        &mock.adapter);
  };
  api.close_adapter = [&mock](auto) { ++mock.adapters_closed; };
  api.get_adapter_luid = [&mock](auto, std::uint64_t &luid) {
    luid = mock.luid;
    return true;
  };
  api.get_interface_index = [&mock](std::uint64_t, std::uint32_t &if_index) {
    if_index = mock.if_index;
    return true;
  };
  api.start_session = [&mock](auto, std::uint32_t) {
    ++mock.sessions_started;
    return static_cast<ecnuvpn::platform::NativeWintunApi::SessionHandle>(
        &mock.session);
  };
  api.end_session = [&mock](auto) { ++mock.sessions_ended; };
  api.delete_adapter = [&mock](auto) {
    ++mock.adapters_deleted;
    if (mock.ops)
      mock.ops->push_back("adapter:delete");
    return mock.delete_adapter_success;
  };
  return api;
}

ecnuvpn::platform::NativeWintunDependencies make_wintun_deps(
    MockWintun &mock) {
  ecnuvpn::platform::NativeWintunDependencies deps;
  deps.path_provider = [&mock] { return mock.dll_path; };
  deps.file_exists = [](const std::wstring &) { return true; };
  deps.api_loader = [&mock](const std::wstring &,
                            ecnuvpn::platform::NativeWintunApi &api,
                            std::string &) {
    api = make_wintun_api(mock);
    return true;
  };
  return deps;
}

struct MockIpHelper {
  std::vector<ecnuvpn::platform::NativeUnicastAddress> addresses;
  std::vector<int> mtus;
  std::vector<ecnuvpn::platform::NativeIpRoute> routes;
  std::vector<ecnuvpn::platform::NativeIpRoute> deleted_routes;
  std::vector<std::uint32_t> dns_read_interfaces;
  std::vector<ecnuvpn::platform::NativeDnsSettings> dns_writes;
  ecnuvpn::platform::NativeDnsSettings current_dns;
  int fail_route_after = -1;
  std::uint32_t delete_route_error = 0;
  std::uint32_t get_dns_error = 0;
  std::uint32_t set_dns_error = 0;
  std::vector<std::string> *ops = nullptr;
};

ecnuvpn::platform::NativeIpHelperApi make_ip_api(MockIpHelper &mock) {
  ecnuvpn::platform::NativeIpHelperApi api;
  api.initialize_unicast_ip_address_entry =
      [](ecnuvpn::platform::NativeUnicastAddress &) {};
  api.create_unicast_ip_address_entry =
      [&mock](const ecnuvpn::platform::NativeUnicastAddress &address) {
        mock.addresses.push_back(address);
        return std::uint32_t{0};
      };
  api.set_interface_mtu = [&mock](std::uint32_t, int mtu) {
    mock.mtus.push_back(mtu);
    return std::uint32_t{0};
  };
  api.get_best_route2 =
      [](const std::string &, ecnuvpn::platform::NativeBestRoute &route) {
        route.interface_index = 7;
        route.next_hop = "192.0.2.1";
        return std::uint32_t{0};
      };
  api.create_ip_forward_entry2 =
      [&mock](const ecnuvpn::platform::NativeIpRoute &route) {
        mock.routes.push_back(route);
        if (mock.ops)
          mock.ops->push_back("route:create");
        if (mock.fail_route_after >= 0 &&
            static_cast<int>(mock.routes.size()) > mock.fail_route_after) {
          return std::uint32_t{87};
        }
        return std::uint32_t{0};
      };
  api.delete_ip_forward_entry2 =
      [&mock](const ecnuvpn::platform::NativeIpRoute &route) {
        mock.deleted_routes.push_back(route);
        if (mock.ops)
          mock.ops->push_back("route:delete");
        return mock.delete_route_error;
      };
  api.get_interface_dns_settings =
      [&mock](std::uint32_t interface_index,
              ecnuvpn::platform::NativeDnsSettings &settings) {
        mock.dns_read_interfaces.push_back(interface_index);
        if (mock.get_dns_error != 0)
          return mock.get_dns_error;
        settings = mock.current_dns;
        return std::uint32_t{0};
      };
  api.set_interface_dns_settings =
      [&mock](std::uint32_t,
              const ecnuvpn::platform::NativeDnsSettings &settings) {
        mock.dns_writes.push_back(settings);
        if (mock.ops)
          mock.ops->push_back("dns:set");
        return mock.set_dns_error;
      };
  return api;
}

std::size_t find_nth(const std::vector<std::string> &values,
                     const std::string &needle, int occurrence) {
  int seen = 0;
  for (std::size_t i = 0; i < values.size(); ++i) {
    if (values[i] == needle && ++seen == occurrence)
      return i;
  }
  return values.size();
}

bool win32_platform_ops_apply_routes_and_cleanup_in_order() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  ok = expect(device.is_open, "prepare should prepare a Wintun adapter") && ok;
  ok = expect(!device.path.empty(), "prepare should return an opaque device path") &&
       ok;
  ok = expect(wintun.sessions_started == 0,
              "prepare should not start a packet session owned by the core data plane") &&
       ok;

  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.mtu = 1320;
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.routes.push_back({"59.78.176.0/20", "", 10, false});
  const bool applied = ops->apply_tunnel_config(device, config);
  ok = expect(applied, "apply should configure address and routes") && ok;
  ok = expect(ip.addresses.size() == 1 &&
                  ip.addresses[0].address == "10.255.0.10" &&
                  ip.addresses[0].prefix_length == 24,
              "apply should derive address and prefix from CIDR") &&
       ok;
  ok = expect(ip.mtus.size() == 1 && ip.mtus[0] == 1320,
              "apply should configure MTU") &&
       ok;
  ok = expect(ip.routes.size() == 2 && ip.routes[0].cidr == "10.0.0.0/8" &&
                  ip.routes[1].cidr == "59.78.176.0/20",
              "apply should create requested routes") &&
       ok;

  auto cleanup = ops->cleanup(device.adapter_name,
                              exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success, "cleanup should succeed") && ok;
  ok = expect(cleanup.routes_removed == 2,
              "cleanup result should report removed route count") &&
       ok;
  ok = expect(ip.deleted_routes.size() == 2,
              "cleanup should remove owned routes") &&
       ok;
  ok = expect(wintun.sessions_started == 0 && wintun.sessions_ended == 0 &&
                  wintun.adapters_deleted == 1 && wintun.adapters_closed == 1,
              "cleanup should delete Wintun without starting a packet session") &&
       ok;
  return ok;
}

bool win32_platform_ops_apply_dns_and_restore_on_cleanup() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  std::vector<std::string> ops_log;
  wintun.ops = &ops_log;
  ip.ops = &ops_log;
  ip.current_dns.servers = {"192.0.2.53"};
  ip.current_dns.search_domain = "corp.example";
  ip.current_dns.suffixes = {"corp.example"};

  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53", "10.0.0.54"};
  config.dns.search_domain = "vpn.example";
  config.dns.suffixes = {"vpn.example", "ecnu.edu.cn"};

  ok = expect(ops->apply_tunnel_config(device, config),
              "apply should configure DNS through the native Windows DNS API") &&
       ok;
  ok = expect(ip.dns_read_interfaces.size() == 1 &&
                  ip.dns_read_interfaces[0] == 42,
              "apply should capture original DNS before changing it") &&
       ok;
  ok = expect(ip.dns_writes.size() == 1 &&
                  ip.dns_writes[0].servers.size() == 2 &&
                  ip.dns_writes[0].servers[0] == "10.0.0.53" &&
                  ip.dns_writes[0].search_domain == "vpn.example" &&
                  ip.dns_writes[0].suffixes.size() == 2,
              "apply should write requested DNS servers and suffixes") &&
       ok;

  auto cleanup = ops->cleanup(device.adapter_name,
                              exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success, "cleanup should restore DNS successfully") && ok;
  ok = expect(cleanup.dns_removed,
              "cleanup result should report DNS restoration") &&
       ok;
  ok = expect(ip.dns_writes.size() == 2 &&
                  ip.dns_writes[1].servers.size() == 1 &&
                  ip.dns_writes[1].servers[0] == "192.0.2.53" &&
                  ip.dns_writes[1].search_domain == "corp.example",
              "cleanup should restore the captured original DNS settings") &&
       ok;

  const std::size_t route_delete = find_nth(ops_log, "route:delete", 1);
  const std::size_t dns_restore = find_nth(ops_log, "dns:set", 2);
  const std::size_t adapter_delete = find_nth(ops_log, "adapter:delete", 1);
  ok = expect(route_delete < dns_restore && dns_restore < adapter_delete,
              "cleanup should remove routes, then DNS, then adapter") &&
       ok;
  return ok;
}

bool win32_platform_ops_reapply_refreshes_routes_without_overwriting_dns_origin() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.current_dns.servers = {"192.0.2.53"};
  ip.current_dns.search_domain = "corp.example";

  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53"};
  config.dns.search_domain = "vpn.example";

  ok = expect(ops->apply_tunnel_config(device, config),
              "first apply should configure route and DNS") &&
       ok;

  ip.current_dns.servers = config.dns.servers;
  ip.current_dns.search_domain = config.dns.search_domain;
  ok = expect(ops->apply_tunnel_config(device, config),
              "second apply should refresh route and DNS") &&
       ok;

  ok = expect(ip.dns_read_interfaces.size() == 1,
              "reapply should not overwrite the original DNS snapshot") &&
       ok;
  ok = expect(ip.addresses.size() == 1,
              "reapply should refresh routes without recreating the tunnel address") &&
       ok;
  ok = expect(ip.routes.size() == 2,
              "reapply should recreate the split route") &&
       ok;
  ok = expect(ip.deleted_routes.size() == 1 &&
                  ip.deleted_routes[0].cidr == "10.0.0.0/8",
              "reapply should remove the previous owned route before recreating it") &&
       ok;

  auto cleanup = ops->cleanup(device.adapter_name,
                              exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success, "cleanup after reapply should succeed") && ok;
  ok = expect(ip.dns_writes.size() == 3 &&
                  ip.dns_writes.back().servers ==
                      std::vector<std::string>{"192.0.2.53"} &&
                  ip.dns_writes.back().search_domain == "corp.example",
              "cleanup should restore the DNS snapshot captured before the first apply") &&
       ok;
  return ok;
}

bool win32_platform_ops_cleans_imported_resource_facts() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.current_dns.servers = {"192.0.2.53"};
  ip.current_dns.search_domain = "corp.example";

  std::vector<exv::platform::ManagedNetworkResource> resources;
  {
    auto ops = exv::platform::create_win32_platform_network_ops(
        make_wintun_deps(wintun), make_ip_api(ip));

    auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
    exv::platform::TunnelConfig config;
    config.interface_address = "10.255.0.10/24";
    config.routes.push_back({"10.0.0.0/8", "", 10, false});
    config.dns.servers = {"10.0.0.53"};

    ok = expect(ops->apply_tunnel_config(device, config),
                "apply should succeed before exporting cleanup facts") &&
         ok;
    resources = ops->managed_resources();
  }

  bool saw_adapter = false;
  bool saw_route = false;
  bool saw_dns = false;
  for (const auto &resource : resources) {
    saw_adapter = saw_adapter || resource.type == "adapter";
    saw_route = saw_route || resource.type == "win32_route";
    saw_dns = saw_dns || resource.type == "win32_dns_original";
  }
  ok = expect(saw_adapter && saw_route && saw_dns,
              "managed_resources should expose adapter, route, and original DNS facts") &&
       ok;

  auto fresh_ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));
  ip.delete_route_error = 1168;
  auto cleanup = fresh_ops->cleanup_resources(
      resources, exv::platform::CleanupPolicy::Full);

  ok = expect(cleanup.success,
              "fresh backend should cleanup using imported resource facts even when a route is already gone") &&
       ok;
  ok = expect(cleanup.routes_removed == 1,
              "fresh cleanup should delete the exported route fact") &&
       ok;
  ok = expect(cleanup.dns_removed,
              "fresh cleanup should restore the exported original DNS fact") &&
       ok;
  ok = expect(cleanup.adapter_removed,
              "fresh cleanup should delete the exported adapter fact") &&
       ok;
  ok = expect(!ip.deleted_routes.empty() &&
                  ip.deleted_routes.back().cidr == "10.0.0.0/8",
              "fresh cleanup should reconstruct the native route row") &&
       ok;
  ok = expect(ip.dns_writes.size() >= 2 &&
                  ip.dns_writes.back().servers.size() == 1 &&
                  ip.dns_writes.back().servers[0] == "192.0.2.53",
              "fresh cleanup should restore the original DNS settings") &&
       ok;
  ok = expect(wintun.adapters_deleted >= 1,
              "fresh cleanup should delete the Wintun adapter") &&
       ok;
  return ok;
}

bool win32_platform_ops_rolls_back_routes_when_apply_fails() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.fail_route_after = 1;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.routes.push_back({"59.78.176.0/20", "", 10, false});

  ok = expect(!ops->apply_tunnel_config(device, config),
              "apply should fail when route creation fails") &&
       ok;
  auto last_error = ops->last_error();
  ok = expect(last_error.code == "native_ip_config_route_create_failed",
              "apply failure should expose native IP config error code") &&
       ok;
  ok = expect(last_error.message.find("CreateIpForwardEntry2") != std::string::npos,
              "apply failure should expose native IP config error message") &&
       ok;
  ok = expect(last_error.target == "59.78.176.0/20",
              "apply failure should expose failing route target") &&
       ok;
  ok = expect(last_error.system_error == 87,
              "apply failure should expose Win32 system error") &&
       ok;
  ok = expect(ip.routes.size() == 2,
              "apply should have attempted the failing route") &&
       ok;
  ok = expect(ip.deleted_routes.size() == 1 &&
                  ip.deleted_routes[0].cidr == "10.0.0.0/8",
              "apply failure should roll back routes created before failure") &&
       ok;
  return ok;
}

bool win32_platform_ops_rolls_back_routes_when_dns_apply_fails() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.set_dns_error = 87;
  ip.current_dns.servers = {"192.0.2.53"};
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53"};

  ok = expect(!ops->apply_tunnel_config(device, config),
              "apply should fail when DNS application fails") &&
       ok;
  ok = expect(ip.routes.size() == 1 && ip.dns_writes.size() == 2,
              "apply should attempt the failing DNS write and restore original DNS") &&
       ok;
  ok = expect(ip.dns_writes.size() == 2 &&
                  ip.dns_writes[1].servers.size() == 1 &&
                  ip.dns_writes[1].servers[0] == "192.0.2.53",
              "DNS failure should restore captured original DNS settings") &&
       ok;
  ok = expect(ip.deleted_routes.size() == 1 &&
                  ip.deleted_routes[0].cidr == "10.0.0.0/8",
              "DNS failure should roll back routes created earlier") &&
       ok;
  return ok;
}

bool win32_platform_ops_restores_dns_when_route_cleanup_fails() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.current_dns.servers = {"192.0.2.53"};
  ip.delete_route_error = 1234;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53"};

  ok = expect(ops->apply_tunnel_config(device, config),
              "apply should succeed before cleanup failure scenario") &&
       ok;

  auto cleanup = ops->cleanup(device.adapter_name,
                              exv::platform::CleanupPolicy::Full);
  ok = expect(!cleanup.success,
              "cleanup should report failure when route deletion fails") &&
       ok;
  ok = expect(cleanup.dns_removed,
              "cleanup should still restore DNS after route deletion failure") &&
       ok;
  ok = expect(ip.dns_writes.size() == 2 &&
                  ip.dns_writes[1].servers.size() == 1 &&
                  ip.dns_writes[1].servers[0] == "192.0.2.53",
              "route cleanup failure should not leave VPN DNS active") &&
       ok;
  ok = expect(wintun.adapters_deleted == 0,
              "cleanup should not delete adapter after route cleanup failure") &&
       ok;
  return ok;
}

bool win32_platform_ops_treats_adapter_delete_failure_as_nonfatal_after_network_cleanup() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  ip.current_dns.servers = {"192.0.2.53"};
  wintun.delete_adapter_success = false;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.routes.push_back({"10.0.0.0/8", "", 10, false});
  config.dns.servers = {"10.0.0.53"};

  ok = expect(ops->apply_tunnel_config(device, config),
              "apply should succeed before adapter delete failure scenario") &&
       ok;

  auto cleanup = ops->cleanup(device.adapter_name,
                              exv::platform::CleanupPolicy::Full);
  ok = expect(cleanup.success,
              "adapter delete failure should not fail cleanup after routes and DNS are restored") &&
       ok;
  ok = expect(cleanup.routes_removed == 1,
              "cleanup should still remove owned routes") &&
       ok;
  ok = expect(cleanup.dns_removed,
              "cleanup should still restore DNS") &&
       ok;
  ok = expect(wintun.adapters_deleted == 1,
              "cleanup should attempt adapter deletion") &&
       ok;
  ok = expect(!cleanup.adapter_removed,
              "cleanup should not report adapter removed when deletion fails") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = win32_platform_ops_apply_routes_and_cleanup_in_order() && ok;
  ok = win32_platform_ops_apply_dns_and_restore_on_cleanup() && ok;
  ok = win32_platform_ops_reapply_refreshes_routes_without_overwriting_dns_origin() && ok;
  ok = win32_platform_ops_cleans_imported_resource_facts() && ok;
  ok = win32_platform_ops_rolls_back_routes_when_apply_fails() && ok;
  ok = win32_platform_ops_rolls_back_routes_when_dns_apply_fails() && ok;
  ok = win32_platform_ops_restores_dns_when_route_cleanup_fails() && ok;
  ok = win32_platform_ops_treats_adapter_delete_failure_as_nonfatal_after_network_cleanup() && ok;
  return ok ? 0 : 1;
}
