#include "platform/win32/platform_network_ops_win32.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace ecnuvpn::utils {
std::string get_bundled_wintun_path() { return ""; }
} // namespace ecnuvpn::utils

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
    return true;
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
  int fail_route_after = -1;
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
        if (mock.fail_route_after >= 0 &&
            static_cast<int>(mock.routes.size()) > mock.fail_route_after) {
          return std::uint32_t{87};
        }
        return std::uint32_t{0};
      };
  api.delete_ip_forward_entry2 =
      [&mock](const ecnuvpn::platform::NativeIpRoute &route) {
        mock.deleted_routes.push_back(route);
        return std::uint32_t{0};
      };
  return api;
}

bool win32_platform_ops_apply_routes_and_cleanup_in_order() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  ok = expect(device.is_open, "prepare should open a Wintun session") && ok;
  ok = expect(!device.path.empty(), "prepare should return an opaque device path") &&
       ok;
  ok = expect(wintun.sessions_started == 1,
              "prepare should start one Wintun session") &&
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
  ok = expect(wintun.sessions_ended == 1 && wintun.adapters_deleted == 1 &&
                  wintun.adapters_closed == 1,
              "cleanup should delete Wintun after route cleanup") &&
       ok;
  return ok;
}

bool win32_platform_ops_reject_dns_until_real_dns_backend_exists() {
  bool ok = true;
  MockWintun wintun;
  MockIpHelper ip;
  auto ops = exv::platform::create_win32_platform_network_ops(
      make_wintun_deps(wintun), make_ip_api(ip));

  auto device = ops->prepare_tunnel_device("ECNU-VPN", 1320);
  exv::platform::TunnelConfig config;
  config.interface_address = "10.255.0.10/24";
  config.dns.servers = {"10.0.0.53"};

  ok = expect(!ops->apply_tunnel_config(device, config),
              "apply must not report success for unimplemented DNS changes") &&
       ok;
  ok = expect(ip.addresses.empty() && ip.routes.empty(),
              "DNS rejection should happen before partial network mutation") &&
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
  ok = expect(ip.routes.size() == 2,
              "apply should have attempted the failing route") &&
       ok;
  ok = expect(ip.deleted_routes.size() == 1 &&
                  ip.deleted_routes[0].cidr == "10.0.0.0/8",
              "apply failure should roll back routes created before failure") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = win32_platform_ops_apply_routes_and_cleanup_in_order() && ok;
  ok = win32_platform_ops_reject_dns_until_real_dns_backend_exists() && ok;
  ok = win32_platform_ops_rolls_back_routes_when_apply_fails() && ok;
  return ok ? 0 : 1;
}
