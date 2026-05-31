#include "platform/win32/native_ip_config.hpp"

#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

struct MockIpHelper {
  std::uint32_t best_route_interface_index = 7;
  std::string best_route_next_hop = "192.0.2.1";
  std::uint32_t create_address_error = 0;
  std::uint32_t set_mtu_error = 0;
  std::uint32_t best_route_error = 0;
  std::uint32_t create_route_error = 0;
  std::string create_route_error_cidr;

  std::vector<ecnuvpn::platform::NativeUnicastAddress> initialized_addresses;
  std::vector<ecnuvpn::platform::NativeUnicastAddress> created_addresses;
  std::vector<int> mtu_values;
  std::vector<std::string> best_route_destinations;
  std::vector<ecnuvpn::platform::NativeIpRoute> created_routes;
  std::vector<ecnuvpn::platform::NativeIpRoute> deleted_routes;
};

ecnuvpn::platform::NativeIpHelperApi make_api(MockIpHelper &mock) {
  ecnuvpn::platform::NativeIpHelperApi api;
  api.initialize_unicast_ip_address_entry =
      [&mock](ecnuvpn::platform::NativeUnicastAddress &address) {
        mock.initialized_addresses.push_back(address);
      };
  api.create_unicast_ip_address_entry =
      [&mock](const ecnuvpn::platform::NativeUnicastAddress &address) {
        mock.created_addresses.push_back(address);
        return mock.create_address_error;
      };
  api.set_interface_mtu = [&mock](std::uint32_t, int mtu) {
    mock.mtu_values.push_back(mtu);
    return mock.set_mtu_error;
  };
  api.get_best_route2 =
      [&mock](const std::string &destination,
              ecnuvpn::platform::NativeBestRoute &route) {
        mock.best_route_destinations.push_back(destination);
        if (mock.best_route_error != 0)
          return mock.best_route_error;
        route.interface_index = mock.best_route_interface_index;
        route.next_hop = mock.best_route_next_hop;
        return std::uint32_t{0};
      };
  api.create_ip_forward_entry2 =
      [&mock](const ecnuvpn::platform::NativeIpRoute &route) {
        mock.created_routes.push_back(route);
        if (!mock.create_route_error_cidr.empty() &&
            route.cidr == mock.create_route_error_cidr)
          return mock.create_route_error;
        return std::uint32_t{0};
      };
  api.delete_ip_forward_entry2 =
      [&mock](const ecnuvpn::platform::NativeIpRoute &route) {
        mock.deleted_routes.push_back(route);
        return std::uint32_t{0};
      };
  return api;
}

ecnuvpn::platform::NativeIpConfigOptions options() {
  ecnuvpn::platform::NativeIpConfigOptions opts;
  opts.interface_index = 42;
  opts.configured_mtu = 1290;
  return opts;
}

ecnuvpn::vpn_engine::TunnelMetadata metadata() {
  ecnuvpn::vpn_engine::TunnelMetadata meta;
  meta.interface_index = 42;
  meta.internal_ip4_address = "10.255.0.10";
  meta.internal_ip4_netmask = "255.255.255.0";
  meta.mtu = 1400;
  return meta;
}

bool installs_server_bypass_before_split_routes() {
  MockIpHelper mock;
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.server_bypass_ips = {"203.0.113.15"};
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};

  ecnuvpn::platform::NativeIpConfig config(make_api(mock), options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(result.ok(), "network config should succeed with mocked IP Helper") &&
       ok;
  ok = expect(mock.created_routes.size() == 3,
              "one bypass and two split routes should be installed") &&
       ok;
  ok = expect(mock.created_routes.size() >= 1 &&
                  mock.created_routes[0].server_bypass,
              "server bypass route should be installed before split routes") &&
       ok;
  ok = expect(mock.created_routes.size() >= 1 &&
                  mock.created_routes[0].cidr == "203.0.113.15/32",
              "bare bypass IP should normalize to /32") &&
       ok;
  ok = expect(mock.created_routes.size() >= 2 &&
                  !mock.created_routes[1].server_bypass,
              "split routes should follow bypass routes") &&
       ok;
  ok = expect(mock.created_routes.size() >= 3 &&
                  mock.created_routes[1].next_hop == "10.255.0.10" &&
                  mock.created_routes[2].interface_index == 42,
              "split routes should target the tunnel interface") &&
       ok;
  ok = expect(mock.best_route_destinations.size() == 1 &&
                  mock.best_route_destinations[0] == "203.0.113.15",
              "bypass route should preserve the pre-tunnel best route") &&
       ok;
  ok = expect(mock.created_routes.size() >= 1 &&
                  mock.created_routes[0].interface_index ==
                      mock.best_route_interface_index &&
                  mock.created_routes[0].next_hop == mock.best_route_next_hop,
              "bypass route should use GetBestRoute2 result") &&
       ok;
  return ok;
}

bool collapses_duplicate_routes() {
  MockIpHelper mock;
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.server_bypass_ips = {"203.0.113.15", "203.0.113.15/32"};
  meta.routes = {"59.78.176.0/20", "59.78.176.0/20", "10.0.0.0/8"};

  ecnuvpn::platform::NativeIpConfig config(make_api(mock), options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(result.ok(), "network config should succeed") && ok;
  ok = expect(mock.created_routes.size() == 3,
              "duplicate bypass and split routes should collapse") &&
       ok;
  ok = expect(mock.created_routes.size() >= 3 &&
                  mock.created_routes[0].cidr == "203.0.113.15/32" &&
                  mock.created_routes[1].cidr == "59.78.176.0/20" &&
                  mock.created_routes[2].cidr == "10.0.0.0/8",
              "route order should remain stable after duplicate collapse") &&
       ok;
  return ok;
}

bool cleanup_removes_only_routes_owned_by_this_session() {
  MockIpHelper mock;
  mock.create_route_error = 1234;
  mock.create_route_error_cidr = "10.0.0.0/8";

  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};

  ecnuvpn::platform::NativeIpConfig config(make_api(mock), options());
  auto result = config.configure(meta);
  auto cleanup_first = config.cleanup();
  auto cleanup_second = config.cleanup();

  bool ok = true;
  ok = expect(!result.ok(), "route creation failure should fail configure") &&
       ok;
  ok = expect(result.error ==
                  ecnuvpn::platform::NativeIpConfigError::route_create_failed,
              "route creation failure should map to route_create_failed") &&
       ok;
  ok = expect(cleanup_first.ok() && cleanup_second.ok(),
              "cleanup should be idempotent even after partial configure") &&
       ok;
  ok = expect(mock.deleted_routes.size() == 1,
              "cleanup should delete only successfully-created routes") &&
       ok;
  ok = expect(mock.deleted_routes.size() == 1 &&
                  mock.deleted_routes[0].cidr == "59.78.176.0/20",
              "cleanup should not delete a route that this session did not own") &&
       ok;
  return ok;
}

bool uses_tunnel_mtu_and_falls_back_to_configured_mtu() {
  MockIpHelper tunnel_mtu_mock;
  ecnuvpn::vpn_engine::TunnelMetadata tunnel_meta = metadata();
  tunnel_meta.mtu = 1400;
  ecnuvpn::platform::NativeIpConfig tunnel_mtu_config(
      make_api(tunnel_mtu_mock), options());
  auto tunnel_result = tunnel_mtu_config.configure(tunnel_meta);

  MockIpHelper fallback_mtu_mock;
  ecnuvpn::vpn_engine::TunnelMetadata fallback_meta = metadata();
  fallback_meta.mtu = 1100;
  ecnuvpn::platform::NativeIpConfigOptions fallback_options = options();
  fallback_options.configured_mtu = 1350;
  ecnuvpn::platform::NativeIpConfig fallback_mtu_config(
      make_api(fallback_mtu_mock), fallback_options);
  auto fallback_result = fallback_mtu_config.configure(fallback_meta);

  bool ok = true;
  ok = expect(tunnel_result.ok() && fallback_result.ok(),
              "MTU config scenarios should succeed") &&
       ok;
  ok = expect(tunnel_mtu_mock.mtu_values.size() == 1 &&
                  tunnel_mtu_mock.mtu_values[0] == 1400,
              "valid tunnel metadata MTU should be applied") &&
       ok;
  ok = expect(fallback_mtu_mock.mtu_values.size() == 1 &&
                  fallback_mtu_mock.mtu_values[0] == 1350,
              "low tunnel MTU should fall back to configured MTU") &&
       ok;
  return ok;
}

bool api_errors_map_to_stable_error_codes() {
  MockIpHelper mock;
  mock.create_address_error = 5;

  ecnuvpn::platform::NativeIpConfig config(make_api(mock), options());
  auto result = config.configure(metadata());

  bool ok = true;
  ok = expect(!result.ok(), "IP Helper failure should fail configure") && ok;
  ok = expect(result.error ==
                  ecnuvpn::platform::NativeIpConfigError::address_create_failed,
              "CreateUnicastIpAddressEntry failure should map to stable enum") &&
       ok;
  ok = expect(std::string(ecnuvpn::platform::native_ip_config_error_code(
                  result.error)) == "address_create_failed",
              "stable error code text should not expose raw Windows errors") &&
       ok;
  ok = expect(result.system_error == 5,
              "raw Windows error should remain available for diagnostics") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = installs_server_bypass_before_split_routes() && ok;
  ok = collapses_duplicate_routes() && ok;
  ok = cleanup_removes_only_routes_owned_by_this_session() && ok;
  ok = uses_tunnel_mtu_and_falls_back_to_configured_mtu() && ok;
  ok = api_errors_map_to_stable_error_codes() && ok;
  return ok ? 0 : 1;
}
