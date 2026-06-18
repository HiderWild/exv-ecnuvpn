#include "support/fake_anyconnect_server.hpp"

#include "vpn_engine/protocol/session.hpp"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <functional>
#include <iostream>
#include <mutex>
#include <string>
#include <vector>

namespace {

static const char *MOCK_PASSWORD = "test-mock-password-placeholder";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::vector<std::uint8_t> bytes(std::initializer_list<std::uint8_t> values) {
  return std::vector<std::uint8_t>(values.begin(), values.end());
}

bool contains_event(
    const ecnuvpn::tests::support::RecordingEventSink &events,
    const std::string &type) {
  for (const auto &event : events.events()) {
    if (event.type == type)
      return true;
  }
  return false;
}

bool events_contain_password(
    const ecnuvpn::tests::support::RecordingEventSink &events,
    const std::string &password) {
  for (const auto &event : events.events()) {
    if (event.message.find(password) != std::string::npos)
      return true;
    for (const auto &field : event.fields) {
      if (field.first.find(password) != std::string::npos ||
          field.second.find(password) != std::string::npos) {
        return true;
      }
    }
  }
  return false;
}

class FakeProtocolTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  explicit FakeProtocolTransport(
      ecnuvpn::tests::support::FakeAnyConnectServer &server)
      : server_(server) {}

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          &options) override {
    ecnuvpn::tests::support::FakeAnyConnectCredentials credentials;
    credentials.username = options.username;
    credentials.password = options.password;
    return server_.password_authenticate(credentials);
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               ecnuvpn::vpn_engine::TunnelMetadata *metadata) override {
    return server_.connect_cstp(cookie, metadata);
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) override {
    return server_.send_packet(packet);
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind kind) override {
    return server_.send_control(kind);
  }

  ecnuvpn::vpn_engine::ValidationResult
  receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame *out) override {
    return server_.receive_frame(out);
  }

  void disconnect() override { server_.close_transport(); }

  void reset_for_reconnect() override { server_.reset_transport(); }

private:
  ecnuvpn::tests::support::FakeAnyConnectServer &server_;
};

class ManualCancellationToken final
    : public ecnuvpn::vpn_engine::protocol::CancellationToken {
public:
  bool is_cancelled() const override { return cancelled_; }
  void cancel() { cancelled_ = true; }

private:
  bool cancelled_ = false;
};

class NoDataCancellingPacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  NoDataCancellingPacketDevice(ManualCancellationToken &cancel,
                               int cancel_after_reads)
      : cancel_(cancel), cancel_after_reads_(cancel_after_reads) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    last_open_metadata_.interface_name = config.interface_name;
    last_open_metadata_.mtu = config.mtu;
    open_ = true;
    ++open_count_;
    return {};
  }
  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    last_open_metadata_ = metadata;
    metadata_open_used_ = true;
    open_ = true;
    ++open_count_;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    if (!open_)
      return {false, "packet_device_closed", "packet device is closed"};

    packet->clear();
    ++read_count_;
    if (read_count_ >= cancel_after_reads_)
      cancel_.cancel();
    return {false, "no_data", "no packet available"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {false, "unexpected_write", "no packets should be written"};
  }

  void close() override {
    if (open_) {
      open_ = false;
      ++close_count_;
    }
  }

  int open_count() const { return open_count_; }
  int close_count() const { return close_count_; }
  int read_count() const { return read_count_; }
  const ecnuvpn::vpn_engine::TunnelMetadata &last_open_metadata() const {
    return last_open_metadata_;
  }
  bool metadata_open_used() const { return metadata_open_used_; }

private:
  ManualCancellationToken &cancel_;
  int cancel_after_reads_ = 0;
  ecnuvpn::vpn_engine::TunnelMetadata last_open_metadata_;
  bool metadata_open_used_ = false;
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
  int read_count_ = 0;
};

// Scriptable, thread-safe transport used to exercise the P3 liveness paths
// (DPD servicing, keepalive emission, dead-peer detection, reconnect) without
// the echo server. Inbound frames are injected explicitly; outbound control
// frames are recorded; data sends are optionally echoed back as inbound data.
class LivenessTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  explicit LivenessTransport(bool echo_data = false) : echo_data_(echo_data) {
    metadata_.interface_name = "fake-cstp0";
    metadata_.internal_ip4_address = "10.0.0.2";
    metadata_.mtu = 1290;
  }

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          & /*options*/) override {
    std::lock_guard<std::mutex> lock(mu_);
    ++auth_attempts_;
    ecnuvpn::vpn_engine::protocol::AuthResult result;
    result.ok = true;
    result.cookie = "liveness-cookie";
    return result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               ecnuvpn::vpn_engine::TunnelMetadata *metadata) override {
    std::lock_guard<std::mutex> lock(mu_);
    ++cstp_connects_;
    last_connect_cookies_.push_back(cookie);
    if (auth_expired_connects_ > 0) {
      --auth_expired_connects_;
      return {false, "auth_expired", "cached cookie expired"};
    }
    if (metadata) {
      *metadata = metadata_;
    }
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) override {
    std::lock_guard<std::mutex> lock(mu_);
    if (closed_)
      return {false, "transport_closed", "transport closed"};
    ++data_sends_;
    if (echo_data_) {
      ecnuvpn::vpn_engine::protocol::InboundFrame frame;
      frame.kind = ecnuvpn::vpn_engine::protocol::InboundFrameKind::data;
      frame.payload = packet;
      inbound_.push_back(std::move(frame));
      cv_.notify_all();
    }
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind kind) override {
    std::lock_guard<std::mutex> lock(mu_);
    if (closed_)
      return {false, "transport_closed", "transport closed"};
    control_sends_.push_back(kind);
    cv_.notify_all();
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  receive_frame(ecnuvpn::vpn_engine::protocol::InboundFrame *out) override {
    std::unique_lock<std::mutex> lock(mu_);
    cv_.wait(lock, [&] { return !inbound_.empty() || closed_; });
    if (!inbound_.empty()) {
      *out = std::move(inbound_.front());
      inbound_.pop_front();
      return {};
    }
    return {false, "transport_closed", "transport closed"};
  }

  void disconnect() override {
    std::lock_guard<std::mutex> lock(mu_);
    closed_ = true;
    ++disconnect_count_;
    cv_.notify_all();
  }

  void reset_for_reconnect() override {
    std::lock_guard<std::mutex> lock(mu_);
    closed_ = false;
    inbound_.clear();
    ++reset_count_;
    cv_.notify_all();
  }

  void inject(ecnuvpn::vpn_engine::protocol::InboundFrameKind kind,
              std::vector<std::uint8_t> payload = {}) {
    std::lock_guard<std::mutex> lock(mu_);
    ecnuvpn::vpn_engine::protocol::InboundFrame frame;
    frame.kind = kind;
    frame.payload = std::move(payload);
    inbound_.push_back(std::move(frame));
    cv_.notify_all();
  }

  void set_connect_metadata(ecnuvpn::vpn_engine::TunnelMetadata metadata) {
    std::lock_guard<std::mutex> lock(mu_);
    metadata_ = std::move(metadata);
  }

  void expire_next_connect_cookie() {
    std::lock_guard<std::mutex> lock(mu_);
    ++auth_expired_connects_;
  }

  int auth_attempts() const {
    std::lock_guard<std::mutex> lock(mu_);
    return auth_attempts_;
  }

  int cstp_connects() const {
    std::lock_guard<std::mutex> lock(mu_);
    return cstp_connects_;
  }

  int control_send_count(
      ecnuvpn::vpn_engine::protocol::InboundFrameKind kind) const {
    std::lock_guard<std::mutex> lock(mu_);
    int count = 0;
    for (const auto &sent : control_sends_) {
      if (sent == kind)
        ++count;
    }
    return count;
  }

private:
  mutable std::mutex mu_;
  std::condition_variable cv_;
  std::deque<ecnuvpn::vpn_engine::protocol::InboundFrame> inbound_;
  std::vector<ecnuvpn::vpn_engine::protocol::InboundFrameKind> control_sends_;
  ecnuvpn::vpn_engine::TunnelMetadata metadata_;
  std::vector<std::string> last_connect_cookies_;
  bool echo_data_ = false;
  bool closed_ = false;
  int auth_expired_connects_ = 0;
  int auth_attempts_ = 0;
  int cstp_connects_ = 0;
  int data_sends_ = 0;
  int disconnect_count_ = 0;
  int reset_count_ = 0;
};

// Returns no_data until a predicate signals the loop should end, then reports a
// clean packet-loop end. Used to gate test shutdown on observable transport
// state so the assertions are deterministic rather than timing-based.
class PredicateDrainDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  explicit PredicateDrainDevice(std::function<bool()> end_when)
      : end_when_(std::move(end_when)) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    last_open_metadata_.interface_name = config.interface_name;
    last_open_metadata_.mtu = config.mtu;
    open_ = true;
    ++open_count_;
    return {};
  }
  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    last_open_metadata_ = metadata;
    open_ = true;
    ++open_count_;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    packet->clear();
    if (end_when_ && end_when_())
      return {false, "packet_device_empty", "packet device drained"};
    return {false, "no_data", "no packet available"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    written_packets_.push_back(packet);
    return {};
  }

  void close() override {
    if (open_) {
      open_ = false;
      ++close_count_;
    }
  }

  int open_count() const { return open_count_; }
  int close_count() const { return close_count_; }

private:
  std::function<bool()> end_when_;
  ecnuvpn::vpn_engine::TunnelMetadata last_open_metadata_;
  std::vector<std::vector<std::uint8_t>> written_packets_;
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
};

// First session (open #1) is permanently silent (no_data) to trigger dead-peer
// detection; from the second session onward it emits a single packet and then
// drains cleanly, letting the reconnected session finish gracefully.
class DeadPeerThenDrainDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig &config) override {
    last_open_metadata_.interface_name = config.interface_name;
    last_open_metadata_.mtu = config.mtu;
    open_ = true;
    ++open_count_;
    session_read_index_ = 0;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    last_open_metadata_ = metadata;
    open_ = true;
    ++open_count_;
    session_read_index_ = 0;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    packet->clear();
    if (open_count_ <= 1)
      return {false, "no_data", "no packet available"};
    if (session_read_index_ == 0) {
      ++session_read_index_;
      *packet = {0x45, 0x00, 0x00, 0x21};
      return {};
    }
    return {false, "packet_device_empty", "packet device drained"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override {
    written_packets_.push_back(packet);
    return {};
  }

  void close() override {
    if (open_) {
      open_ = false;
      ++close_count_;
    }
  }

  int open_count() const { return open_count_; }
  const ecnuvpn::vpn_engine::TunnelMetadata &last_open_metadata() const {
    return last_open_metadata_;
  }

private:
  ecnuvpn::vpn_engine::TunnelMetadata last_open_metadata_;
  std::vector<std::vector<std::uint8_t>> written_packets_;
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
  int session_read_index_ = 0;
};

// Always reports no_data; never drains and never cancels. Lets every forwarding
// session reach dead-peer detection so reconnect exhaustion can be exercised.
class SilentNoDataDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::DeviceConfig & /*config*/) override {
    open_ = true;
    ++open_count_;
    return {};
  }
  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata & /*metadata*/) override {
    open_ = true;
    ++open_count_;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    packet->clear();
    return {false, "no_data", "no packet available"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {};
  }

  void close() override {
    if (open_) {
      open_ = false;
      ++close_count_;
    }
  }

  int open_count() const { return open_count_; }
  int close_count() const { return close_count_; }

private:
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
};

ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions session_options() {
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions options;
  options.server.scheme = "https";
  options.server.host = "vpn.example.invalid";
  options.server.port = 443;
  options.server.base_path = "/";
  options.username = "alice";
  options.password = MOCK_PASSWORD;
  options.useragent = "ECNU-VPN test";
  options.disable_dtls = true;
  return options;
}

bool test_authenticate_success_and_connect_cstp() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  ProtocolSession session(session_options(), &transport);

  auto auth = session.authenticate();
  ok = expect(auth.ok, "auth success should return ok") && ok;
  ok = expect(server.auth_attempts() == 1,
              "auth success should call transport authenticate once") &&
       ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto cstp = session.connect_cstp(&metadata);
  ok = expect(cstp.ok, "CSTP connect should return ok after auth") && ok;
  ok = expect(metadata.interface_name == "fake-cstp0",
              "CSTP connect should return tunnel metadata") &&
       ok;
  ok = expect(session.state().phase ==
                  ecnuvpn::vpn_engine::SessionPhase::configuring_network,
              "CSTP connect should configure session state") &&
       ok;

  return ok;
}

bool test_auth_failure_never_reconnects() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);

  auto options = session_options();
  options.password = "test-mock-wrong-password";
  options.auto_reconnect = true;
  options.max_reconnects = 3;

  ProtocolSession session(options, &transport);
  auto auth = session.authenticate();

  ok = expect(!auth.ok, "bad credentials should fail auth") && ok;
  ok = expect(auth.code == "auth_failed",
              "bad credentials should return auth_failed") &&
       ok;
  ok = expect(server.auth_attempts() == 1,
              "auth failure should not retry or reconnect") &&
       ok;
  ok = expect(server.cstp_connects() == 0,
              "auth failure should not advance to CSTP") &&
       ok;
  ok = expect(session.reconnect_attempts() == 0,
              "auth failure should not increment reconnect attempts") &&
       ok;

  return ok;
}

bool test_packet_echo() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x45, 0x00, 0x00, 0x2a})});
  ManualCancellationToken cancel;

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before packet loop") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP connect should succeed before packet loop") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok, "packet echo loop should finish cleanly") && ok;
  ok = expect(session.state().network_ready(),
              "packet loop should mark network ready") &&
       ok;
  ok = expect(device.open_count() == 1,
              "packet loop should open the packet device") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] ==
                      bytes({0x45, 0x00, 0x00, 0x2a}),
              "packet loop should echo packet bytes to device") &&
       ok;
  ok = expect(contains_event(events, "packet.inbound"),
              "packet loop should emit packet.inbound") &&
       ok;
  ok = expect(!events_contain_password(events, MOCK_PASSWORD),
              "event stream must not include password") &&
       ok;

  return ok;
}

bool test_reconnect_after_transport_close() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;

  FakeAnyConnectServerOptions server_options;
  server_options.close_on_data_frame_number = 1;
  server_options.close_only_once = true;

  FakeAnyConnectServer server(server_options);
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device(
      {bytes({0xaa, 0xbb, 0xcc}), bytes({0x45, 0x11, 0x22, 0x33})});
  ManualCancellationToken cancel;

  auto options = session_options();
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "initial auth should succeed") && ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "initial CSTP connect should succeed") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok, "reconnect loop should finish cleanly") && ok;
  ok = expect(session.reconnect_attempts() == 1,
              "one reconnect should be attempted") &&
       ok;
  ok = expect(server.auth_attempts() == 1,
              "reconnect should reuse cached cookie before re-authentication") &&
       ok;
  ok = expect(server.cstp_connects() == 2,
              "reconnect should repeat CSTP connect") &&
       ok;
  ok = expect(device.open_count() == 2,
              "reconnect should reopen packet device") &&
       ok;
  ok = expect(device.close_count() == 2,
              "each forwarding session should close the packet device") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] ==
                      bytes({0x45, 0x11, 0x22, 0x33}),
              "post-reconnect packet should be echoed") &&
       ok;
  ok = expect(contains_event(events, "reconnect_started"),
              "reconnect should emit reconnect_started") &&
       ok;
  ok = expect(contains_event(events, "reconnect.cookie_reused"),
              "reconnect should emit cached cookie reuse") &&
       ok;
  ok = expect(contains_event(events, "reconnect_succeeded"),
              "reconnect should emit reconnect_succeeded") &&
       ok;

  return ok;
}

bool test_cancellation_exits_without_opening_device() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x01})});
  ManualCancellationToken cancel;
  cancel.cancel();

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before cancellation") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before cancellation") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "cancelled loop should return session_cancelled") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "cancelled loop should mark session stopped") &&
       ok;
  ok = expect(device.open_count() == 0,
              "cancelled loop should not open packet device") &&
       ok;
  ok = expect(device.written_packets().empty(),
              "cancelled loop should not write packets") &&
       ok;

  return ok;
}

bool test_active_loop_cancellation_during_no_data_poll_exits() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ManualCancellationToken cancel;
  NoDataCancellingPacketDevice device(cancel, 3);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before active cancellation") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before active cancellation") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "active cancelled loop should return session_cancelled") &&
       ok;
  ok = expect(device.open_count() == 1,
              "active loop should open the packet device") &&
       ok;
  ok = expect(device.read_count() >= 3,
              "active loop should retry no_data reads until cancellation") &&
       ok;
  ok = expect(device.close_count() == 1,
              "active cancellation should close the packet device") &&
       ok;
  ok = expect(device.last_open_metadata().interface_name == "fake-cstp0",
              "active loop should pass tunnel interface name to packet device") &&
       ok;
  ok = expect(device.last_open_metadata().routes.empty() &&
                  device.last_open_metadata().server_bypass_ips.empty(),
              "active loop should not pass route ownership into packet device") &&
       ok;
  ok = expect(!device.metadata_open_used(),
              "active loop should open packet device with DeviceConfig") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "active cancellation should mark session stopped") &&
       ok;

  return ok;
}

bool test_disconnect_stops_before_packet_loop() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x01})});
  ManualCancellationToken cancel;

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before disconnect") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before disconnect") &&
       ok;

  session.disconnect();
  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "disconnected loop should return session_cancelled") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "disconnect should mark session stopped") &&
       ok;
  ok = expect(device.open_count() == 0,
              "disconnect before loop should not open packet device") &&
       ok;

  return ok;
}

bool test_inbound_dpd_request_is_answered() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  PredicateDrainDevice device([&] {
    return transport.control_send_count(InboundFrameKind::dpd_response) > 0;
  });
  ManualCancellationToken cancel;

  transport.inject(InboundFrameKind::dpd_request);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before DPD test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before DPD test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok, "DPD servicing loop should finish cleanly") && ok;
  ok = expect(transport.control_send_count(InboundFrameKind::dpd_response) == 1,
              "inbound DPD request should be answered with a DPD response") &&
       ok;
  ok = expect(contains_event(events, "dpd.responded"),
              "DPD servicing should emit dpd.responded") &&
       ok;

  return ok;
}

bool test_peer_disconnect_is_not_generic_transport_eof() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  transport.inject(InboundFrameKind::disconnect);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before disconnect frame test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before disconnect frame test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "tunnel_disconnected",
              "peer disconnect frame should not be generic transport_closed") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::failed,
              "peer disconnect should mark session failed") &&
       ok;

  return ok;
}

bool test_terminate_frame_is_not_generic_transport_eof() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  transport.inject(InboundFrameKind::terminate);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before terminate frame test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before terminate frame test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "tunnel_disconnected",
              "terminate frame should not be generic transport_closed") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::failed,
              "terminate frame should mark session failed") &&
       ok;

  return ok;
}

bool test_compressed_frame_fails_with_explicit_unsupported_code() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  transport.inject(InboundFrameKind::compressed);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before compressed frame test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before compressed frame test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "cstp_compressed_unsupported",
              "compressed frame should fail with explicit unsupported code") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::failed,
              "compressed frame should mark session failed") &&
       ok;

  return ok;
}

bool test_idle_keepalive_is_emitted() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  ManualCancellationToken cancel;
  NoDataCancellingPacketDevice device(cancel, 5);

  auto options = session_options();
  options.keepalive_idle_poll_interval = 2;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before keepalive test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before keepalive test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "keepalive loop should end via cancellation") &&
       ok;
  ok = expect(transport.control_send_count(InboundFrameKind::keepalive) >= 1,
              "idle loop should emit at least one keepalive") &&
       ok;
  ok = expect(contains_event(events, "keepalive.sent"),
              "idle keepalive should emit keepalive.sent") &&
       ok;

  return ok;
}

bool test_metadata_keepalive_drives_idle_timer() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  ManualCancellationToken cancel;
  NoDataCancellingPacketDevice device(cancel, 5);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.keepalive_seconds = 1;
  transport.set_connect_metadata(metadata);

  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before metadata keepalive test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before metadata keepalive test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "metadata keepalive loop should end via cancellation") &&
       ok;
  ok = expect(transport.control_send_count(InboundFrameKind::keepalive) >= 1,
              "metadata keepalive should schedule control keepalive") &&
       ok;
  ok = expect(contains_event(events, "keepalive.sent"),
              "metadata keepalive should emit keepalive.sent") &&
       ok;

  return ok;
}

bool test_dead_peer_triggers_reconnect() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/true);
  RecordingEventSink events;
  DeadPeerThenDrainDevice device;
  ManualCancellationToken cancel;

  auto options = session_options();
  options.dpd_idle_poll_interval = 2;
  options.dead_peer_poll_budget = 3;
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before dead-peer test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before dead-peer test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok,
              "dead-peer reconnect loop should recover and finish cleanly") &&
       ok;
  ok = expect(session.reconnect_attempts() == 1,
              "dead peer should trigger exactly one reconnect") &&
       ok;
  ok = expect(transport.auth_attempts() == 1,
              "dead-peer reconnect should reuse cached cookie") &&
       ok;
  ok = expect(transport.control_send_count(InboundFrameKind::dpd_request) >= 1,
              "idle loop should send a DPD probe before declaring dead peer") &&
       ok;
  ok = expect(contains_event(events, "dpd.dead"),
              "dead peer detection should emit dpd.dead") &&
       ok;
  ok = expect(device.open_count() == 2,
              "dead-peer reconnect should reopen the packet device") &&
       ok;
  ok = expect(contains_event(events, "reconnect_started"),
              "dead-peer reconnect should emit reconnect_started") &&
       ok;
  ok = expect(contains_event(events, "reconnect.cookie_reused"),
              "dead-peer reconnect should emit cached cookie reuse") &&
       ok;
  ok = expect(contains_event(events, "reconnect_succeeded"),
              "dead-peer reconnect should emit reconnect_succeeded") &&
       ok;

  return ok;
}

bool test_reconnect_reauthenticates_when_cached_cookie_expires() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/true);
  RecordingEventSink events;
  DeadPeerThenDrainDevice device;
  ManualCancellationToken cancel;

  auto options = session_options();
  options.dpd_idle_poll_interval = 2;
  options.dead_peer_poll_budget = 3;
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before cookie-expiry reconnect test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before cookie-expiry reconnect test") &&
       ok;

  transport.expire_next_connect_cookie();
  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok,
              "cookie-expiry reconnect should re-authenticate and recover") &&
       ok;
  ok = expect(session.reconnect_attempts() == 1,
              "cookie expiry should still use one reconnect attempt") &&
       ok;
  ok = expect(transport.auth_attempts() == 2,
              "expired cached cookie should trigger one full re-authentication") &&
       ok;
  ok = expect(transport.cstp_connects() == 3,
              "expired cached cookie should attempt cached CSTP then full-auth CSTP") &&
       ok;
  ok = expect(contains_event(events, "reconnect.cookie_expired"),
              "cookie-expiry reconnect should emit expiration event") &&
       ok;
  ok = expect(contains_event(events, "reconnect_succeeded"),
              "cookie-expiry reconnect should still succeed") &&
       ok;

  return ok;
}

bool test_reconnect_skips_cached_cookie_after_session_timeout() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/true);
  RecordingEventSink events;
  DeadPeerThenDrainDevice device;
  ManualCancellationToken cancel;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.session_timeout_seconds = 60;
  transport.set_connect_metadata(metadata);

  std::chrono::steady_clock::time_point now{};
  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;
  options.dpd_idle_poll_interval = 2;
  options.dead_peer_poll_budget = 3;
  options.auto_reconnect = true;
  options.max_reconnects = 1;
  options.monotonic_clock = [&now]() { return now; };

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before session-timeout reconnect test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before session-timeout reconnect test") &&
       ok;

  now += std::chrono::seconds(61);
  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok,
              "session-timeout reconnect should re-authenticate and recover") &&
       ok;
  ok = expect(session.reconnect_attempts() == 1,
              "session-timeout reconnect should use one reconnect attempt") &&
       ok;
  ok = expect(transport.auth_attempts() == 2,
              "elapsed session timeout should force one full re-authentication") &&
       ok;
  ok = expect(transport.cstp_connects() == 2,
              "elapsed session timeout should skip cached-cookie CSTP attempt") &&
       ok;
  ok = expect(contains_event(events, "reconnect.cookie_expired"),
              "session-timeout reconnect should emit cookie expiry event") &&
       ok;
  ok = expect(!contains_event(events, "reconnect.cookie_reused"),
              "session-timeout reconnect should not report cookie reuse") &&
       ok;

  return ok;
}

bool test_reconnect_reopens_device_with_updated_metadata() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/true);
  RecordingEventSink events;
  DeadPeerThenDrainDevice device;
  ManualCancellationToken cancel;

  auto options = session_options();
  options.dpd_idle_poll_interval = 2;
  options.dead_peer_poll_budget = 3;
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before metadata-refresh reconnect test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before metadata-refresh reconnect test") &&
       ok;

  ecnuvpn::vpn_engine::TunnelMetadata refreshed;
  refreshed.interface_name = "fake-cstp1";
  refreshed.internal_ip4_address = "10.0.0.3";
  refreshed.mtu = 1400;
  transport.set_connect_metadata(refreshed);

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok,
              "metadata-refresh reconnect should recover and finish cleanly") &&
       ok;
  ok = expect(device.open_count() == 2,
              "metadata-refresh reconnect should reopen the packet device") &&
       ok;
  ok = expect(device.last_open_metadata().interface_name == "fake-cstp1",
              "reconnect should use refreshed interface metadata") &&
       ok;
  ok = expect(device.last_open_metadata().mtu == 1400,
              "reconnect should use refreshed MTU metadata") &&
       ok;

  return ok;
}

bool test_metadata_new_tunnel_rekey_triggers_reconnect() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/true);
  RecordingEventSink events;
  DeadPeerThenDrainDevice device;
  ManualCancellationToken cancel;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.rekey_seconds = 1;
  metadata.rekey_method = "new-tunnel";
  transport.set_connect_metadata(metadata);

  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before metadata rekey test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before metadata rekey test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok,
              "new-tunnel rekey should reconnect and finish cleanly") &&
       ok;
  ok = expect(session.reconnect_attempts() == 1,
              "new-tunnel rekey should use the existing reconnect path") &&
       ok;
  ok = expect(contains_event(events, "rekey.due"),
              "new-tunnel rekey should emit rekey.due") &&
       ok;

  return ok;
}

bool test_metadata_ssl_rekey_reports_unsupported_when_reconnect_disabled() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.rekey_seconds = 1;
  metadata.rekey_method = "ssl";
  transport.set_connect_metadata(metadata);

  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;
  options.auto_reconnect = false;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before SSL rekey test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before SSL rekey test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "rekey_unsupported",
              "SSL rekey should report explicit unsupported code") &&
       ok;
  ok = expect(contains_event(events, "rekey.due"),
              "SSL rekey should still emit rekey.due") &&
       ok;

  return ok;
}

bool test_metadata_idle_timeout_has_specific_code() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.idle_timeout_seconds = 1;
  transport.set_connect_metadata(metadata);

  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before idle timeout test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before idle timeout test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "idle_timeout",
              "idle timeout should report explicit idle_timeout code") &&
       ok;

  return ok;
}

bool test_metadata_session_timeout_has_specific_code() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.internal_ip4_address = "10.0.0.2";
  metadata.mtu = 1290;
  metadata.session_timeout_seconds = 1;
  transport.set_connect_metadata(metadata);

  auto options = session_options();
  options.liveness_idle_polls_per_second = 2;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata connected;
  ok = expect(session.authenticate().ok,
              "auth should succeed before session timeout test") &&
       ok;
  ok = expect(session.connect_cstp(&connected).ok,
              "CSTP should succeed before session timeout test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_timeout",
              "session timeout should report explicit session_timeout code") &&
       ok;

  return ok;
}

bool test_reconnect_exhaustion_fails_with_stable_code() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  LivenessTransport transport(/*echo_data=*/false);
  RecordingEventSink events;
  SilentNoDataDevice device;
  ManualCancellationToken cancel;

  auto options = session_options();
  options.dpd_idle_poll_interval = 2;
  options.dead_peer_poll_budget = 3;
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before exhaustion test") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before exhaustion test") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "transport_closed",
              "exhausted reconnect should fail with transport_closed") &&
       ok;
  ok = expect(session.reconnect_attempts() == 1,
              "exhaustion should stop after max_reconnects attempts") &&
       ok;
  ok = expect(transport.auth_attempts() == 1,
              "exhaustion should not re-authenticate while cached cookie works") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::failed,
              "exhausted reconnect should mark session failed") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = test_authenticate_success_and_connect_cstp() && ok;
  ok = test_auth_failure_never_reconnects() && ok;
  ok = test_packet_echo() && ok;
  ok = test_reconnect_after_transport_close() && ok;
  ok = test_cancellation_exits_without_opening_device() && ok;
  ok = test_active_loop_cancellation_during_no_data_poll_exits() && ok;
  ok = test_disconnect_stops_before_packet_loop() && ok;
  ok = test_inbound_dpd_request_is_answered() && ok;
  ok = test_peer_disconnect_is_not_generic_transport_eof() && ok;
  ok = test_terminate_frame_is_not_generic_transport_eof() && ok;
  ok = test_compressed_frame_fails_with_explicit_unsupported_code() && ok;
  ok = test_idle_keepalive_is_emitted() && ok;
  ok = test_metadata_keepalive_drives_idle_timer() && ok;
  ok = test_dead_peer_triggers_reconnect() && ok;
  ok = test_reconnect_reauthenticates_when_cached_cookie_expires() && ok;
  ok = test_reconnect_skips_cached_cookie_after_session_timeout() && ok;
  ok = test_reconnect_reopens_device_with_updated_metadata() && ok;
  ok = test_metadata_new_tunnel_rekey_triggers_reconnect() && ok;
  ok = test_metadata_ssl_rekey_reports_unsupported_when_reconnect_disabled() && ok;
  ok = test_metadata_idle_timeout_has_specific_code() && ok;
  ok = test_metadata_session_timeout_has_specific_code() && ok;
  ok = test_reconnect_exhaustion_fails_with_stable_code() && ok;

  return ok ? 0 : 1;
}
