#include "platform/win32/native_packet_device.hpp"

#include <cstdint>
#include <cstddef>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <memory>
#include <sstream>
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

#ifndef EXV_SOURCE_DIR
#define EXV_SOURCE_DIR "."
#endif

std::string read_text_file(const std::filesystem::path &path) {
  std::ifstream input(path, std::ios::binary);
  std::ostringstream buffer;
  buffer << input.rdbuf();
  return buffer.str();
}

exv::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  exv::vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

exv::vpn_engine::TunnelMetadata metadata() {
  exv::vpn_engine::TunnelMetadata meta;
  meta.internal_ip4_address = "10.255.0.10";
  meta.internal_ip4_netmask = "255.255.255.0";
  meta.mtu = 1400;
  meta.routes = {"59.78.176.0/20", "10.0.0.0/8"};
  meta.server_bypass_ips = {"203.0.113.15"};
  return meta;
}

struct MockState {
  exv::platform::NativeWintunStartResult start_result;
  exv::platform::NativeIpConfigResult configure_result;
  exv::platform::NativeIpConfigResult cleanup_result;
  std::vector<exv::platform::NativeIpConfigResult> cleanup_results;

  int wintun_starts = 0;
  int wintun_stops = 0;
  int ip_configures = 0;
  int ip_cleanups = 0;
  std::uint32_t ip_config_factory_interface_index = 0;
  exv::vpn_engine::TunnelMetadata configured_metadata;
  exv::platform::NativeWintunConfig wintun_start_config;
  std::vector<std::vector<std::uint8_t>> incoming_packets;
  std::vector<std::vector<std::uint8_t>> written_packets;
  std::vector<std::string> events;
  bool running = false;
};

class FakeWintunSession final
    : public exv::platform::NativePacketDeviceWintunSession {
public:
  explicit FakeWintunSession(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  exv::platform::NativeWintunStartResult
  start(const exv::platform::NativeWintunConfig &config) override {
    ++state_->wintun_starts;
    state_->wintun_start_config = config;
    state_->events.push_back("wintun_start");
    if (state_->start_result.ok())
      state_->running = true;
    return state_->start_result;
  }

  exv::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!state_->running)
      return invalid("packet_device_closed", "packet device is closed");
    if (state_->incoming_packets.empty())
      return invalid("packet_device_empty", "no packet is queued");

    *packet = state_->incoming_packets.front();
    state_->incoming_packets.erase(state_->incoming_packets.begin());
    return {};
  }

  exv::vpn_engine::ValidationResult
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
    : public exv::platform::NativePacketDeviceIpConfig {
public:
  explicit FakeIpConfig(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  exv::platform::NativeIpConfigResult
  configure(const exv::vpn_engine::TunnelMetadata &metadata) override {
    ++state_->ip_configures;
    state_->events.push_back("ip_configure");
    state_->configured_metadata = metadata;
    return state_->configure_result;
  }

  exv::platform::NativeIpConfigResult cleanup() override {
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

exv::platform::NativePacketDeviceDependencies
dependencies(std::shared_ptr<MockState> state) {
  exv::platform::NativePacketDeviceDependencies deps;
  deps.create_wintun_session = [state] {
    return std::unique_ptr<exv::platform::NativePacketDeviceWintunSession>(
        new FakeWintunSession(state));
  };
  deps.create_ip_config = [state](std::uint32_t interface_index) {
    state->ip_config_factory_interface_index = interface_index;
    return std::unique_ptr<exv::platform::NativePacketDeviceIpConfig>(
        new FakeIpConfig(state));
  };
  return deps;
}

std::shared_ptr<MockState> make_state() {
  auto state = std::make_shared<MockState>();
  state->start_result.metadata.adapter_name = L"EXV-Test-Wintun";
  state->start_result.metadata.if_index = 77;
  state->start_result.metadata.luid = 0x1234;
  return state;
}

bool open_starts_wintun_and_configures_native_ip() {
  auto state = make_state();
  exv::platform::NativePacketDevice device(dependencies(state));

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

bool open_with_device_config_passes_interface_name_to_wintun() {
  auto state = make_state();
  exv::platform::NativePacketDevice device(dependencies(state));
  exv::vpn_engine::DeviceConfig config;
  config.interface_name = "EXV-Wintun";
  config.mtu = 1318;

  auto result = device.open(config);

  bool ok = true;
  ok = expect(result.ok, "packet device open with DeviceConfig should succeed") &&
       ok;
  ok = expect(state->wintun_starts == 1,
              "DeviceConfig open should start one Wintun session") &&
       ok;
  ok = expect(state->wintun_start_config.adapter_name_prefix ==
                  L"EXV-Wintun",
              "DeviceConfig interface_name should select the prepared adapter") &&
       ok;
  ok = expect(state->ip_configures == 0,
              "DeviceConfig open should not re-apply native IP config") &&
       ok;
  return ok;
}

bool device_config_open_requires_elevation_before_wintun_start() {
  auto state = make_state();
  auto deps = dependencies(state);
  deps.is_elevated = [] { return false; };
  exv::platform::NativePacketDevice device(std::move(deps));
  exv::vpn_engine::DeviceConfig config;
  config.interface_name = "EXV";

  auto result = device.open(config);

  bool ok = true;
  ok = expect(!result.ok,
              "DeviceConfig open should fail before Wintun when not elevated") &&
       ok;
  ok = expect(result.code == "native_wintun_adapter_open_failed",
              "non-elevated open should use the Wintun adapter failure code") &&
       ok;
  ok = expect(result.message.find("EXV-Wintun") != std::string::npos,
              "non-elevated open should report the target Wintun adapter") &&
       ok;
  ok = expect(result.message.find("Windows error 5") != std::string::npos,
              "non-elevated open should preserve the access-denied cause") &&
       ok;
  ok = expect(state->wintun_starts == 0,
              "non-elevated open should not call into the Wintun session") &&
       ok;
  return ok;
}

bool metadata_open_requires_elevation_before_wintun_start() {
  auto state = make_state();
  auto deps = dependencies(state);
  deps.is_elevated = [] { return false; };
  exv::platform::NativePacketDevice device(std::move(deps));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(!result.ok,
              "metadata open should fail before Wintun when not elevated") &&
       ok;
  ok = expect(result.code == "native_wintun_adapter_open_failed",
              "metadata non-elevated open should use Wintun failure code") &&
       ok;
  ok = expect(state->wintun_starts == 0,
              "metadata non-elevated open should not call Wintun") &&
       ok;
  ok = expect(state->ip_configures == 0,
              "metadata non-elevated open should not configure native IP") &&
       ok;
  return ok;
}

bool packet_io_delegates_to_wintun_session_after_open() {
  auto state = make_state();
  state->incoming_packets.push_back({0x45, 0x00, 0x00, 0x2a});
  exv::platform::NativePacketDevice device(dependencies(state));

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

bool wintun_empty_queue_maps_to_retryable_no_data() {
  const auto source_path = std::filesystem::path(EXV_SOURCE_DIR) /
                           "src" / "platform" / "win32" /
                           "native_packet_device.cpp";
  const std::string source = read_text_file(source_path);
  const auto branch = source.find("error == ERROR_NO_MORE_ITEMS");
  const auto retryable = source.find("return invalid(\"no_data\"", branch);
  const auto terminal =
      source.find("return invalid(\"packet_device_empty\"", branch);

  bool ok = true;
  ok = expect(branch != std::string::npos,
              "Wintun read should handle ERROR_NO_MORE_ITEMS explicitly") &&
       ok;
  ok = expect(retryable != std::string::npos,
              "empty Wintun queue should map to retryable no_data") &&
       ok;
  ok = expect(terminal == std::string::npos || retryable < terminal,
              "empty Wintun queue must not map to terminal packet_device_empty") &&
       ok;
  return ok;
}

bool closed_device_rejects_packet_io() {
  auto state = make_state();
  exv::platform::NativePacketDevice device(dependencies(state));
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
      exv::platform::NativeIpConfigError::route_create_failed;
  state->configure_result.message = "route failed";
  exv::platform::NativePacketDevice device(dependencies(state));

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
  exv::platform::NativePacketDevice device(dependencies(state));
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
      exv::platform::NativeIpConfigError::route_delete_failed;
  state->cleanup_result.message = "delete failed";
  exv::platform::NativePacketDevice device(dependencies(state));

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
  exv::platform::NativeIpConfigResult cleanup_failure;
  cleanup_failure.error =
      exv::platform::NativeIpConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  state->cleanup_results = {cleanup_failure, {}};
  exv::platform::NativePacketDevice device(dependencies(state));

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
      exv::platform::NativeIpConfigError::route_create_failed;
  state->configure_result.message = "route failed";
  exv::platform::NativeIpConfigResult cleanup_failure;
  cleanup_failure.error =
      exv::platform::NativeIpConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  cleanup_failure.target = "59.78.176.0/20";
  state->cleanup_results = {cleanup_failure, {}};
  exv::platform::NativePacketDevice device(dependencies(state));

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
      exv::platform::NativeWintunError::session_start_failed;
  state->start_result.detail = "session failed";
  exv::platform::NativePacketDevice device(dependencies(state));

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

namespace exv {
namespace platform {

std::string get_bundled_wintun_path() { return ""; }
bool check_root() { return true; }

} // namespace platform
} // namespace exv

int main() {
  bool ok = true;
  ok = open_starts_wintun_and_configures_native_ip() && ok;
  ok = open_with_device_config_passes_interface_name_to_wintun() && ok;
  ok = device_config_open_requires_elevation_before_wintun_start() && ok;
  ok = metadata_open_requires_elevation_before_wintun_start() && ok;
  ok = packet_io_delegates_to_wintun_session_after_open() && ok;
  ok = wintun_empty_queue_maps_to_retryable_no_data() && ok;
  ok = closed_device_rejects_packet_io() && ok;
  ok = ip_config_failure_cleans_up_partial_open() && ok;
  ok = close_cleans_routes_before_wintun_and_is_idempotent() && ok;
  ok = close_cleanup_failure_keeps_cleanup_retry_available() && ok;
  ok = successful_close_retry_clears_cleanup_state() && ok;
  ok = open_rollback_cleanup_failure_is_reported_and_retryable() && ok;
  ok = wintun_start_failure_skips_ip_config() && ok;
  return ok ? 0 : 1;
}
