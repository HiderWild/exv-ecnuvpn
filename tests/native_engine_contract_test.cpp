#include "core/config/config.hpp"
#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/production_transport.hpp"
#include "vpn_engine/protocol/session.hpp"
#include "vpn_engine/session_state.hpp"

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <iostream>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

namespace {

static const char *MOCK_PASSWORD = "test-mock-password-placeholder";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

class MockPacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  ecnuvpn::vpn_engine::ValidationResult open(
      const ecnuvpn::vpn_engine::DeviceConfig & /*config*/) override {
    opened_ = true;
    return {};
  }
  ecnuvpn::vpn_engine::ValidationResult open(
      const ecnuvpn::vpn_engine::TunnelMetadata & /*metadata*/) override {
    opened_ = true;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "null_packet", "packet output must not be null"};
    if (read_queue_.empty())
      return {false, "no_data", "no packet available"};

    *packet = read_queue_.front();
    read_queue_.erase(read_queue_.begin());
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    last_written_ = packet;
    return {};
  }

  void close() override { closed_ = true; }

  void push_read_packet(std::vector<std::uint8_t> packet) {
    read_queue_.push_back(std::move(packet));
  }

  bool opened() const { return opened_; }
  bool closed() const { return closed_; }
  const std::vector<std::uint8_t> &last_written() const { return last_written_; }

private:
  bool opened_ = false;
  bool closed_ = false;
  std::vector<std::uint8_t> last_written_;
  std::vector<std::vector<std::uint8_t>> read_queue_;
};

class RecordingEventSink final : public ecnuvpn::vpn_engine::EventSink {
public:
  void emit(const ecnuvpn::vpn_engine::VpnEngineEvent &event) override {
    const std::lock_guard<std::mutex> lock(mu_);
    events_.push_back(event);
  }

  bool contains(const std::string &type) const {
    const std::lock_guard<std::mutex> lock(mu_);
    for (const auto &event : events_) {
      if (event.type == type)
        return true;
    }
    return false;
  }

  int count(const std::string &type) const {
    const std::lock_guard<std::mutex> lock(mu_);
    int matches = 0;
    for (const auto &event : events_) {
      if (event.type == type)
        ++matches;
    }
    return matches;
  }

private:
  mutable std::mutex mu_;
  std::vector<ecnuvpn::vpn_engine::VpnEngineEvent> events_;
};

class FakeProtocolTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  struct State {
    bool auth_ok = true;
    bool cstp_ok = true;
    int auth_count = 0;
    int cstp_count = 0;
    int exchange_count = 0;
    int disconnect_count = 0;
    int reset_count = 0;
    int cstp_mtu = 1400;
    std::string last_cookie;
    ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions last_options;
  };

  explicit FakeProtocolTransport(std::shared_ptr<State> state)
      : state_(std::move(state)) {}

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          &options) override {
    state_->last_options = options;
    ++state_->auth_count;
    if (!state_->auth_ok) {
      ecnuvpn::vpn_engine::protocol::AuthResult auth;
      auth.ok = false;
      auth.error_code = "auth_failed";
      auth.error_message = "invalid username or password";
      return auth;
    }

    ecnuvpn::vpn_engine::protocol::AuthResult auth;
    auth.ok = true;
    auth.cookie = "webvpn_session=FAKE_COOKIE";
    return auth;
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               ecnuvpn::vpn_engine::TunnelMetadata *metadata) override {
    state_->last_cookie = cookie;
    ++state_->cstp_count;
    if (!state_->cstp_ok)
      return {false, "cstp_failed", "CSTP refused connection"};
    if (!metadata)
      return {false, "cstp_null_metadata", "metadata output must not be null"};

    metadata->interface_name = "fake-cstp0";
    metadata->interface_index = 7;
    metadata->internal_ip4_address = "10.255.0.10";
    metadata->internal_ip4_netmask = "255.255.255.0";
    metadata->mtu = state_->cstp_mtu;
    metadata->routes = {"198.51.100.0/24"};
    metadata->server_bypass_ips = {"192.0.2.10"};
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) override {
    std::unique_lock<std::mutex> lock(transport_mu_);
    ++state_->exchange_count;
    if (transport_closed_)
      return {false, "transport_closed", "fake transport is closed"};
    echo_queue_.push_back(packet);
    transport_cv_.notify_all();
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind /*kind*/)
      override {
    std::unique_lock<std::mutex> lock(transport_mu_);
    if (transport_closed_)
      return {false, "transport_closed", "fake transport is closed"};
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame *out) override {
    if (!out)
      return {false, "packet_null_out", "inbound frame output must not be null"};
    out->kind = ecnuvpn::vpn_engine::protocol::InboundFrameKind::none;
    out->payload.clear();

    std::unique_lock<std::mutex> lock(transport_mu_);
    transport_cv_.wait(
        lock, [this] { return !echo_queue_.empty() || transport_closed_; });
    if (!echo_queue_.empty()) {
      out->kind = ecnuvpn::vpn_engine::protocol::InboundFrameKind::data;
      out->payload = std::move(echo_queue_.front());
      echo_queue_.pop_front();
      return {};
    }
    return {false, "transport_closed", "fake transport is closed"};
  }

  void disconnect() override {
    {
      std::unique_lock<std::mutex> lock(transport_mu_);
      transport_closed_ = true;
      transport_cv_.notify_all();
    }
    ++state_->disconnect_count;
  }

  void reset_for_reconnect() override {
    {
      std::unique_lock<std::mutex> lock(transport_mu_);
      transport_closed_ = false;
      echo_queue_.clear();
      transport_cv_.notify_all();
    }
    ++state_->reset_count;
  }

private:
  std::shared_ptr<State> state_;
  std::mutex transport_mu_;
  std::condition_variable transport_cv_;
  std::deque<std::vector<std::uint8_t>> echo_queue_;
  bool transport_closed_ = false;
};

struct TlsStreamLifetimeState {
  bool destroyed = false;
};

class LifetimeTlsStream final
    : public ecnuvpn::vpn_engine::protocol::TlsStream {
public:
  explicit LifetimeTlsStream(std::shared_ptr<TlsStreamLifetimeState> state)
      : state_(std::move(state)) {}

  ~LifetimeTlsStream() override { state_->destroyed = true; }

  ecnuvpn::vpn_engine::ValidationResult connect(
      const ecnuvpn::vpn_engine::protocol::TlsEndpoint & /*endpoint*/)
      override {
    return {false, "unexpected_connect", "lifetime test should not connect"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> & /*bytes*/) override {
    return {false, "unexpected_write", "lifetime test should not write"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_some(std::vector<std::uint8_t> * /*bytes*/) override {
    return {false, "unexpected_read", "lifetime test should not read"};
  }

  void close() override {}

private:
  std::shared_ptr<TlsStreamLifetimeState> state_;
};

struct PacketDeviceState {
  mutable std::mutex mu;
  bool open = false;
  int open_count = 0;
  int close_count = 0;
  int read_count = 0;
  std::thread::id read_thread_id;
  std::thread::id close_thread_id;
  std::vector<std::vector<std::uint8_t>> written_packets;
  ecnuvpn::vpn_engine::TunnelMetadata last_open_metadata;
};

class ScriptedEnginePacketDevice final
    : public ecnuvpn::vpn_engine::PacketDevice {
public:
  ScriptedEnginePacketDevice(std::shared_ptr<PacketDeviceState> state,
                             std::vector<std::vector<std::uint8_t>> packets = {})
      : state_(std::move(state)), packets_(std::move(packets)) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    ++state_->read_count;
    if (!state_->open)
      return {false, "packet_device_closed", "packet device is closed"};
    if (packets_.empty())
      return {false, "packet_device_empty", "no packet is queued"};

    *packet = packets_.front();
    packets_.erase(packets_.begin());
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (!state_->open)
      return {false, "packet_device_closed", "packet device is closed"};
    state_->written_packets.push_back(packet);
    return {};
  }

  void close() override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (state_->open) {
      state_->open = false;
      ++state_->close_count;
      state_->close_thread_id = std::this_thread::get_id();
    }
  }

private:
  std::shared_ptr<PacketDeviceState> state_;
  std::vector<std::vector<std::uint8_t>> packets_;
};

class PollingPacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  explicit PollingPacketDevice(std::shared_ptr<PacketDeviceState> state)
      : state_(std::move(state)) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    ++state_->read_count;
    state_->read_thread_id = std::this_thread::get_id();
    if (!state_->open)
      return {false, "packet_device_closed", "packet device is closed"};
    packet->clear();
    return {false, "no_data", "no packet available"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {false, "unexpected_write", "no packet should be written"};
  }

  void close() override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (state_->open) {
      state_->open = false;
      ++state_->close_count;
      state_->close_thread_id = std::this_thread::get_id();
    }
  }

private:
  std::shared_ptr<PacketDeviceState> state_;
};

class FailingOpenPacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  explicit FailingOpenPacketDevice(std::shared_ptr<PacketDeviceState> state)
      : state_(std::move(state)) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    ++state_->open_count;
    return {false, "packet_device_open_failed",
            "packet device refused to open"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    ++state_->open_count;
    return {false, "packet_device_open_failed",
            "packet device refused to open"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> * /*packet*/) override {
    return {false, "unexpected_read", "packet loop should not read"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {false, "unexpected_write", "packet loop should not write"};
  }

  void close() override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    if (state_->open) {
      state_->open = false;
      ++state_->close_count;
      state_->close_thread_id = std::this_thread::get_id();
    }
  }

private:
  std::shared_ptr<PacketDeviceState> state_;
};

int open_count(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->open_count;
}

int close_count(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->close_count;
}

int read_count(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->read_count;
}

bool is_open(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->open;
}

std::thread::id close_thread_id(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->close_thread_id;
}

std::thread::id read_thread_id(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->read_thread_id;
}

std::vector<std::vector<std::uint8_t>>
written_packets(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->written_packets;
}

ecnuvpn::vpn_engine::TunnelMetadata
last_open_metadata(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->last_open_metadata;
}

std::unique_ptr<FakeProtocolTransport>
make_fake_transport(const std::shared_ptr<FakeProtocolTransport::State> &state) {
  return std::make_unique<FakeProtocolTransport>(state);
}

std::unique_ptr<ScriptedEnginePacketDevice>
make_scripted_device(const std::shared_ptr<PacketDeviceState> &state,
                     std::vector<std::vector<std::uint8_t>> packets = {}) {
  return std::make_unique<ScriptedEnginePacketDevice>(state, std::move(packets));
}

std::unique_ptr<PollingPacketDevice>
make_polling_device(const std::shared_ptr<PacketDeviceState> &state) {
  return std::make_unique<PollingPacketDevice>(state);
}

std::unique_ptr<FailingOpenPacketDevice>
make_failing_open_device(const std::shared_ptr<PacketDeviceState> &state) {
  return std::make_unique<FailingOpenPacketDevice>(state);
}

ecnuvpn::vpn_engine::VpnEngineConfig engine_config() {
  ecnuvpn::vpn_engine::VpnEngineConfig cfg;
  cfg.server = "https://vpn.example.invalid/+CSCOE+/logon.html";
  cfg.username = "alice";
  cfg.password = MOCK_PASSWORD;
  cfg.useragent = "ECNU-VPN native test";
  cfg.disable_dtls = true;
  cfg.auto_reconnect = false;
  return cfg;
}

template <typename Predicate>
bool wait_until(Predicate predicate,
                std::chrono::milliseconds timeout =
                    std::chrono::milliseconds(1000)) {
  const auto deadline = std::chrono::steady_clock::now() + timeout;
  while (std::chrono::steady_clock::now() < deadline) {
    if (predicate())
      return true;
    std::this_thread::sleep_for(std::chrono::milliseconds(2));
  }
  return predicate();
}

bool test_injected_fake_start_runs_packet_loop_and_cleans_up() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();
  RecordingEventSink events;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(
        device,
        std::vector<std::vector<std::uint8_t>>{{0x45, 0x00, 0x00, 0x2a}});
  };
  deps.event_sink = &events;

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok, "injected native start should succeed") && ok;
  ok = expect(started.code != "native_protocol_unimplemented",
              "injected native start must not return the old stub code") &&
       ok;
  ok = expect(transport->auth_count == 1,
              "start should authenticate through injected transport") &&
       ok;
  ok = expect(transport->cstp_count == 1,
              "start should connect CSTP through injected transport") &&
       ok;
  ok = expect(transport->last_options.server.host == "vpn.example.invalid",
              "start should parse VPN server URL before auth") &&
       ok;
  ok = expect(transport->last_options.server.base_path == "/+CSCOE+/logon.html",
              "start should preserve VPN server URL path") &&
       ok;
  ok = expect(open_count(device) == 1,
              "start should open the injected packet device") &&
       ok;

  const ecnuvpn::vpn_engine::TunnelMetadata opened_metadata =
      last_open_metadata(device);
  ok = expect(opened_metadata.mtu == 1400,
              "negotiated tunnel MTU should reach the packet device") &&
       ok;
  ok = expect(opened_metadata.routes ==
                  std::vector<std::string>{"198.51.100.0/24"},
              "split-include routes should reach the packet device") &&
       ok;
  ok = expect(opened_metadata.server_bypass_ips ==
                  std::vector<std::string>{"192.0.2.10"},
              "server-bypass IPs should reach the packet device") &&
       ok;

  ok = expect(wait_until([&device]() {
                return written_packets(device).size() == 1;
              }),
              "packet loop should echo queued packet through transport") &&
       ok;
  ok = expect(written_packets(device)[0] ==
                  std::vector<std::uint8_t>({0x45, 0x00, 0x00, 0x2a}),
              "packet loop should write echoed packet bytes") &&
       ok;

  ok = expect(wait_until([&session]() {
                const ecnuvpn::vpn_engine::VpnEngineStatus status =
                    session.status();
                return !status.running && !status.network_ready;
              }),
              "clean packet loop exit should clear running and network_ready") &&
       ok;

  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();
  ok = expect(status.pid == -1,
              "native session should not report an OpenConnect PID") &&
       ok;
  ok = expect(status.interface_name == "fake-cstp0",
              "status should bridge tunnel interface metadata") &&
       ok;
  ok = expect(status.internal_ip == "10.255.0.10",
              "status should bridge tunnel IP metadata") &&
       ok;
  ok = expect(events.contains("packet.loop.started"),
              "start should emit packet loop event") &&
       ok;
  ok = expect(events.contains("cstp.connected"),
              "start should emit structured CSTP event") &&
       ok;
  ok = expect(close_count(device) == 1,
              "clean packet loop exit should close the packet device") &&
       ok;

  session.stop();
  ok = expect(close_count(device) == 1,
              "stop after clean packet loop exit must not close device again") &&
       ok;

  return ok;
}

bool test_dtls_config_flag_does_not_block_native_engine() {
  // The native engine v1 is CSTP/TLS-only by design. Historically the engine
  // had a run-time guard that rejected configs with disable_dtls=false.
  // That guard was a bug: the Windows platform default for disable_dtls is
  // false (only relevant for the OpenConnect CLI path), so the native engine
  // would crash within 250ms for all users who had not explicitly set
  // disable_dtls=true in config.json. The guard has been removed; the engine
  // forces CSTP mode internally and accepts configs regardless of the flag.
  bool ok = true;

  // Use a fake transport so the test doesn't attempt real network I/O.
  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->auth_ok = false; // fail at auth so we stop cleanly

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = []() {
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  ecnuvpn::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.disable_dtls = false; // intentionally set the "wrong" flag

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(cfg, deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();
  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();

  // Must NOT be rejected as unsupported_dtls — engine forces CSTP mode
  // regardless of this flag and proceeds to auth.
  ok = expect(started.code != "unsupported_dtls",
              "native engine must not reject disable_dtls=false configs") && ok;
  // Should fail at auth (fake transport rejects auth), not at the DTLS guard.
  ok = expect(!started.ok, "start should fail at auth stage with fake transport") && ok;
  ok = expect(!status.running && !status.network_ready,
              "session must not be running after auth failure") && ok;

  return ok;
}

bool test_auth_failure_maps_error_without_device() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->auth_ok = false;
  int packet_devices_created = 0;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();
  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(!started.ok, "auth failure should fail native start") && ok;
  ok = expect(started.code == "auth_failed",
              "auth failure should surface deterministic auth_failed") &&
       ok;
  ok = expect(status.error_code == "auth_failed",
              "status should retain auth failure code") &&
       ok;
  ok = expect(transport->auth_count == 1,
              "auth failure should attempt auth exactly once") &&
       ok;
  ok = expect(transport->cstp_count == 0,
              "auth failure must not connect CSTP") &&
       ok;
  ok = expect(packet_devices_created == 0,
              "auth failure must not create or open packet device") &&
       ok;
  ok = expect(!status.running && !status.network_ready,
              "auth failure must not leave session running") &&
       ok;

  return ok;
}

bool test_cstp_failure_maps_error_without_device_open() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->cstp_ok = false;
  int packet_devices_created = 0;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();
  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(!started.ok, "CSTP failure should fail native start") && ok;
  ok = expect(started.code == "cstp_failed",
              "CSTP failure should surface deterministic code") &&
       ok;
  ok = expect(status.error_code == "cstp_failed",
              "status should retain CSTP failure code") &&
       ok;
  ok = expect(transport->auth_count == 1,
              "CSTP failure should authenticate first") &&
       ok;
  ok = expect(transport->cstp_count == 1,
              "CSTP failure should attempt CSTP once") &&
       ok;
  ok = expect(packet_devices_created == 0,
              "CSTP failure must not create or open packet device") &&
       ok;
  ok = expect(!status.running && !status.network_ready,
              "CSTP failure must not leave session running") &&
       ok;

  return ok;
}

bool test_packet_loop_start_failure_emits_native_start_failed() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();
  RecordingEventSink events;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_failing_open_device(device);
  };
  deps.event_sink = &events;

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();
  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(!started.ok, "packet device open failure should fail start") && ok;
  ok = expect(started.code == "packet_device_open_failed",
              "packet device open failure should surface deterministic code") &&
       ok;
  ok = expect(status.error_code == "packet_device_open_failed",
              "status should retain packet loop startup failure code") &&
       ok;
  ok = expect(!status.running && !status.network_ready,
              "packet loop startup failure must not leave session running") &&
       ok;
  ok = expect(events.contains("packet_device.failed"),
              "packet loop startup failure should emit packet device event") &&
       ok;
  ok = expect(events.count("native.start.failed") == 1,
              "packet loop startup failure should emit native.start.failed once") &&
       ok;

  return ok;
}

bool test_stop_cancels_packet_loop_and_closes_device() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_polling_device(device);
  };

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok, "polling fake start should succeed") && ok;
  ok = expect(open_count(device) == 1,
              "polling fake start should open device") &&
       ok;
  ok = expect(session.status().network_ready,
              "polling fake start should reach packet loop before returning") &&
       ok;
  ok = expect(wait_until([&device]() { return read_count(device) > 0; }),
              "polling fake should be read by the packet loop before stop") &&
       ok;

  session.stop();
  const ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(close_count(device) == 1,
              "stop should close active packet device exactly once") &&
       ok;
  ok = expect(!is_open(device),
              "stop should leave packet device closed") &&
       ok;
  ok = expect(close_thread_id(device) == read_thread_id(device),
              "stop cancellation should let the packet loop close its device") &&
       ok;
  ok = expect(!status.running && !status.network_ready,
              "stop should clear running and network_ready status") &&
       ok;

  return ok;
}

bool test_default_dependencies_expose_factories() {
  bool ok = true;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps =
      ecnuvpn::vpn_engine::default_native_engine_dependencies();

  ok = expect(static_cast<bool>(deps.transport_factory),
              "default native dependencies should expose a transport factory") &&
       ok;
  ok = expect(static_cast<bool>(deps.packet_device_factory),
              "default native dependencies should expose a packet device factory") &&
       ok;

  if (deps.transport_factory) {
    std::unique_ptr<ecnuvpn::vpn_engine::protocol::ProtocolTransport> transport =
        deps.transport_factory();
    ok = expect(static_cast<bool>(transport),
                "default transport factory should create a transport") &&
         ok;
  }

  if (deps.packet_device_factory) {
    std::unique_ptr<ecnuvpn::vpn_engine::PacketDevice> device =
        deps.packet_device_factory();
    ok = expect(static_cast<bool>(device),
                "default packet device factory should create a packet device") &&
         ok;
  }

  return ok;
}

bool test_default_tls_only_start_does_not_report_missing_transport_factory() {
  bool ok = true;

  ecnuvpn::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.server = "https://127.0.0.1:1";

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(cfg);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();

  ok = expect(!started.ok,
              "default native start to a closed local port should fail") &&
       ok;
  ok = expect(started.code != "native_transport_unimplemented",
              "default native start must not fail with the missing transport factory code") &&
       ok;
  ok = expect(started.code != "native_packet_device_unimplemented",
              "default native start should fail before packet device creation on TLS connect failure") &&
       ok;

  return ok;
}

bool test_production_transport_can_own_tls_stream() {
  bool ok = true;

  auto state = std::make_shared<TlsStreamLifetimeState>();

  {
    std::unique_ptr<ecnuvpn::vpn_engine::protocol::TlsStream> stream(
        new LifetimeTlsStream(state));
    ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport transport(
        std::move(stream));

    ok = expect(!state->destroyed,
                "owned TLS stream should survive transport construction") &&
         ok;
  }

  ok = expect(state->destroyed,
              "owned TLS stream should be destroyed with the transport") &&
       ok;

  return ok;
}

bool test_invalid_metadata_mtu_falls_back_to_safe_default() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->cstp_mtu = 0; // gateway yields an unusable MTU
  auto device = std::make_shared<PacketDeviceState>();
  RecordingEventSink events;

  ecnuvpn::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(device, std::vector<std::vector<std::uint8_t>>{});
  };
  deps.event_sink = &events;

  ecnuvpn::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.mtu = 1290; // documented safe default

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(cfg, deps);
  const ecnuvpn::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok, "start should succeed despite an invalid gateway MTU") &&
       ok;
  ok = expect(wait_until([&device]() { return open_count(device) == 1; }),
              "packet device should still open with a fallback MTU") &&
       ok;
  ok = expect(last_open_metadata(device).mtu == 1290,
              "invalid gateway MTU should fall back to the configured default") &&
       ok;

  session.stop();
  return ok;
}

} // namespace

int main() {
  static_assert(std::is_abstract_v<ecnuvpn::vpn_engine::PacketDevice>,
                "PacketDevice must remain an abstract interface");

  bool ok = true;

  // PacketDevice contract: header must compile without platform headers.
  {
    MockPacketDevice dev;

    ecnuvpn::vpn_engine::TunnelMetadata meta;
    ecnuvpn::vpn_engine::ValidationResult o = dev.open(meta);
    ok = expect(o.ok, "mock open should succeed") && ok;
    ok = expect(dev.opened(), "mock open should update state") && ok;

    std::vector<std::uint8_t> write_pkt = {1, 2, 3};
    ecnuvpn::vpn_engine::ValidationResult w = dev.write_packet(write_pkt);
    ok = expect(w.ok, "mock write_packet should succeed") && ok;
    ok = expect(dev.last_written() == write_pkt,
                "mock write_packet should capture packet bytes") &&
         ok;

    dev.push_read_packet({9, 8});
    std::vector<std::uint8_t> read_pkt;
    ecnuvpn::vpn_engine::ValidationResult r = dev.read_packet(&read_pkt);
    ok = expect(r.ok, "mock read_packet should succeed when queued") && ok;
    ok = expect(read_pkt == std::vector<std::uint8_t>({9, 8}),
                "mock read_packet should return queued packet") &&
         ok;

    r = dev.read_packet(&read_pkt);
    ok = expect(!r.ok && r.code == "no_data",
                "mock read_packet should fail when no packet available") &&
         ok;

    r = dev.read_packet(nullptr);
    ok = expect(!r.ok && r.code == "null_packet",
                "mock read_packet should validate output pointer") &&
         ok;

    dev.close();
    ok = expect(dev.closed(), "mock close should update state") && ok;
  }

  ecnuvpn::Config cfg;
  cfg.vpn_engine = "native";
  cfg.server = "https://vpn-ct.ecnu.edu.cn";
  cfg.username = "alice";
  cfg.useragent = "AnyConnect Win_x86_64 4.10.05095";
  cfg.mtu = 1290;
  cfg.routes = {"59.78.176.0/20"};
  cfg.disable_dtls = true;
  cfg.extra_args = {"--dump-http-traffic"};

  ecnuvpn::vpn_engine::ValidationResult validation =
      ecnuvpn::vpn_engine::validate_native_config(cfg);
  ok = expect(!validation.ok,
              "native engine should reject legacy OpenConnect extra_args") &&
       ok;
  ok = expect(validation.code == "unsupported_extra_args",
              "native extra_args rejection should use a stable code") &&
       ok;

  cfg.extra_args.clear();
  validation = ecnuvpn::vpn_engine::validate_native_config(cfg);
  ok = expect(validation.ok, "native engine should accept v1 ECNU password config") &&
       ok;

  ecnuvpn::vpn_engine::VpnEngineConfig engine_cfg =
      ecnuvpn::vpn_engine::make_native_config(cfg, MOCK_PASSWORD);
  ok = expect(engine_cfg.server == cfg.server,
              "native engine config should carry server") &&
       ok;
  ok = expect(engine_cfg.password == MOCK_PASSWORD,
              "native engine config should carry per-session password") &&
       ok;
  ok = expect(engine_cfg.disable_dtls == true,
              "native engine config forces disable_dtls=true (CSTP-only; user flag is for OpenConnect)") &&
       ok;

  ecnuvpn::vpn_engine::VpnEngineEvent event;
  event.type = "auth";
  event.level = "info";
  event.message = "连接成功";
  event.fields["stage"] = "cookie";
  nlohmann::json event_json = ecnuvpn::vpn_engine::event_to_json(event);
  ok = expect(event_json.value("message", std::string()) == "连接成功",
              "native engine events should preserve UTF-8 text") &&
       ok;
  ok = expect(event_json["fields"].value("stage", std::string()) == "cookie",
              "native engine events should expose structured fields") &&
       ok;

  ecnuvpn::vpn_engine::NativeVpnEngineSession session(engine_cfg);
  ecnuvpn::vpn_engine::VpnEngineStatus status = session.status();
  ok = expect(!status.running && !status.network_ready && status.pid == -1,
              "new native session should not report an openconnect process") &&
       ok;

  ok = test_injected_fake_start_runs_packet_loop_and_cleans_up() && ok;
  ok = test_dtls_config_flag_does_not_block_native_engine() && ok;
  ok = test_auth_failure_maps_error_without_device() && ok;
  ok = test_cstp_failure_maps_error_without_device_open() && ok;
  ok = test_packet_loop_start_failure_emits_native_start_failed() && ok;
  ok = test_stop_cancels_packet_loop_and_closes_device() && ok;
  ok = test_default_dependencies_expose_factories() && ok;
  ok = test_default_tls_only_start_does_not_report_missing_transport_factory() &&
       ok;
  ok = test_production_transport_can_own_tls_stream() && ok;
  ok = test_invalid_metadata_mtu_falls_back_to_safe_default() && ok;

  return ok ? 0 : 1;
}
