#include "platform/darwin/native_route_config.hpp"

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

struct MockRouteApi {
  ecnuvpn::platform::NativeDarwinUpstreamRoute upstream_route;
  int set_mtu_error = 0;
  int upstream_route_error = 0;
  int add_route_error = 0;
  int delete_route_error = 0;
  std::string add_route_error_cidr;

  std::vector<std::string> mtu_interfaces;
  std::vector<int> mtu_values;
  std::vector<std::string> upstream_route_destinations;
  std::vector<ecnuvpn::platform::NativeDarwinRoute> added_routes;
  std::vector<ecnuvpn::platform::NativeDarwinRoute> deleted_routes;
};

ecnuvpn::platform::NativeDarwinRouteApi make_api(MockRouteApi &mock) {
  ecnuvpn::platform::NativeDarwinRouteApi api;
  api.set_interface_mtu = [&mock](const std::string &interface_name, int mtu) {
    mock.mtu_interfaces.push_back(interface_name);
    mock.mtu_values.push_back(mtu);
    return mock.set_mtu_error;
  };
  api.get_best_route =
      [&mock](const std::string &destination,
              ecnuvpn::platform::NativeDarwinUpstreamRoute &route) {
        mock.upstream_route_destinations.push_back(destination);
        if (mock.upstream_route_error != 0)
          return mock.upstream_route_error;
        route = mock.upstream_route;
        return 0;
      };
  api.add_route = [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
    mock.added_routes.push_back(route);
    if (!mock.add_route_error_cidr.empty() &&
        route.cidr == mock.add_route_error_cidr)
      return mock.add_route_error;
    return 0;
  };
  api.delete_route =
      [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
        mock.deleted_routes.push_back(route);
        return mock.delete_route_error;
      };
  return api;
}

ecnuvpn::platform::NativeDarwinRouteConfigOptions options() {
  ecnuvpn::platform::NativeDarwinRouteConfigOptions opts;
  opts.interface_name = "utun7";
  opts.interface_index = 77;
  opts.configured_mtu = 1290;
  return opts;
}

ecnuvpn::vpn_engine::TunnelMetadata metadata() {
  ecnuvpn::vpn_engine::TunnelMetadata meta;
  meta.interface_name = "utun7";
  meta.interface_index = 77;
  meta.internal_ip4_address = "10.255.0.10";
  meta.internal_ip4_netmask = "255.255.255.0";
  meta.mtu = 1400;
  return meta;
}

MockRouteApi mock_api() {
  MockRouteApi mock;
  mock.upstream_route.interface_name = "en0";
  mock.upstream_route.interface_index = 4;
  mock.upstream_route.gateway = "192.0.2.1";
  return mock;
}

bool preserves_upstream_route_before_split_routes() {
  MockRouteApi mock = mock_api();
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.server_bypass_ips = {"203.0.113.15"};
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};

  ecnuvpn::platform::NativeDarwinRouteConfig config(make_api(mock),
                                                    options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(result.ok(), "route config should succeed with mocked API") && ok;
  ok = expect(mock.mtu_interfaces.size() == 1 &&
                  mock.mtu_interfaces[0] == "utun7" &&
                  mock.mtu_values[0] == 1400,
              "MTU should be configured through the injectable boundary") &&
       ok;
  ok = expect(mock.upstream_route_destinations.size() == 1 &&
                  mock.upstream_route_destinations[0] == "203.0.113.15",
              "server bypass should query the pre-tunnel upstream route") &&
       ok;
  ok = expect(mock.added_routes.size() == 3,
              "one server bypass and two split routes should be added") &&
       ok;
  ok = expect(mock.added_routes.size() >= 1 &&
                  mock.added_routes[0].server_bypass &&
                  mock.added_routes[0].cidr == "203.0.113.15/32",
              "server bypass route should be added before split routes") &&
       ok;
  ok = expect(mock.added_routes.size() >= 1 &&
                  mock.added_routes[0].interface_name == "en0" &&
                  mock.added_routes[0].interface_index == 4 &&
                  mock.added_routes[0].gateway == "192.0.2.1",
              "server bypass route should preserve upstream interface and gateway") &&
       ok;
  ok = expect(mock.added_routes.size() >= 3 &&
                  !mock.added_routes[1].server_bypass &&
                  !mock.added_routes[2].server_bypass,
              "split routes should follow the server bypass") &&
       ok;
  ok = expect(mock.added_routes.size() >= 3 &&
                  mock.added_routes[1].interface_name == "utun7" &&
                  mock.added_routes[2].interface_name == "utun7" &&
                  mock.added_routes[1].interface_index == 77 &&
                  mock.added_routes[2].interface_index == 77 &&
                  mock.added_routes[1].gateway.empty() &&
                  mock.added_routes[2].gateway.empty(),
              "split routes should use the utun interface directly") &&
       ok;
  return ok;
}

bool server_bypass_route_carries_upstream_message_scope() {
  MockRouteApi mock = mock_api();
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.server_bypass_ips = {"203.0.113.15"};

  ecnuvpn::platform::NativeDarwinRouteConfig config(make_api(mock),
                                                    options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(result.ok(), "route config should succeed with server bypass") &&
       ok;
  ok = expect(mock.added_routes.size() == 1,
              "one server bypass route should be added") &&
       ok;
  ok = expect(mock.added_routes.size() == 1 &&
                  mock.added_routes[0].server_bypass &&
                  mock.added_routes[0].gateway == "192.0.2.1",
              "server bypass should be gateway-backed") &&
       ok;
  ok = expect(mock.added_routes.size() == 1 &&
                  mock.added_routes[0].message_interface_scoped &&
                  mock.added_routes[0].message_interface_index == 4,
              "server bypass route message should be scoped to upstream interface") &&
       ok;
  return ok;
}

bool split_route_carries_utun_message_scope() {
  MockRouteApi mock = mock_api();
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.routes = {"59.78.176.0/20"};

  ecnuvpn::platform::NativeDarwinRouteConfig config(make_api(mock),
                                                    options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(result.ok(), "route config should succeed with split route") &&
       ok;
  ok = expect(mock.added_routes.size() == 1,
              "one split route should be added") &&
       ok;
  ok = expect(mock.added_routes.size() == 1 &&
                  !mock.added_routes[0].server_bypass &&
                  mock.added_routes[0].gateway.empty(),
              "split route should be interface-backed") &&
       ok;
  ok = expect(mock.added_routes.size() == 1 &&
                  mock.added_routes[0].message_interface_scoped &&
                  mock.added_routes[0].message_interface_index == 77,
              "split route message should be scoped to utun interface") &&
       ok;
  return ok;
}

bool cleanup_is_idempotent() {
  MockRouteApi mock = mock_api();
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.routes = {"59.78.176.0/20"};

  ecnuvpn::platform::NativeDarwinRouteConfig config(make_api(mock),
                                                    options());
  auto configure_result = config.configure(meta);
  auto cleanup_first = config.cleanup();
  auto cleanup_second = config.cleanup();

  bool ok = true;
  ok = expect(configure_result.ok(), "route config should succeed") && ok;
  ok = expect(cleanup_first.ok() && cleanup_second.ok(),
              "cleanup should remain successful when called repeatedly") &&
       ok;
  ok = expect(mock.deleted_routes.size() == 1,
              "cleanup should delete each owned route once") &&
       ok;
  ok = expect(mock.deleted_routes.size() == 1 &&
                  mock.deleted_routes[0].cidr == "59.78.176.0/20",
              "cleanup should delete the owned split route") &&
       ok;
  return ok;
}

bool route_failures_include_target_and_stable_error_code() {
  MockRouteApi mock = mock_api();
  mock.add_route_error = 65;
  mock.add_route_error_cidr = "10.0.0.0/8";
  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};

  ecnuvpn::platform::NativeDarwinRouteConfig config(make_api(mock),
                                                    options());
  auto result = config.configure(meta);

  bool ok = true;
  ok = expect(!result.ok(), "route add failure should fail configure") && ok;
  ok = expect(result.error ==
                  ecnuvpn::platform::
                      NativeDarwinRouteConfigError::route_add_failed,
              "route add failure should map to the stable route_add_failed enum") &&
       ok;
  ok = expect(std::string(ecnuvpn::platform::
                              native_darwin_route_config_error_code(
                                  result.error)) == "route_add_failed",
              "route add failure should expose stable error code text") &&
       ok;
  ok = expect(result.target == "10.0.0.0/8",
              "route add failure should include the target CIDR") &&
       ok;
  ok = expect(result.system_error == 65,
              "route add failure should preserve the native error code") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = preserves_upstream_route_before_split_routes() && ok;
  ok = server_bypass_route_carries_upstream_message_scope() && ok;
  ok = split_route_carries_utun_message_scope() && ok;
  ok = cleanup_is_idempotent() && ok;
  ok = route_failures_include_target_and_stable_error_code() && ok;
  return ok ? 0 : 1;
}
