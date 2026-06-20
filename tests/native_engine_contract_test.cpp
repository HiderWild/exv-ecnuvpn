#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/native_handshake_job.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/production_transport.hpp"
#include "vpn_engine/protocol/session.hpp"
#include "vpn_engine/session_state.hpp"

#include <atomic>
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

class MockPacketDevice final : public exv::vpn_engine::PacketDevice {
public:
  exv::vpn_engine::ValidationResult open(
      const exv::vpn_engine::DeviceConfig & /*config*/) override {
    opened_ = true;
    return {};
  }
  exv::vpn_engine::ValidationResult open(
      const exv::vpn_engine::TunnelMetadata & /*metadata*/) override {
    opened_ = true;
    return {};
  }

  exv::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "null_packet", "packet output must not be null"};
    if (read_queue_.empty())
      return {false, "no_data", "no packet available"};

    *packet = read_queue_.front();
    read_queue_.erase(read_queue_.begin());
    return {};
  }

  exv::vpn_engine::ValidationResult
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

class RecordingEventSink final : public exv::vpn_engine::EventSink {
public:
  void emit(const exv::vpn_engine::VpnEngineEvent &event) override {
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

  bool contains_field(const std::string &type, const std::string &key,
                      const std::string &value) const {
    const std::lock_guard<std::mutex> lock(mu_);
    for (const auto &event : events_) {
      if (event.type != type)
        continue;
      const auto found = event.fields.find(key);
      if (found != event.fields.end() && found->second == value)
        return true;
    }
    return false;
  }

private:
  mutable std::mutex mu_;
  std::vector<exv::vpn_engine::VpnEngineEvent> events_;
};

class FakeProtocolTransport final
    : public exv::vpn_engine::protocol::ProtocolTransport {
public:
  struct State {
    bool auth_ok = true;
    bool cstp_ok = true;
    std::string auth_error_code = "auth_failed";
    std::string auth_error_message = "invalid username or password";
    std::string auth_prompt_label;
    std::string auth_prompt_type;
    std::string auth_group_options;
    int auth_count = 0;
    int cstp_count = 0;
    int exchange_count = 0;
    int disconnect_count = 0;
    int reset_count = 0;
    int cstp_mtu = 1400;
    int keepalive_seconds = 0;
    std::atomic<int> keepalive_control_count{0};
    std::string last_cookie;
    exv::vpn_engine::protocol::ProtocolSessionOptions last_options;
  };

  explicit FakeProtocolTransport(std::shared_ptr<State> state)
      : state_(std::move(state)) {}

  exv::vpn_engine::protocol::AuthResult authenticate(
      const exv::vpn_engine::protocol::ProtocolSessionOptions
          &options) override {
    state_->last_options = options;
    ++state_->auth_count;
    if (!state_->auth_ok) {
      exv::vpn_engine::protocol::AuthResult auth;
      auth.ok = false;
      auth.error_code = state_->auth_error_code;
      auth.error_message = state_->auth_error_message;
      auth.interaction_prompt_label = state_->auth_prompt_label;
      auth.interaction_prompt_type = state_->auth_prompt_type;
      auth.interaction_group_options = state_->auth_group_options;
      return auth;
    }

    exv::vpn_engine::protocol::AuthResult auth;
    auth.ok = true;
    auth.cookie = "webvpn=FAKE_COOKIE";
    return auth;
  }

  exv::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               exv::vpn_engine::TunnelMetadata *metadata) override {
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
    metadata->keepalive_seconds = state_->keepalive_seconds;
    metadata->routes = {"198.51.100.0/24"};
    metadata->server_bypass_ips = {"192.0.2.10"};
    return {};
  }

  exv::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) override {
    std::unique_lock<std::mutex> lock(transport_mu_);
    ++state_->exchange_count;
    if (transport_closed_)
      return {false, "transport_closed", "fake transport is closed"};
    echo_queue_.push_back(packet);
    transport_cv_.notify_all();
    return {};
  }

  exv::vpn_engine::ValidationResult
  send_control(exv::vpn_engine::protocol::InboundFrameKind kind) override {
    std::unique_lock<std::mutex> lock(transport_mu_);
    if (transport_closed_)
      return {false, "transport_closed", "fake transport is closed"};
    if (kind == exv::vpn_engine::protocol::InboundFrameKind::keepalive)
      ++state_->keepalive_control_count;
    return {};
  }

  exv::vpn_engine::ValidationResult
  receive_frame(exv::vpn_engine::protocol::InboundFrame *out) override {
    if (!out)
      return {false, "packet_null_out", "inbound frame output must not be null"};
    out->kind = exv::vpn_engine::protocol::InboundFrameKind::none;
    out->payload.clear();

    std::unique_lock<std::mutex> lock(transport_mu_);
    transport_cv_.wait(
        lock, [this] { return !echo_queue_.empty() || transport_closed_; });
    if (!echo_queue_.empty()) {
      out->kind = exv::vpn_engine::protocol::InboundFrameKind::data;
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
    : public exv::vpn_engine::protocol::TlsStream {
public:
  explicit LifetimeTlsStream(std::shared_ptr<TlsStreamLifetimeState> state)
      : state_(std::move(state)) {}

  ~LifetimeTlsStream() override { state_->destroyed = true; }

  exv::vpn_engine::ValidationResult connect(
      const exv::vpn_engine::protocol::TlsEndpoint & /*endpoint*/)
      override {
    return {false, "unexpected_connect", "lifetime test should not connect"};
  }

  exv::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> & /*bytes*/) override {
    return {false, "unexpected_write", "lifetime test should not write"};
  }

  exv::vpn_engine::ValidationResult
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
  exv::vpn_engine::TunnelMetadata last_open_metadata;
  bool metadata_open_used = false;
};

class ScriptedEnginePacketDevice final
    : public exv::vpn_engine::PacketDevice {
public:
  ScriptedEnginePacketDevice(std::shared_ptr<PacketDeviceState> state,
                             std::vector<std::vector<std::uint8_t>> packets = {})
      : state_(std::move(state)), packets_(std::move(packets)) {}

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    state_->metadata_open_used = true;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  exv::vpn_engine::ValidationResult
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

  exv::vpn_engine::ValidationResult
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

class PollingPacketDevice final : public exv::vpn_engine::PacketDevice {
public:
  explicit PollingPacketDevice(std::shared_ptr<PacketDeviceState> state)
      : state_(std::move(state)) {}

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    state_->metadata_open_used = true;
    state_->open = true;
    ++state_->open_count;
    return {};
  }

  exv::vpn_engine::ValidationResult
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

  exv::vpn_engine::ValidationResult
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

class FailingOpenPacketDevice final : public exv::vpn_engine::PacketDevice {
public:
  explicit FailingOpenPacketDevice(std::shared_ptr<PacketDeviceState> state)
      : state_(std::move(state)) {}

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::DeviceConfig &config) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata.interface_name = config.interface_name;
    state_->last_open_metadata.mtu = config.mtu;
    ++state_->open_count;
    return {false, "packet_device_open_failed",
            "packet device refused to open"};
  }

  exv::vpn_engine::ValidationResult
  open(const exv::vpn_engine::TunnelMetadata &metadata) override {
    const std::lock_guard<std::mutex> lock(state_->mu);
    state_->last_open_metadata = metadata;
    state_->metadata_open_used = true;
    ++state_->open_count;
    return {false, "packet_device_open_failed",
            "packet device refused to open"};
  }

  exv::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> * /*packet*/) override {
    return {false, "unexpected_read", "packet loop should not read"};
  }

  exv::vpn_engine::ValidationResult
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

exv::vpn_engine::TunnelMetadata
last_open_metadata(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->last_open_metadata;
}

bool metadata_open_used(const std::shared_ptr<PacketDeviceState> &state) {
  const std::lock_guard<std::mutex> lock(state->mu);
  return state->metadata_open_used;
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

exv::vpn_engine::VpnEngineConfig engine_config() {
  exv::vpn_engine::VpnEngineConfig cfg;
  cfg.server = "https://vpn.example.invalid/+CSCOE+/logon.html";
  cfg.username = "alice";
  cfg.password = MOCK_PASSWORD;
  cfg.useragent = "EXV native test";
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

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(
        device,
        std::vector<std::vector<std::uint8_t>>{{0x45, 0x00, 0x00, 0x2a}});
  };
  deps.event_sink = &events;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();

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

  const exv::vpn_engine::TunnelMetadata opened_metadata =
      last_open_metadata(device);
  ok = expect(opened_metadata.mtu == 1400,
              "negotiated tunnel MTU should reach the packet device") &&
       ok;
  ok = expect(opened_metadata.routes.empty(),
              "split-include routes must not reach the packet device") &&
       ok;
  ok = expect(opened_metadata.server_bypass_ips.empty(),
              "server-bypass IPs must not reach the packet device") &&
       ok;
  ok = expect(!metadata_open_used(device),
              "native engine packet loop should use DeviceConfig open") &&
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
                const exv::vpn_engine::VpnEngineStatus status =
                    session.status();
                return !status.running && !status.network_ready;
              }),
              "clean packet loop exit should clear running and network_ready") &&
       ok;

  const exv::vpn_engine::VpnEngineStatus status = session.status();
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

bool test_network_configurator_runs_before_packet_open() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();
  bool configurator_called = false;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(device);
  };
  deps.network_configurator =
      [&device, &configurator_called](
          const exv::vpn_engine::TunnelMetadata &metadata,
          exv::vpn_engine::DeviceConfig *device_config) {
        configurator_called = true;
        if (!device_config)
          return exv::vpn_engine::ValidationResult{
              false, "device_config_missing",
              "device config output must not be null"};
        if (open_count(device) != 0)
          return exv::vpn_engine::ValidationResult{
              false, "packet_opened_too_early",
              "packet device opened before helper network config"};
        if (metadata.routes.empty() || metadata.server_bypass_ips.empty())
          return exv::vpn_engine::ValidationResult{
              false, "metadata_incomplete",
              "network configurator should receive CSTP route metadata"};

        device_config->interface_name = "helper-wintun0";
        device_config->mtu = 1320;
        return exv::vpn_engine::ValidationResult{};
      };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok,
              "start should succeed with injected network configurator") &&
       ok;
  ok = expect(configurator_called,
              "network configurator should run after CSTP and before packet open") &&
       ok;

  const exv::vpn_engine::TunnelMetadata opened_metadata =
      last_open_metadata(device);
  ok = expect(opened_metadata.interface_name == "helper-wintun0",
              "packet device should open using helper-prepared adapter name") &&
       ok;
  ok = expect(opened_metadata.mtu == 1320,
              "packet device should open using helper-prepared MTU") &&
       ok;
  ok = expect(opened_metadata.routes.empty() &&
                  opened_metadata.server_bypass_ips.empty(),
              "packet device should still receive only device config") &&
       ok;

  session.stop();
  return ok;
}

bool test_native_session_splits_handshake_from_packet_attach() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();
  int packet_devices_created = 0;
  int network_config_calls = 0;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&]() {
    ++packet_devices_created;
    return make_scripted_device(device);
  };
  deps.network_configurator =
      [&](const exv::vpn_engine::TunnelMetadata &metadata,
          exv::vpn_engine::DeviceConfig *device_config) {
        ++network_config_calls;
        if (metadata.internal_ip4_address != "10.255.0.10") {
          return exv::vpn_engine::ValidationResult{
              false, "metadata_missing", "handshake metadata missing"};
        }
        if (!device_config) {
          return exv::vpn_engine::ValidationResult{
              false, "device_config_missing", "device config missing"};
        }
        device_config->interface_name = "split-wintun0";
        device_config->mtu = 1310;
        return exv::vpn_engine::ValidationResult{};
      };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  exv::vpn_engine::TunnelMetadata metadata;
  const auto handshake = session.start_handshake(&metadata);

  ok = expect(handshake.ok, "split handshake should succeed") && ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "split handshake returns CSTP metadata") &&
       ok;
  ok = expect(packet_devices_created == 0,
              "split handshake must not create packet device") &&
       ok;
  ok = expect(network_config_calls == 0,
              "split handshake must not apply network config") &&
       ok;

  exv::vpn_engine::DeviceConfig device_config;
  device_config.interface_name = "split-wintun0";
  device_config.mtu = 1310;
  const auto attached = session.start_packet_loop(device_config);
  ok = expect(attached.ok, "split packet attach should succeed") && ok;
  ok = expect(packet_devices_created == 1,
              "packet attach creates packet device") &&
       ok;
  ok = expect(network_config_calls == 0,
              "packet attach must not apply network config") &&
       ok;
  ok = expect(last_open_metadata(device).interface_name == "split-wintun0",
              "packet attach uses configured adapter") &&
       ok;

  session.stop();
  return ok;
}

bool test_native_session_adopts_prepared_handshake_without_reauth() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();

  exv::vpn_engine::NativeVpnEngineDependencies handshake_deps;
  handshake_deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };

  exv::vpn_engine::NativeHandshakeResult prepared;
  exv::vpn_engine::NativeHandshakeJob job(engine_config(), handshake_deps);
  const auto prepared_result = job.run(std::stop_token{}, &prepared);

  ok = expect(prepared_result.ok, "prepared handshake should succeed") && ok;
  ok = expect(transport->auth_count == 1,
              "preparing handshake should authenticate exactly once") &&
       ok;
  ok = expect(transport->cstp_count == 1,
              "preparing handshake should connect CSTP exactly once") &&
       ok;

  auto device = std::make_shared<PacketDeviceState>();
  int unexpected_transport_factory_calls = 0;

  exv::vpn_engine::NativeVpnEngineDependencies attach_deps;
  attach_deps.transport_factory = [&]() {
    ++unexpected_transport_factory_calls;
    return make_fake_transport(transport);
  };
  attach_deps.packet_device_factory = [&]() {
    return make_scripted_device(device);
  };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(),
                                                      attach_deps);
  exv::vpn_engine::TunnelMetadata adopted_metadata;
  const auto adopted = session.adopt_handshake(std::move(prepared),
                                               &adopted_metadata);

  ok = expect(adopted.ok, "native session should adopt prepared handshake") &&
       ok;
  ok = expect(adopted_metadata.internal_ip4_address == "10.255.0.10",
              "adopted handshake should expose CSTP metadata") &&
       ok;

  exv::vpn_engine::DeviceConfig device_config;
  device_config.interface_name = "adopted-wintun0";
  device_config.mtu = 1300;
  const auto attached = session.start_packet_loop(device_config);

  ok = expect(attached.ok, "adopted handshake should start packet loop") && ok;
  ok = expect(unexpected_transport_factory_calls == 0,
              "adopting handshake must not create a new transport") &&
       ok;
  ok = expect(transport->auth_count == 1,
              "adopting handshake must not authenticate again") &&
       ok;
  ok = expect(transport->cstp_count == 1,
              "adopting handshake must not reconnect CSTP") &&
       ok;
  ok = expect(last_open_metadata(device).interface_name == "adopted-wintun0",
              "packet attach should use serial-tail device config") &&
       ok;

  session.stop();
  return ok;
}

bool test_native_session_stop_disconnects_adopted_handshake_before_attach() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();

  exv::vpn_engine::NativeVpnEngineDependencies handshake_deps;
  handshake_deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };

  exv::vpn_engine::NativeHandshakeResult prepared;
  exv::vpn_engine::NativeHandshakeJob job(engine_config(), handshake_deps);
  const auto prepared_result = job.run(std::stop_token{}, &prepared);

  ok = expect(prepared_result.ok, "prepared handshake should succeed") && ok;
  ok = expect(transport->disconnect_count == 0,
              "prepared handshake should still be connected before adoption") &&
       ok;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config());
  const auto adopted = session.adopt_handshake(std::move(prepared));
  ok = expect(adopted.ok, "adopting prepared handshake should succeed") && ok;

  session.stop();
  ok = expect(transport->disconnect_count == 1,
              "stop should explicitly disconnect adopted handshake transport") &&
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

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = []() {
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  exv::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.disable_dtls = false; // intentionally set the "wrong" flag

  exv::vpn_engine::NativeVpnEngineSession session(cfg, deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

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

bool test_dtls_enabled_emits_unavailable_and_continues_cstp() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  auto device = std::make_shared<PacketDeviceState>();
  RecordingEventSink events;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(
        device,
        std::vector<std::vector<std::uint8_t>>{{0x45, 0x00, 0x00, 0x2a}});
  };
  deps.event_sink = &events;

  exv::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.disable_dtls = false;

  exv::vpn_engine::NativeVpnEngineSession session(cfg, deps);
  const exv::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok,
              "DTLS-enabled native start should continue over CSTP") &&
       ok;
  ok = expect(events.contains("dtls.unavailable"),
              "DTLS-enabled pre-A14 start should emit dtls.unavailable") &&
       ok;
  ok = expect(events.contains_field("dtls.unavailable", "code",
                                    "dtls_unavailable"),
              "DTLS unavailable event should carry stable code") &&
       ok;
  ok = expect(events.contains("packet.loop.started"),
              "DTLS unavailable must not block packet loop startup") &&
       ok;

  session.stop();
  return ok;
}

bool test_auth_failure_maps_error_without_device() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->auth_ok = false;
  int packet_devices_created = 0;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

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

bool test_auth_challenge_emits_interaction_event_without_device() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->auth_ok = false;
  transport->auth_error_code = "auth_challenge_required";
  transport->auth_error_message = "verification required";
  transport->auth_prompt_label = "Verification code";
  transport->auth_prompt_type = "password";
  RecordingEventSink events;
  int packet_devices_created = 0;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };
  deps.event_sink = &events;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(!started.ok,
              "auth challenge should stop native start until UI response path exists") &&
       ok;
  ok = expect(started.code == "auth_challenge_required",
              "auth challenge should preserve stable error code") &&
       ok;
  ok = expect(status.error_code == "auth_challenge_required",
              "status should retain auth challenge code") &&
       ok;
  ok = expect(events.contains("auth.challenge_required"),
              "auth challenge should emit interaction event") &&
       ok;
  ok = expect(events.contains_field("auth.challenge_required", "label",
                                    "Verification code"),
              "challenge event should include prompt label") &&
       ok;
  ok = expect(events.contains_field("auth.challenge_required", "input_type",
                                    "password"),
              "challenge event should include prompt input type") &&
       ok;
  ok = expect(packet_devices_created == 0,
              "auth challenge must not create packet device") &&
       ok;

  return ok;
}

bool test_csd_required_emits_unsupported_event_without_device() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->auth_ok = false;
  transport->auth_error_code = "csd_required_unsupported";
  transport->auth_error_message = "AnyConnect host-scan is required";
  RecordingEventSink events;
  int packet_devices_created = 0;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };
  deps.event_sink = &events;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

  ok = expect(!started.ok,
              "host-scan requirement should stop native start") &&
       ok;
  ok = expect(started.code == "csd_required_unsupported",
              "host-scan requirement should preserve stable error code") &&
       ok;
  ok = expect(status.error_code == "csd_required_unsupported",
              "status should retain host-scan unsupported code") &&
       ok;
  ok = expect(events.contains("csd.required_unsupported"),
              "host-scan requirement should emit CSD unsupported event") &&
       ok;
  ok = expect(events.contains_field("csd.required_unsupported", "code",
                                    "csd_required_unsupported"),
              "CSD unsupported event should carry stable code") &&
       ok;
  ok = expect(events.contains("auth.failed"),
              "host-scan requirement should still emit auth.failed") &&
       ok;
  ok = expect(packet_devices_created == 0,
              "host-scan requirement must not create packet device") &&
       ok;

  return ok;
}

bool test_cstp_failure_maps_error_without_device_open() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->cstp_ok = false;
  int packet_devices_created = 0;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&packet_devices_created]() {
    ++packet_devices_created;
    return make_scripted_device(std::make_shared<PacketDeviceState>());
  };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

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

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_failing_open_device(device);
  };
  deps.event_sink = &events;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();
  const exv::vpn_engine::VpnEngineStatus status = session.status();

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

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_polling_device(device);
  };

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();

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
  const exv::vpn_engine::VpnEngineStatus status = session.status();

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

bool test_gateway_keepalive_metadata_drives_native_session_timer() {
  bool ok = true;

  auto transport = std::make_shared<FakeProtocolTransport::State>();
  transport->keepalive_seconds = 1;
  auto device = std::make_shared<PacketDeviceState>();
  RecordingEventSink events;

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_polling_device(device);
  };
  deps.protocol_options_configurator =
      [](exv::vpn_engine::protocol::ProtocolSessionOptions *options) {
        if (options)
          options->liveness_idle_polls_per_second = 2;
      };
  deps.event_sink = &events;

  exv::vpn_engine::NativeVpnEngineSession session(engine_config(), deps);
  const exv::vpn_engine::ValidationResult started = session.start();

  ok = expect(started.ok,
              "native start should succeed before keepalive metadata test") &&
       ok;
  ok = expect(wait_until([&events]() { return events.contains("keepalive.sent"); },
                         std::chrono::milliseconds(1500)),
              "gateway keepalive metadata should emit keepalive.sent") &&
       ok;
  ok = expect(transport->keepalive_control_count.load() >= 1,
              "gateway keepalive metadata should send a keepalive control frame") &&
       ok;

  session.stop();
  return ok;
}

bool test_default_dependencies_expose_factories() {
  bool ok = true;

  exv::vpn_engine::NativeVpnEngineDependencies deps =
      exv::vpn_engine::default_native_engine_dependencies();

  ok = expect(static_cast<bool>(deps.transport_factory),
              "default native dependencies should expose a transport factory") &&
       ok;
  ok = expect(static_cast<bool>(deps.packet_device_factory),
              "default native dependencies should expose a packet device factory") &&
       ok;

  if (deps.transport_factory) {
    std::unique_ptr<exv::vpn_engine::protocol::ProtocolTransport> transport =
        deps.transport_factory();
    ok = expect(static_cast<bool>(transport),
                "default transport factory should create a transport") &&
         ok;
  }

  if (deps.packet_device_factory) {
    std::unique_ptr<exv::vpn_engine::PacketDevice> device =
        deps.packet_device_factory();
    ok = expect(static_cast<bool>(device),
                "default packet device factory should create a packet device") &&
         ok;
  }

  return ok;
}

bool test_default_tls_only_start_does_not_report_missing_transport_factory() {
  bool ok = true;

  exv::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.server = "https://127.0.0.1:1";

  exv::vpn_engine::NativeVpnEngineSession session(cfg);
  const exv::vpn_engine::ValidationResult started = session.start();

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
    std::unique_ptr<exv::vpn_engine::protocol::TlsStream> stream(
        new LifetimeTlsStream(state));
    exv::vpn_engine::protocol::ProductionProtocolTransport transport(
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

  exv::vpn_engine::NativeVpnEngineDependencies deps;
  deps.transport_factory = [&transport]() {
    return make_fake_transport(transport);
  };
  deps.packet_device_factory = [&device]() {
    return make_scripted_device(device, std::vector<std::vector<std::uint8_t>>{});
  };
  deps.event_sink = &events;

  exv::vpn_engine::VpnEngineConfig cfg = engine_config();
  cfg.mtu = 1290; // documented safe default

  exv::vpn_engine::NativeVpnEngineSession session(cfg, deps);
  const exv::vpn_engine::ValidationResult started = session.start();

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
  static_assert(std::is_abstract_v<exv::vpn_engine::PacketDevice>,
                "PacketDevice must remain an abstract interface");

  bool ok = true;

  // PacketDevice contract: header must compile without platform headers.
  {
    MockPacketDevice dev;

    exv::vpn_engine::TunnelMetadata meta;
    exv::vpn_engine::ValidationResult o = dev.open(meta);
    ok = expect(o.ok, "mock open should succeed") && ok;
    ok = expect(dev.opened(), "mock open should update state") && ok;

    std::vector<std::uint8_t> write_pkt = {1, 2, 3};
    exv::vpn_engine::ValidationResult w = dev.write_packet(write_pkt);
    ok = expect(w.ok, "mock write_packet should succeed") && ok;
    ok = expect(dev.last_written() == write_pkt,
                "mock write_packet should capture packet bytes") &&
         ok;

    dev.push_read_packet({9, 8});
    std::vector<std::uint8_t> read_pkt;
    exv::vpn_engine::ValidationResult r = dev.read_packet(&read_pkt);
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

  exv::vpn_engine::VpnEngineConfig engine_cfg =
      engine_config();
  exv::vpn_engine::ValidationResult validation =
      exv::vpn_engine::validate_native_config(engine_cfg);
  ok = expect(validation.ok,
              "native engine should accept complete engine config") &&
       ok;

  engine_cfg.password.clear();
  validation = exv::vpn_engine::validate_native_config(engine_cfg);
  ok = expect(!validation.ok && validation.code == "config_invalid",
              "native engine should reject engine config without password") &&
       ok;

  engine_cfg = engine_config();

  exv::vpn_engine::VpnEngineEvent event;
  event.type = "auth";
  event.level = "info";
  event.message = "连接成功";
  event.fields["stage"] = "cookie";
  nlohmann::json event_json = exv::vpn_engine::event_to_json(event);
  ok = expect(event_json.value("message", std::string()) == "连接成功",
              "native engine events should preserve UTF-8 text") &&
       ok;
  ok = expect(event_json["fields"].value("stage", std::string()) == "cookie",
              "native engine events should expose structured fields") &&
       ok;

  exv::vpn_engine::NativeVpnEngineSession session(engine_cfg);
  exv::vpn_engine::VpnEngineStatus status = session.status();
  ok = expect(!status.running && !status.network_ready && status.pid == -1,
              "new native session should not report an openconnect process") &&
       ok;

  ok = test_injected_fake_start_runs_packet_loop_and_cleans_up() && ok;
  ok = test_network_configurator_runs_before_packet_open() && ok;
  ok = test_native_session_splits_handshake_from_packet_attach() && ok;
  ok = test_native_session_adopts_prepared_handshake_without_reauth() && ok;
  ok = test_native_session_stop_disconnects_adopted_handshake_before_attach() && ok;
  ok = test_dtls_config_flag_does_not_block_native_engine() && ok;
  ok = test_dtls_enabled_emits_unavailable_and_continues_cstp() && ok;
  ok = test_auth_failure_maps_error_without_device() && ok;
  ok = test_auth_challenge_emits_interaction_event_without_device() && ok;
  ok = test_csd_required_emits_unsupported_event_without_device() && ok;
  ok = test_cstp_failure_maps_error_without_device_open() && ok;
  ok = test_packet_loop_start_failure_emits_native_start_failed() && ok;
  ok = test_stop_cancels_packet_loop_and_closes_device() && ok;
  ok = test_gateway_keepalive_metadata_drives_native_session_timer() && ok;
  ok = test_default_dependencies_expose_factories() && ok;
  ok = test_default_tls_only_start_does_not_report_missing_transport_factory() &&
       ok;
  ok = test_production_transport_can_own_tls_stream() && ok;
  ok = test_invalid_metadata_mtu_falls_back_to_safe_default() && ok;

  return ok ? 0 : 1;
}
