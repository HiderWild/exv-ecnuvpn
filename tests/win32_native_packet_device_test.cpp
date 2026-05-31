#include "platform/win32/native_packet_device.hpp"

#include <cstdint>
#include <cstddef>
#include <iostream>
#include <memory>
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

ecnuvpn::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  ecnuvpn::vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

ecnuvpn::vpn_engine::TunnelMetadata metadata() {
  ecnuvpn::vpn_engine::TunnelMetadata meta;
  meta.internal_ip4_address = "10.255.0.10";
  meta.internal_ip4_netmask = "255.255.255.0";
  meta.mtu = 1400;
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};
  meta.server_bypass_ips = {"203.0.113.15"};
  return meta;
}

struct MockState {
  ecnuvpn::platform::NativeWintunStartResult start_result;
  ecnuvpn::platform::NativeIpConfigResult configure_result;
  ecnuvpn::platform::NativeIpConfigResult cleanup_result;
  std::vector<ecnuvpn::platform::NativeIpConfigResult> cleanup_results;

  int wintun_starts = 0;
  int wintun_stops = 0;
  int ip_configures = 0;
  int ip_cleanups = 0;
  std::uint32_t ip_config_factory_interface_index = 0;
  ecnuvpn::vpn_engine::TunnelMetadata configured_metadata;
  std::vector<std::vector<std::uint8_t>> incoming_packets;
  std::vector<std::vector<std::uint8_t>> written_packets;
  std::vector<std::string> events;
  bool running = false;
};

class FakeWintunSession final
    : public ecnuvpn::platform::NativePacketDeviceWintunSession {
public:
  explicit FakeWintunSession(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  ecnuvpn::platform::NativeWintunStartResult start() override {
    ++state_->wintun_starts;
    state_->events.push_back("wintun_start");
    if (state_->start_result.ok())
      state_->running = true;
    return state_->start_result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!state_->running)
      return invalid("packet_device_closed", "packet device is closed");
    if (state_->incoming_packets.empty())
      return invalid("packet_device_empty", "no packet is queued");

    *packet = state_->incoming_packets.front();
    state_->incoming_packets.erase(state_->incoming_packets.begin());
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    if (!state_->running)
      return invalid("packet_device_closed", "packet device is closed");
    state_->written_packets.push_back(packet);
    return {};
  }

  void stop() override {
    if (!state_->running)
      return;
    ++state_->wintun_stops;
    state_->events.push_back("wintun_stop");
    state_->running = false;
  }

private:
  std::shared_ptr<MockState> state_;
};

class FakeIpConfig final
    : public ecnuvpn::platform::NativePacketDeviceIpConfig {
public:
  explicit FakeIpConfig(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  ecnuvpn::platform::NativeIpConfigResult
  configure(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    ++state_->ip_configures;
    state_->events.push_back("ip_configure");
    state_->configured_metadata = metadata;
    return state_->configure_result;
  }

  ecnuvpn::platform::NativeIpConfigResult cleanup() override {
    std::size_t cleanup_index =
        static_cast<std::size_t>(state_->ip_cleanups);
    ++state_->ip_cleanups;
    state_->events.push_back("ip_cleanup");
    if (cleanup_index < state_->cleanup_results.size())
      return state_->cleanup_results[cleanup_index];
    return state_->cleanup_result;
  }

private:
  std::shared_ptr<MockState> state_;
};

ecnuvpn::platform::NativePacketDeviceDependencies
dependencies(std::shared_ptr<MockState> state) {
  ecnuvpn::platform::NativePacketDeviceDependencies deps;
  deps.create_wintun_session = [state] {
    return std::unique_ptr<ecnuvpn::platform::NativePacketDeviceWintunSession>(
        new FakeWintunSession(state));
  };
  deps.create_ip_config = [state](std::uint32_t interface_index) {
    state->ip_config_factory_interface_index = interface_index;
    return std::unique_ptr<ecnuvpn::platform::NativePacketDeviceIpConfig>(
        new FakeIpConfig(state));
  };
  return deps;
}

std::shared_ptr<MockState> make_state() {
  auto state = std::make_shared<MockState>();
  state->start_result.metadata.adapter_name = L"ECNUVPN-Test-Wintun";
  state->start_result.metadata.if_index = 77;
  state->start_result.metadata.luid = 0x1234;
  return state;
}

bool open_starts_wintun_and_configures_native_ip() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(result.ok, "packet device open should succeed") && ok;
  ok = expect(state->wintun_starts == 1,
              "open should start the Wintun session") &&
       ok;
  ok = expect(state->ip_configures == 1,
              "open should configure native IP state") &&
       ok;
  ok = expect(state->ip_config_factory_interface_index == 77,
              "IP config factory should receive the Wintun interface index") &&
       ok;
  ok = expect(state->configured_metadata.interface_index == 77,
              "configured metadata should use the Wintun interface index") &&
       ok;
  ok = expect(state->configured_metadata.routes.size() == 2 &&
                  state->configured_metadata.server_bypass_ips.size() == 1,
              "configured metadata should preserve routes and bypass routes") &&
       ok;
  return ok;
}

bool packet_io_delegates_to_wintun_session_after_open() {
  auto state = make_state();
  state->incoming_packets.push_back({0x45, 0x00, 0x00, 0x2a});
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  std::vector<std::uint8_t> packet;
  auto read = device.read_packet(&packet);
  auto written = device.write_packet({0x45, 0x01, 0x02});

  bool ok = true;
  ok = expect(opened.ok && read.ok && written.ok,
              "open/read/write should succeed with fake Wintun") &&
       ok;
  ok = expect(packet == std::vector<std::uint8_t>({0x45, 0x00, 0x00, 0x2a}),
              "read_packet should return the Wintun packet bytes") &&
       ok;
  ok = expect(state->written_packets.size() == 1 &&
                  state->written_packets[0] ==
                      std::vector<std::uint8_t>({0x45, 0x01, 0x02}),
              "write_packet should send packet bytes through Wintun") &&
       ok;
  return ok;
}

bool closed_device_rejects_packet_io() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));
  std::vector<std::uint8_t> packet;

  auto read_before_open = device.read_packet(&packet);
  auto write_before_open = device.write_packet({0x45});
  auto opened = device.open(metadata());
  device.close();
  auto read_after_close = device.read_packet(&packet);

  bool ok = true;
  ok = expect(!read_before_open.ok &&
                  read_before_open.code == "packet_device_closed",
              "read before open should fail as closed") &&
       ok;
  ok = expect(!write_before_open.ok &&
                  write_before_open.code == "packet_device_closed",
              "write before open should fail as closed") &&
       ok;
  ok = expect(opened.ok, "open should succeed before close") && ok;
  ok = expect(!read_after_close.ok &&
                  read_after_close.code == "packet_device_closed",
              "read after close should fail as closed") &&
       ok;
  return ok;
}

bool ip_config_failure_cleans_up_partial_open() {
  auto state = make_state();
  state->configure_result.error =
      ecnuvpn::platform::NativeIpConfigError::route_create_failed;
  state->configure_result.message = "route failed";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(!result.ok, "IP config failure should fail open") && ok;
  ok = expect(result.code == "native_ip_config_route_create_failed",
              "IP config failure should map to a stable packet device code") &&
       ok;
  ok = expect(state->ip_cleanups == 1,
              "failed open should ask IP config to cleanup partial routes") &&
       ok;
  ok = expect(state->wintun_stops == 1,
              "failed open should stop the Wintun session") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"wintun_start", "ip_configure",
                                            "ip_cleanup", "wintun_stop"}),
              "failed open cleanup should remove routes before stopping Wintun") &&
       ok;
  return ok;
}

bool close_cleans_routes_before_wintun_and_is_idempotent() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));
  auto opened = device.open(metadata());
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before close") && ok;
  ok = expect(state->ip_cleanups == 1,
              "close should cleanup native routes exactly once") &&
       ok;
  ok = expect(state->wintun_stops == 1,
              "close should stop Wintun exactly once") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"wintun_start", "ip_configure",
                                            "ip_cleanup", "wintun_stop"}),
              "close should cleanup routes before stopping Wintun") &&
       ok;
  return ok;
}

bool close_cleanup_failure_keeps_cleanup_retry_available() {
  auto state = make_state();
  state->cleanup_result.error =
      ecnuvpn::platform::NativeIpConfigError::route_delete_failed;
  state->cleanup_result.message = "delete failed";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before cleanup retry test") && ok;
  ok = expect(state->ip_cleanups == 2,
              "failed close cleanup should keep IP config for a later retry") &&
       ok;
  ok = expect(state->wintun_stops == 1,
              "Wintun should still stop after route cleanup fails") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"wintun_start", "ip_configure",
                                            "ip_cleanup", "wintun_stop",
                                            "ip_cleanup"}),
              "close should retry route cleanup after stopping Wintun once") &&
       ok;
  return ok;
}

bool successful_close_retry_clears_cleanup_state() {
  auto state = make_state();
  ecnuvpn::platform::NativeIpConfigResult cleanup_failure;
  cleanup_failure.error =
      ecnuvpn::platform::NativeIpConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  state->cleanup_results = {cleanup_failure, {}};
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  device.close();
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before cleanup retry") && ok;
  ok = expect(state->ip_cleanups == 2,
              "successful retry should clear IP config state") &&
       ok;
  ok = expect(state->wintun_stops == 1,
              "Wintun should be stopped once even when cleanup needs retry") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"wintun_start", "ip_configure",
                                            "ip_cleanup", "wintun_stop",
                                            "ip_cleanup"}),
              "successful retry should leave no route cleanup work") &&
       ok;
  return ok;
}

bool open_rollback_cleanup_failure_is_reported_and_retryable() {
  auto state = make_state();
  state->configure_result.error =
      ecnuvpn::platform::NativeIpConfigError::route_create_failed;
  state->configure_result.message = "route failed";
  ecnuvpn::platform::NativeIpConfigResult cleanup_failure;
  cleanup_failure.error =
      ecnuvpn::platform::NativeIpConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  cleanup_failure.target = "59.78.176.0/20";
  state->cleanup_results = {cleanup_failure, {}};
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());
  device.close();

  bool ok = true;
  ok = expect(!result.ok, "IP config failure should fail open") && ok;
  ok = expect(result.code == "native_ip_config_route_create_failed",
              "open should keep the original IP config failure code") &&
       ok;
  ok = expect(result.message.find("native_ip_config_route_delete_failed") !=
                  std::string::npos,
              "open rollback cleanup failure should be reported stably") &&
       ok;
  ok = expect(state->ip_cleanups == 2,
              "open rollback cleanup failure should remain retryable") &&
       ok;
  ok = expect(state->wintun_stops == 1,
              "open rollback should stop Wintun even when route cleanup fails") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"wintun_start", "ip_configure",
                                            "ip_cleanup", "wintun_stop",
                                            "ip_cleanup"}),
              "open rollback should cleanup before Wintun stop and retry later") &&
       ok;
  return ok;
}

bool wintun_start_failure_skips_ip_config() {
  auto state = make_state();
  state->start_result.error =
      ecnuvpn::platform::NativeWintunError::session_start_failed;
  state->start_result.detail = "session failed";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(!result.ok, "Wintun start failure should fail open") && ok;
  ok = expect(result.code == "native_wintun_session_start_failed",
              "Wintun failure should map to a stable packet device code") &&
       ok;
  ok = expect(state->ip_configures == 0,
              "IP config should not run when Wintun start fails") &&
       ok;
  ok = expect(state->wintun_stops == 0,
              "device should not stop a Wintun session that never started") &&
       ok;
  return ok;
}

} // namespace

namespace ecnuvpn {
namespace utils {

std::string get_bundled_wintun_path() { return ""; }

} // namespace utils
} // namespace ecnuvpn

int main() {
  bool ok = true;
  ok = open_starts_wintun_and_configures_native_ip() && ok;
  ok = packet_io_delegates_to_wintun_session_after_open() && ok;
  ok = closed_device_rejects_packet_io() && ok;
  ok = ip_config_failure_cleans_up_partial_open() && ok;
  ok = close_cleans_routes_before_wintun_and_is_idempotent() && ok;
  ok = close_cleanup_failure_keeps_cleanup_retry_available() && ok;
  ok = successful_close_retry_clears_cleanup_state() && ok;
  ok = open_rollback_cleanup_failure_is_reported_and_retryable() && ok;
  ok = wintun_start_failure_skips_ip_config() && ok;
  return ok ? 0 : 1;
}
