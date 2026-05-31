#include "platform/darwin/native_packet_device.hpp"

#include <algorithm>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstring>
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
  ecnuvpn::platform::NativeUtunStartResult start_result;
  ecnuvpn::platform::NativeDarwinRouteConfigResult configure_result;
  ecnuvpn::platform::NativeDarwinRouteConfigResult cleanup_result;
  std::vector<ecnuvpn::platform::NativeDarwinRouteConfigResult>
      cleanup_results;
  ecnuvpn::vpn_engine::ValidationResult read_result;
  ecnuvpn::vpn_engine::ValidationResult write_result;

  int utun_starts = 0;
  int utun_stops = 0;
  int route_configures = 0;
  int route_cleanups = 0;
  ecnuvpn::vpn_engine::TunnelMetadata utun_factory_metadata;
  ecnuvpn::platform::NativeUtunMetadata route_factory_metadata;
  ecnuvpn::vpn_engine::TunnelMetadata configured_metadata;
  std::vector<std::vector<std::uint8_t>> incoming_frames;
  std::vector<std::vector<std::uint8_t>> written_frames;
  std::vector<std::string> events;
  bool running = false;
};

class FakeUtunSession final
    : public ecnuvpn::platform::NativePacketDeviceUtunSession {
public:
  explicit FakeUtunSession(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  ecnuvpn::platform::NativeUtunStartResult start() override {
    ++state_->utun_starts;
    state_->events.push_back("utun_start");
    if (state_->start_result.ok())
      state_->running = true;
    return state_->start_result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_frame(std::vector<std::uint8_t> *frame) override {
    if (!state_->running)
      return invalid("packet_device_closed", "packet device is closed");
    if (!state_->read_result.ok)
      return state_->read_result;
    if (state_->incoming_frames.empty())
      return invalid("darwin_utun_read_failed", "failed to read utun frame");

    *frame = state_->incoming_frames.front();
    state_->incoming_frames.erase(state_->incoming_frames.begin());
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_frame(const std::vector<std::uint8_t> &frame) override {
    if (!state_->running)
      return invalid("packet_device_closed", "packet device is closed");
    if (!state_->write_result.ok)
      return state_->write_result;
    state_->written_frames.push_back(frame);
    return {};
  }

  void stop() override {
    if (!state_->running)
      return;
    ++state_->utun_stops;
    state_->events.push_back("utun_stop");
    state_->running = false;
  }

private:
  std::shared_ptr<MockState> state_;
};

class FakeRouteConfig final
    : public ecnuvpn::platform::NativePacketDeviceRouteConfig {
public:
  explicit FakeRouteConfig(std::shared_ptr<MockState> state)
      : state_(std::move(state)) {}

  ecnuvpn::platform::NativeDarwinRouteConfigResult
  configure(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    ++state_->route_configures;
    state_->events.push_back("route_configure");
    state_->configured_metadata = metadata;
    return state_->configure_result;
  }

  ecnuvpn::platform::NativeDarwinRouteConfigResult cleanup() override {
    const std::size_t cleanup_index =
        static_cast<std::size_t>(state_->route_cleanups);
    ++state_->route_cleanups;
    state_->events.push_back("route_cleanup");
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
  deps.create_utun_session =
      [state](const ecnuvpn::vpn_engine::TunnelMetadata &metadata) {
        state->utun_factory_metadata = metadata;
        return std::unique_ptr<
            ecnuvpn::platform::NativePacketDeviceUtunSession>(
            new FakeUtunSession(state));
      };
  deps.create_route_config =
      [state](const ecnuvpn::platform::NativeUtunMetadata &metadata) {
        state->route_factory_metadata = metadata;
        return std::unique_ptr<
            ecnuvpn::platform::NativePacketDeviceRouteConfig>(
            new FakeRouteConfig(state));
      };
  return deps;
}

std::shared_ptr<MockState> make_state() {
  auto state = std::make_shared<MockState>();
  state->start_result.metadata.fd = 42;
  state->start_result.metadata.interface_name = "utun9";
  state->start_result.metadata.mtu = 1400;
  return state;
}

struct TestNativeUtunState {
  int fd = 84;
  int last_error = 0;
  int close_calls = 0;
  int mtu = 0;
  std::string interface_name = "utun-test";
};

ecnuvpn::platform::NativeUtunApi
test_utun_api(std::shared_ptr<TestNativeUtunState> state) {
  ecnuvpn::platform::NativeUtunApi api;
  api.open_socket = [state](int, int, int) { return state->fd; };
  api.resolve_control_id =
      [](int, const std::string &, std::uint32_t &control_id) {
        control_id = 7;
        return 0;
      };
  api.connect_utun = [](int, std::uint32_t, std::uint32_t) { return 0; };
  api.get_interface_name = [state](int, std::string &interface_name) {
    interface_name = state->interface_name;
    return 0;
  };
  api.set_mtu = [state](const std::string &, int mtu) {
    state->mtu = mtu;
    return 0;
  };
  api.close_fd = [state](int fd) {
    if (fd == state->fd)
      ++state->close_calls;
    return 0;
  };
  api.last_error = [state] { return state->last_error; };
  return api;
}

struct TestPacketIoState {
  int last_error = 0;
  int set_nonblocking_calls = 0;
  int read_calls = 0;
  int write_calls = 0;
  int wait_readable_calls = 0;
  int wait_writable_calls = 0;
  std::vector<int> readable_results;
  std::vector<int> readable_errors;
  std::vector<int> writable_results;
  std::vector<int> writable_errors;
  std::vector<std::ptrdiff_t> read_results;
  std::vector<int> read_errors;
  std::vector<std::uint8_t> read_payload;
  std::vector<std::ptrdiff_t> write_results;
  std::vector<int> write_errors;
  std::vector<std::vector<std::uint8_t>> write_chunks;
};

int next_int_result(const std::vector<int> &values, int index,
                    int default_value) {
  const std::size_t offset = static_cast<std::size_t>(index);
  return offset < values.size() ? values[offset] : default_value;
}

std::ptrdiff_t next_ptrdiff_result(const std::vector<std::ptrdiff_t> &values,
                                   int index,
                                   std::ptrdiff_t default_value) {
  const std::size_t offset = static_cast<std::size_t>(index);
  return offset < values.size() ? values[offset] : default_value;
}

ecnuvpn::platform::NativeDarwinPacketIoApi
test_packet_io_api(std::shared_ptr<TestPacketIoState> state) {
  ecnuvpn::platform::NativeDarwinPacketIoApi api;
  api.set_nonblocking_fd = [state](int) {
    ++state->set_nonblocking_calls;
    return 0;
  };
  api.wait_readable_fd = [state](int, int) {
    const int index = state->wait_readable_calls++;
    const int result = next_int_result(state->readable_results, index, 1);
    if (result < 0)
      state->last_error = next_int_result(state->readable_errors, index, EIO);
    return result;
  };
  api.wait_writable_fd = [state](int, int) {
    const int index = state->wait_writable_calls++;
    const int result = next_int_result(state->writable_results, index, 1);
    if (result < 0)
      state->last_error = next_int_result(state->writable_errors, index, EIO);
    return result;
  };
  api.read_fd = [state](int, void *buffer, std::size_t size) {
    const int index = state->read_calls++;
    const std::ptrdiff_t result =
        next_ptrdiff_result(state->read_results, index,
                            static_cast<std::ptrdiff_t>(
                                state->read_payload.size()));
    if (result < 0) {
      state->last_error = next_int_result(state->read_errors, index, EIO);
      return result;
    }

    const std::size_t count = std::min<std::size_t>(
        static_cast<std::size_t>(result),
        std::min<std::size_t>(size, state->read_payload.size()));
    std::memcpy(buffer, state->read_payload.data(), count);
    return static_cast<std::ptrdiff_t>(count);
  };
  api.write_fd = [state](int, const void *buffer, std::size_t size) {
    const int index = state->write_calls++;
    const std::ptrdiff_t result =
        next_ptrdiff_result(state->write_results, index,
                            static_cast<std::ptrdiff_t>(size));
    if (result < 0) {
      state->last_error = next_int_result(state->write_errors, index, EIO);
      return result;
    }

    const std::size_t count =
        std::min<std::size_t>(static_cast<std::size_t>(result), size);
    const std::uint8_t *bytes =
        static_cast<const std::uint8_t *>(buffer);
    state->write_chunks.push_back(
        std::vector<std::uint8_t>(bytes, bytes + count));
    return static_cast<std::ptrdiff_t>(count);
  };
  api.last_error = [state] { return state->last_error; };
  return api;
}

bool open_starts_utun_and_configures_darwin_routes() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(result.ok, "packet device open should succeed") && ok;
  ok = expect(state->utun_starts == 1, "open should start utun") && ok;
  ok = expect(state->route_configures == 1,
              "open should configure Darwin routes") &&
       ok;
  ok = expect(state->utun_factory_metadata.mtu == 1400,
              "utun factory should receive the tunnel MTU") &&
       ok;
  ok = expect(state->route_factory_metadata.interface_name == "utun9" &&
                  state->route_factory_metadata.fd == 42,
              "route config factory should receive utun metadata") &&
       ok;
  ok = expect(state->configured_metadata.interface_name == "utun9",
              "route config should use the opened utun interface name") &&
       ok;
  ok = expect(state->configured_metadata.routes.size() == 2 &&
                  state->configured_metadata.server_bypass_ips.size() == 1,
              "route config should preserve routes and bypass routes") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure"}),
              "open should start utun before configuring routes") &&
       ok;
  return ok;
}

bool packet_io_strips_and_adds_utun_address_family_header() {
  auto state = make_state();
  state->incoming_frames.push_back(
      {0x00, 0x00, 0x00, 0x02, 0x45, 0x00, 0x00, 0x2a});
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  std::vector<std::uint8_t> packet;
  auto read = device.read_packet(&packet);
  auto write_v4 = device.write_packet({0x45, 0x01, 0x02});
  auto write_v6 = device.write_packet({0x60, 0x01, 0x02});

  bool ok = true;
  ok = expect(opened.ok && read.ok && write_v4.ok && write_v6.ok,
              "open/read/write should succeed with fake utun") &&
       ok;
  ok = expect(packet == std::vector<std::uint8_t>({0x45, 0x00, 0x00, 0x2a}),
              "read_packet should strip the four-byte utun header") &&
       ok;
  ok = expect(state->written_frames.size() == 2,
              "write_packet should write one utun frame per packet") &&
       ok;
  ok = expect(state->written_frames.size() == 2 &&
                  state->written_frames[0] ==
                      std::vector<std::uint8_t>({0x00, 0x00, 0x00, 0x02,
                                                 0x45, 0x01, 0x02}),
              "IPv4 writes should prepend the AF_INET utun header") &&
       ok;
  ok = expect(state->written_frames.size() == 2 &&
                  state->written_frames[1] ==
                      std::vector<std::uint8_t>({0x00, 0x00, 0x00, 0x1e,
                                                 0x60, 0x01, 0x02}),
              "IPv6 writes should prepend the AF_INET6 utun header") &&
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

bool invalid_utun_frames_return_stable_errors() {
  auto short_state = make_state();
  short_state->incoming_frames.push_back({0x00, 0x00, 0x00});
  ecnuvpn::platform::NativePacketDevice short_device(
      dependencies(short_state));
  auto short_opened = short_device.open(metadata());
  std::vector<std::uint8_t> packet;
  auto short_read = short_device.read_packet(&packet);

  auto family_state = make_state();
  family_state->incoming_frames.push_back(
      {0x00, 0x00, 0x00, 0x63, 0x45, 0x00});
  ecnuvpn::platform::NativePacketDevice family_device(
      dependencies(family_state));
  auto family_opened = family_device.open(metadata());
  auto family_read = family_device.read_packet(&packet);

  bool ok = true;
  ok = expect(short_opened.ok && family_opened.ok,
              "test devices should open before invalid frame reads") &&
       ok;
  ok = expect(!short_read.ok &&
                  short_read.code == "darwin_utun_invalid_frame",
              "short utun frame should return a stable error code") &&
       ok;
  ok = expect(!family_read.ok &&
                  family_read.code ==
                      "darwin_utun_unsupported_address_family",
              "unsupported utun family should return a stable error code") &&
       ok;
  return ok;
}

bool invalid_packets_return_stable_write_errors() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));
  auto opened = device.open(metadata());

  auto empty = device.write_packet({});
  auto unknown = device.write_packet({0x10, 0x00});

  bool ok = true;
  ok = expect(opened.ok, "device should open before invalid writes") && ok;
  ok = expect(!empty.ok && empty.code == "packet_device_invalid_packet",
              "empty packet should return a stable error code") &&
       ok;
  ok = expect(!unknown.ok &&
                  unknown.code == "darwin_utun_unsupported_ip_version",
              "unknown IP version should return a stable error code") &&
       ok;
  ok = expect(state->written_frames.empty(),
              "invalid packets should not be written to utun") &&
       ok;
  return ok;
}

bool utun_read_write_failures_are_propagated() {
  auto state = make_state();
  state->read_result =
      invalid("darwin_utun_read_failed", "failed to read utun frame");
  state->write_result =
      invalid("darwin_utun_write_failed", "failed to write utun frame");
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));
  auto opened = device.open(metadata());

  std::vector<std::uint8_t> packet;
  auto read = device.read_packet(&packet);
  auto written = device.write_packet({0x45, 0x01, 0x02});

  bool ok = true;
  ok = expect(opened.ok, "device should open before failure propagation test") &&
       ok;
  ok = expect(!read.ok && read.code == "darwin_utun_read_failed",
              "utun read failure should propagate with its stable code") &&
       ok;
  ok = expect(!written.ok && written.code == "darwin_utun_write_failed",
              "utun write failure should propagate with its stable code") &&
       ok;
  ok = expect(state->written_frames.empty(),
              "failed utun write should not record a frame") &&
       ok;
  return ok;
}

bool real_utun_read_timeout_returns_retryable_no_data() {
  auto utun_state = std::make_shared<TestNativeUtunState>();
  auto io_state = std::make_shared<TestPacketIoState>();
  io_state->readable_results = {0};

  auto session = ecnuvpn::platform::create_native_darwin_utun_packet_session(
      metadata(), test_utun_api(utun_state), test_packet_io_api(io_state));

  auto started = session->start();
  std::vector<std::uint8_t> frame;
  auto read = session->read_frame(&frame);
  session->stop();

  bool ok = true;
  ok = expect(started.ok(), "native utun test session should start") && ok;
  ok = expect(io_state->set_nonblocking_calls == 1,
              "utun fd should be switched to nonblocking mode") &&
       ok;
  ok = expect(!read.ok && read.code == "no_data",
              "quiet utun read should return retryable no_data") &&
       ok;
  ok = expect(io_state->read_calls == 0,
              "quiet utun read should not enter blocking read after timeout") &&
       ok;
  ok = expect(frame.empty(), "no_data read should not produce a frame") && ok;
  return ok;
}

bool real_utun_read_transients_are_retryable() {
  auto eintr_utun = std::make_shared<TestNativeUtunState>();
  auto eintr_io = std::make_shared<TestPacketIoState>();
  eintr_io->readable_results = {1};
  eintr_io->read_results = {-1};
  eintr_io->read_errors = {EINTR};
  auto eintr_session =
      ecnuvpn::platform::create_native_darwin_utun_packet_session(
          metadata(), test_utun_api(eintr_utun),
          test_packet_io_api(eintr_io));

  auto eintr_started = eintr_session->start();
  std::vector<std::uint8_t> frame;
  auto eintr_read = eintr_session->read_frame(&frame);
  eintr_session->stop();

  auto eagain_utun = std::make_shared<TestNativeUtunState>();
  auto eagain_io = std::make_shared<TestPacketIoState>();
  eagain_io->readable_results = {1};
  eagain_io->read_results = {-1};
  eagain_io->read_errors = {EAGAIN};
  auto eagain_session =
      ecnuvpn::platform::create_native_darwin_utun_packet_session(
          metadata(), test_utun_api(eagain_utun),
          test_packet_io_api(eagain_io));

  auto eagain_started = eagain_session->start();
  auto eagain_read = eagain_session->read_frame(&frame);
  eagain_session->stop();

  bool ok = true;
  ok = expect(eintr_started.ok() && eagain_started.ok(),
              "native utun transient read sessions should start") &&
       ok;
  ok = expect(!eintr_read.ok && eintr_read.code == "try_again",
              "EINTR utun read should return retryable try_again") &&
       ok;
  ok = expect(!eagain_read.ok && eagain_read.code == "would_block",
              "EAGAIN utun read should return retryable would_block") &&
       ok;
  return ok;
}

bool real_utun_write_retries_eintr_and_partial_writes() {
  auto utun_state = std::make_shared<TestNativeUtunState>();
  auto io_state = std::make_shared<TestPacketIoState>();
  io_state->writable_results = {1, 1, 1};
  io_state->write_results = {-1, 2, 3};
  io_state->write_errors = {EINTR};

  auto session = ecnuvpn::platform::create_native_darwin_utun_packet_session(
      metadata(), test_utun_api(utun_state), test_packet_io_api(io_state));

  auto started = session->start();
  auto written = session->write_frame({1, 2, 3, 4, 5});
  session->stop();

  bool ok = true;
  ok = expect(started.ok(), "native utun write test session should start") &&
       ok;
  ok = expect(written.ok,
              "utun write should retry EINTR and finish partial writes") &&
       ok;
  ok = expect(io_state->write_calls == 3,
              "utun write should retry after EINTR and continue after partial write") &&
       ok;
  ok = expect(io_state->write_chunks ==
                  std::vector<std::vector<std::uint8_t>>(
                      {{1, 2}, {3, 4, 5}}),
              "partial utun writes should resume at the unwritten offset") &&
       ok;
  return ok;
}

bool route_cleanup_retry_uses_stable_configured_utun_index() {
  struct RouteMock {
    std::uint32_t resolved_index = 321;
    std::vector<ecnuvpn::platform::NativeDarwinRoute> added_routes;
    std::vector<ecnuvpn::platform::NativeDarwinRoute> deleted_routes;
  } mock;

  ecnuvpn::platform::NativeDarwinRouteApi api;
  api.set_interface_mtu = [](const std::string &, int) { return 0; };
  api.get_best_route =
      [](const std::string &,
         ecnuvpn::platform::NativeDarwinUpstreamRoute &) { return 0; };
  api.add_route = [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
    mock.added_routes.push_back(route);
    return 0;
  };
  api.delete_route =
      [&mock](const ecnuvpn::platform::NativeDarwinRoute &route) {
        mock.deleted_routes.push_back(route);
        return 0;
      };
  api.interface_index_from_name =
      [&mock](const std::string &) { return mock.resolved_index; };

  ecnuvpn::platform::NativeDarwinRouteConfigOptions opts;
  opts.interface_name = "utun42";
  opts.configured_mtu = 1400;

  ecnuvpn::vpn_engine::TunnelMetadata meta = metadata();
  meta.interface_name = "utun42";
  meta.interface_index = -1;
  meta.routes = {"59.78.176.0/20"};
  meta.server_bypass_ips.clear();

  ecnuvpn::platform::NativeDarwinRouteConfig config(api, opts);
  auto configured = config.configure(meta);
  mock.resolved_index = 0;
  auto cleaned = config.cleanup();

  bool ok = true;
  ok = expect(configured.ok() && cleaned.ok(),
              "route config should configure and cleanup with mocked APIs") &&
       ok;
  ok = expect(mock.added_routes.size() == 1 &&
                  mock.added_routes[0].interface_index == 321 &&
                  mock.added_routes[0].message_interface_index == 321,
              "configured split route should retain resolved utun indexes") &&
       ok;
  ok = expect(mock.deleted_routes.size() == 1 &&
                  mock.deleted_routes[0].interface_index == 321 &&
                  mock.deleted_routes[0].message_interface_index == 321,
              "cleanup retry should not depend on live utun name lookup") &&
       ok;
  return ok;
}

bool route_config_failure_cleans_up_partial_open() {
  auto state = make_state();
  state->configure_result.error =
      ecnuvpn::platform::NativeDarwinRouteConfigError::route_add_failed;
  state->configure_result.message = "route failed";
  state->configure_result.target = "59.78.176.0/20";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(!result.ok, "route config failure should fail open") && ok;
  ok = expect(result.code ==
                  "native_darwin_route_config_route_add_failed",
              "route config failure should map to a stable packet device code") &&
       ok;
  ok = expect(state->route_cleanups == 1,
              "failed open should cleanup partial Darwin routes") &&
       ok;
  ok = expect(state->utun_stops == 1,
              "failed open should stop the utun session") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure",
                                            "route_cleanup", "utun_stop"}),
              "failed open cleanup should remove routes before stopping utun") &&
       ok;
  return ok;
}

bool close_cleans_routes_before_utun_and_is_idempotent() {
  auto state = make_state();
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));
  auto opened = device.open(metadata());
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before close") && ok;
  ok = expect(state->route_cleanups == 1,
              "close should cleanup native routes exactly once") &&
       ok;
  ok = expect(state->utun_stops == 1, "close should stop utun exactly once") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure",
                                            "route_cleanup", "utun_stop"}),
              "close should cleanup routes before stopping utun") &&
       ok;
  return ok;
}

bool close_cleanup_failure_keeps_cleanup_retry_available() {
  auto state = make_state();
  state->cleanup_result.error =
      ecnuvpn::platform::NativeDarwinRouteConfigError::route_delete_failed;
  state->cleanup_result.message = "delete failed";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before cleanup retry test") && ok;
  ok = expect(state->route_cleanups == 2,
              "failed close cleanup should keep routes for a later retry") &&
       ok;
  ok = expect(state->utun_stops == 1,
              "utun should still stop after route cleanup fails") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure",
                                            "route_cleanup", "utun_stop",
                                            "route_cleanup"}),
              "close should retry route cleanup after stopping utun once") &&
       ok;
  return ok;
}

bool successful_close_retry_clears_cleanup_state() {
  auto state = make_state();
  ecnuvpn::platform::NativeDarwinRouteConfigResult cleanup_failure;
  cleanup_failure.error =
      ecnuvpn::platform::NativeDarwinRouteConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  state->cleanup_results = {cleanup_failure, {}};
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto opened = device.open(metadata());
  device.close();
  device.close();
  device.close();

  bool ok = true;
  ok = expect(opened.ok, "open should succeed before cleanup retry") && ok;
  ok = expect(state->route_cleanups == 2,
              "successful retry should clear route cleanup state") &&
       ok;
  ok = expect(state->utun_stops == 1,
              "utun should be stopped once even when cleanup needs retry") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure",
                                            "route_cleanup", "utun_stop",
                                            "route_cleanup"}),
              "successful retry should leave no route cleanup work") &&
       ok;
  return ok;
}

bool open_rollback_cleanup_failure_is_reported_and_retryable() {
  auto state = make_state();
  state->configure_result.error =
      ecnuvpn::platform::NativeDarwinRouteConfigError::route_add_failed;
  state->configure_result.message = "route failed";
  ecnuvpn::platform::NativeDarwinRouteConfigResult cleanup_failure;
  cleanup_failure.error =
      ecnuvpn::platform::NativeDarwinRouteConfigError::route_delete_failed;
  cleanup_failure.message = "delete failed";
  cleanup_failure.target = "59.78.176.0/20";
  state->cleanup_results = {cleanup_failure, {}};
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());
  device.close();

  bool ok = true;
  ok = expect(!result.ok, "route config failure should fail open") && ok;
  ok = expect(result.code ==
                  "native_darwin_route_config_route_add_failed",
              "open should keep the original route config failure code") &&
       ok;
  ok = expect(result.message.find(
                  "native_darwin_route_config_route_delete_failed") !=
                  std::string::npos,
              "rollback cleanup failure should be reported stably") &&
       ok;
  ok = expect(state->route_cleanups == 2,
              "rollback cleanup failure should remain retryable") &&
       ok;
  ok = expect(state->utun_stops == 1,
              "open rollback should stop utun even when cleanup fails") &&
       ok;
  ok = expect(state->events ==
                  std::vector<std::string>({"utun_start", "route_configure",
                                            "route_cleanup", "utun_stop",
                                            "route_cleanup"}),
              "rollback should cleanup before utun stop and retry later") &&
       ok;
  return ok;
}

bool utun_start_failure_skips_route_config() {
  auto state = make_state();
  state->start_result.error =
      ecnuvpn::platform::NativeUtunError::socket_open_failed;
  state->start_result.detail = "socket failed";
  ecnuvpn::platform::NativePacketDevice device(dependencies(state));

  auto result = device.open(metadata());

  bool ok = true;
  ok = expect(!result.ok, "utun start failure should fail open") && ok;
  ok = expect(result.code == "native_utun_socket_open_failed",
              "utun start failure should map to a stable packet device code") &&
       ok;
  ok = expect(state->route_configures == 0,
              "route config should not run when utun start fails") &&
       ok;
  ok = expect(state->utun_stops == 0,
              "device should not stop a utun session that never started") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = open_starts_utun_and_configures_darwin_routes() && ok;
  ok = packet_io_strips_and_adds_utun_address_family_header() && ok;
  ok = closed_device_rejects_packet_io() && ok;
  ok = invalid_utun_frames_return_stable_errors() && ok;
  ok = invalid_packets_return_stable_write_errors() && ok;
  ok = utun_read_write_failures_are_propagated() && ok;
  ok = real_utun_read_timeout_returns_retryable_no_data() && ok;
  ok = real_utun_read_transients_are_retryable() && ok;
  ok = real_utun_write_retries_eintr_and_partial_writes() && ok;
  ok = route_cleanup_retry_uses_stable_configured_utun_index() && ok;
  ok = route_config_failure_cleans_up_partial_open() && ok;
  ok = close_cleans_routes_before_utun_and_is_idempotent() && ok;
  ok = close_cleanup_failure_keeps_cleanup_retry_available() && ok;
  ok = successful_close_retry_clears_cleanup_state() && ok;
  ok = open_rollback_cleanup_failure_is_reported_and_retryable() && ok;
  ok = utun_start_failure_skips_route_config() && ok;
  return ok ? 0 : 1;
}
