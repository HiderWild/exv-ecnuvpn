#include "platform/darwin/native_utun.hpp"

#include <cerrno>
#include <cstdint>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

struct MockUtun {
  int next_fd = 17;
  std::uint32_t control_id = 0xCAFE;
  std::string interface_name = "utun7";

  int socket_result = 17;
  int resolve_result = 0;
  int connect_result = 0;
  int ifname_result = 0;
  int mtu_result = 0;
  int current_errno = 0;

  int socket_calls = 0;
  int socket_domain = 0;
  int socket_type = 0;
  int socket_protocol = 0;
  std::vector<std::string> control_names;
  int connected_fd = -1;
  std::uint32_t connected_control_id = 0;
  std::uint32_t connected_unit = 0;
  int ifname_fd = -1;
  std::vector<std::pair<std::string, int>> mtu_calls;
  std::vector<int> closed_fds;
};

exv::platform::NativeUtunApi make_api(MockUtun &mock) {
  exv::platform::NativeUtunApi api;
  api.open_socket = [&mock](int domain, int type, int protocol) {
    ++mock.socket_calls;
    mock.socket_domain = domain;
    mock.socket_type = type;
    mock.socket_protocol = protocol;
    return mock.socket_result;
  };
  api.resolve_control_id = [&mock](int, const std::string &control_name,
                                   std::uint32_t &control_id) {
    mock.control_names.push_back(control_name);
    if (mock.resolve_result != 0)
      return mock.resolve_result;
    control_id = mock.control_id;
    return 0;
  };
  api.connect_utun = [&mock](int fd, std::uint32_t control_id,
                             std::uint32_t unit) {
    mock.connected_fd = fd;
    mock.connected_control_id = control_id;
    mock.connected_unit = unit;
    return mock.connect_result;
  };
  api.get_interface_name = [&mock](int fd, std::string &interface_name) {
    mock.ifname_fd = fd;
    if (mock.ifname_result != 0)
      return mock.ifname_result;
    interface_name = mock.interface_name;
    return 0;
  };
  api.set_mtu = [&mock](const std::string &interface_name, int mtu) {
    mock.mtu_calls.push_back({interface_name, mtu});
    return mock.mtu_result;
  };
  api.close_fd = [&mock](int fd) {
    mock.closed_fds.push_back(fd);
    return 0;
  };
  api.last_error = [&mock] { return mock.current_errno; };
  return api;
}

exv::platform::NativeUtunConfig config_with_mtu(int mtu) {
  exv::platform::NativeUtunConfig config;
  config.mtu = mtu;
  return config;
}

bool opens_control_socket_through_injected_boundary() {
  MockUtun mock;
  exv::platform::NativeUtun utun(make_api(mock), config_with_mtu(1380));

  auto result = utun.start();

  bool ok = true;
  ok = expect(result.ok(), "mocked utun lifecycle should start") && ok;
  ok = expect(mock.socket_calls == 1,
              "start should open one utun control socket") &&
       ok;
  ok = expect(mock.socket_domain == exv::platform::native_utun_pf_system(),
              "socket domain should model PF_SYSTEM") &&
       ok;
  ok = expect(mock.socket_type == exv::platform::native_utun_socket_type(),
              "socket type should model SOCK_DGRAM") &&
       ok;
  ok = expect(mock.socket_protocol ==
                  exv::platform::native_utun_sysproto_control(),
              "socket protocol should model SYSPROTO_CONTROL") &&
       ok;
  ok = expect(mock.control_names.size() == 1 &&
                  mock.control_names[0] ==
                      exv::platform::native_utun_control_name(),
              "control lookup should use UTUN_CONTROL_NAME") &&
       ok;
  ok = expect(mock.connected_fd == mock.next_fd,
              "connect should use the opened socket fd") &&
       ok;
  ok = expect(mock.connected_control_id == mock.control_id,
              "connect should use the resolved utun control id") &&
       ok;
  return ok;
}

bool reports_utun_interface_name_from_boundary() {
  MockUtun mock;
  mock.interface_name = "utun12";
  exv::platform::NativeUtun utun(make_api(mock), config_with_mtu(1400));

  auto result = utun.start();

  bool ok = true;
  ok = expect(result.ok(), "mocked utun lifecycle should start") && ok;
  ok = expect(mock.ifname_fd == mock.next_fd,
              "interface query should use the opened fd") &&
       ok;
  ok = expect(result.metadata.interface_name == "utun12",
              "start result should report the utun interface name") &&
       ok;
  ok = expect(utun.metadata().interface_name == "utun12",
              "running session metadata should keep the utun interface name") &&
       ok;
  return ok;
}

bool applies_mtu_through_native_api_boundary() {
  MockUtun mock;
  mock.interface_name = "utun3";
  exv::platform::NativeUtun utun(make_api(mock), config_with_mtu(1412));

  auto result = utun.start();

  bool ok = true;
  ok = expect(result.ok(), "mocked utun lifecycle should start") && ok;
  ok = expect(mock.mtu_calls.size() == 1,
              "start should apply MTU through the native API boundary") &&
       ok;
  ok = expect(mock.mtu_calls.size() == 1 &&
                  mock.mtu_calls[0] == std::make_pair(std::string("utun3"),
                                                      1412),
              "MTU boundary should receive the utun name and configured MTU") &&
       ok;
  return ok;
}

bool stop_closes_fd_once() {
  MockUtun mock;
  exv::platform::NativeUtun utun(make_api(mock), config_with_mtu(1290));

  auto result = utun.start();
  utun.stop();
  utun.stop();

  bool ok = true;
  ok = expect(result.ok(), "mocked utun lifecycle should start") && ok;
  ok = expect(mock.closed_fds.size() == 1 && mock.closed_fds[0] == mock.next_fd,
              "stop should close the active utun fd exactly once") &&
       ok;
  ok = expect(!utun.running(), "stop should clear running state") && ok;
  return ok;
}

bool permission_errors_map_to_stable_code() {
  MockUtun mock;
  mock.connect_result = -1;
  mock.current_errno = EACCES;
  exv::platform::NativeUtun utun(make_api(mock), config_with_mtu(1290));

  auto result = utun.start();

  bool ok = true;
  ok = expect(!result.ok(), "permission failure should fail start") && ok;
  ok = expect(result.error ==
                  exv::platform::NativeUtunError::utun_permission_denied,
              "permission failure should map to utun_permission_denied") &&
       ok;
  ok = expect(std::string(exv::platform::native_utun_error_code(
                  result.error)) == "utun_permission_denied",
              "stable error code text should report utun_permission_denied") &&
       ok;
  ok = expect(mock.closed_fds.size() == 1 && mock.closed_fds[0] == mock.next_fd,
              "failed start should close the partially-opened fd") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = opens_control_socket_through_injected_boundary() && ok;
  ok = reports_utun_interface_name_from_boundary() && ok;
  ok = applies_mtu_through_native_api_boundary() && ok;
  ok = stop_closes_fd_once() && ok;
  ok = permission_errors_map_to_stable_code() && ok;
  return ok ? 0 : 1;
}
