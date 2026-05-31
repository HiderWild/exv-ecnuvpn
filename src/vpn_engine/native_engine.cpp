#include "vpn_engine/native_engine.hpp"

#include "vpn_engine/protocol/production_transport.hpp"

#if defined(_WIN32)
#include "platform/win32/native_packet_device.hpp"
#include "platform/win32/native_tls_stream.hpp"
#elif defined(__APPLE__)
#include "platform/darwin/native_packet_device.hpp"
#include "platform/darwin/native_tls_stream.hpp"
#endif

#include <exception>
#include <map>
#include <memory>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {

namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

ValidationResult validate_engine_config(const VpnEngineConfig &cfg) {
  if (cfg.server.empty())
    return invalid("config_invalid", "VPN server is not configured.");
  if (cfg.username.empty())
    return invalid("config_invalid", "VPN username is not configured.");
  if (cfg.password.empty())
    return invalid("config_invalid", "VPN password is not configured.");
  return ValidationResult{};
}

ValidationResult native_transport_unimplemented() {
  return invalid("native_transport_unimplemented",
                 "Native engine production TLS transport is not implemented yet.");
}

ValidationResult native_packet_device_unimplemented() {
  return invalid("native_packet_device_unimplemented",
                 "Native engine platform packet device is not available.");
}

class UnsupportedProtocolTransport final
    : public protocol::ProtocolTransport {
public:
  protocol::AuthResult
  authenticate(const protocol::ProtocolSessionOptions & /*options*/) override {
    protocol::AuthResult result;
    result.ok = false;
    result.error_code = "native_transport_unavailable";
    result.error_message =
        "Native engine production TLS transport is not available on this platform.";
    return result;
  }

  ValidationResult connect_cstp(const std::string & /*cookie*/,
                                TunnelMetadata * /*metadata*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  ValidationResult
  exchange_packet(const std::vector<std::uint8_t> & /*packet*/,
                  std::vector<std::uint8_t> * /*response_packet*/) override {
    return invalid("native_transport_unavailable",
                   "Native engine production TLS transport is not available on this platform.");
  }

  void disconnect() override {}
  void reset_for_reconnect() override {}
};

class UnsupportedPacketDevice final : public PacketDevice {
public:
  ValidationResult open(const TunnelMetadata & /*metadata*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  ValidationResult read_packet(std::vector<std::uint8_t> * /*packet*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return invalid("native_packet_device_unavailable",
                   "Native engine platform packet device is not available on this platform.");
  }

  void close() override {}
};

} // namespace

ValidationResult validate_native_config(const Config &cfg) {
  if (!cfg.extra_args.empty()) {
    return invalid("unsupported_extra_args",
                   "Native VPN engine does not support legacy OpenConnect extra_args.");
  }
  if (cfg.server.empty())
    return invalid("config_invalid", "VPN server is not configured.");
  if (cfg.username.empty())
    return invalid("config_invalid", "VPN username is not configured.");
  return ValidationResult{};
}

VpnEngineConfig make_native_config(const Config &cfg,
                                   const std::string &plaintext_password) {
  VpnEngineConfig engine_cfg;
  engine_cfg.engine = cfg.vpn_engine;
  engine_cfg.server = cfg.server;
  engine_cfg.username = cfg.username;
  engine_cfg.password = plaintext_password;
  engine_cfg.useragent = cfg.useragent;
  engine_cfg.mtu = cfg.mtu;
  engine_cfg.routes = cfg.routes;
  engine_cfg.windows_tunnel_driver = cfg.windows_tunnel_driver;
  engine_cfg.windows_tap_interface = cfg.windows_tap_interface;
  engine_cfg.auto_reconnect = cfg.auto_reconnect;
  engine_cfg.disable_dtls = cfg.disable_dtls;
  return engine_cfg;
}

nlohmann::json event_to_json(const VpnEngineEvent &event) {
  return nlohmann::json{{"type", event.type},
                        {"level", event.level},
                        {"message", event.message},
                        {"fields", event.fields}};
}

nlohmann::json status_to_json(const VpnEngineStatus &status) {
  return nlohmann::json{{"running", status.running},
                        {"network_ready", status.network_ready},
                        {"pid", status.pid},
                        {"interface", status.interface_name},
                        {"internal_ip", status.internal_ip},
                        {"error_code", status.error_code},
                        {"error_message", status.error_message}};
}

NativeVpnEngineDependencies default_native_engine_dependencies() {
  NativeVpnEngineDependencies deps;

#if defined(_WIN32) || defined(__APPLE__)
  deps.transport_factory = [] {
    std::unique_ptr<protocol::TlsStream> stream(new platform::NativeTlsStream());
    return std::unique_ptr<protocol::ProtocolTransport>(
        new protocol::ProductionProtocolTransport(std::move(stream)));
  };
  deps.packet_device_factory = [] {
    return platform::create_native_packet_device();
  };
#else
  deps.transport_factory = [] {
    return std::unique_ptr<protocol::ProtocolTransport>(
        new UnsupportedProtocolTransport());
  };
  deps.packet_device_factory = [] {
    return std::unique_ptr<PacketDevice>(new UnsupportedPacketDevice());
  };
#endif

  return deps;
}

class NativeVpnEngineSession::LoopEventSink final : public EventSink {
public:
  explicit LoopEventSink(NativeVpnEngineSession *owner) : owner_(owner) {}

  void emit(const VpnEngineEvent &event) override {
    if (owner_)
      owner_->on_loop_event(event);
    if (owner_ && owner_->dependencies_.event_sink)
      owner_->dependencies_.event_sink->emit(event);
  }

private:
  NativeVpnEngineSession *owner_ = nullptr;
};

NativeVpnEngineSession::NativeVpnEngineSession(VpnEngineConfig config)
    : NativeVpnEngineSession(std::move(config),
                             default_native_engine_dependencies()) {}

NativeVpnEngineSession::NativeVpnEngineSession(
    VpnEngineConfig config, NativeVpnEngineDependencies dependencies)
    : config_(std::move(config)), dependencies_(std::move(dependencies)) {}

NativeVpnEngineSession::~NativeVpnEngineSession() { stop(); }

ValidationResult NativeVpnEngineSession::start() {
  {
    const std::lock_guard<std::mutex> lock(mu_);
    if (status_.running || packet_loop_thread_.joinable()) {
      return invalid("session_already_running",
                     "Native VPN engine session is already running.");
    }

    status_ = VpnEngineStatus{};
    loop_started_ = false;
    loop_finished_ = false;
    loop_start_result_ = ValidationResult{};
    cancel_requested_.store(false);
  }

  auto fail = [this](const ValidationResult &failure) {
    {
      const std::lock_guard<std::mutex> lock(mu_);
      set_failure_locked(failure);
    }
    emit_event("native.start.failed", "error", failure.message,
               {{"code", failure.code}});
    return failure;
  };

  ValidationResult validated = validate_engine_config(config_);
  if (!validated.ok)
    return fail(validated);

  if (!config_.disable_dtls) {
    return fail(invalid("unsupported_dtls",
                        "Native engine v1 supports CSTP/TLS only."));
  }

  protocol::ParsedVpnUrl parsed;
  ValidationResult parsed_result = protocol::parse_vpn_url(config_.server, &parsed);
  if (!parsed_result.ok)
    return fail(parsed_result);

  if (!dependencies_.transport_factory)
    return fail(native_transport_unimplemented());

  std::unique_ptr<protocol::ProtocolTransport> transport;
  try {
    transport = dependencies_.transport_factory();
  } catch (const std::exception &e) {
    return fail(invalid("native_transport_factory_failed", e.what()));
  } catch (...) {
    return fail(invalid("native_transport_factory_failed",
                        "Native transport factory failed."));
  }

  if (!transport)
    return fail(native_transport_unimplemented());

  protocol::ProtocolSessionOptions options;
  options.server = parsed;
  options.username = config_.username;
  options.password = config_.password;
  options.useragent = config_.useragent;
  options.disable_dtls = config_.disable_dtls;
  options.auto_reconnect = config_.auto_reconnect;
  options.max_reconnects = config_.auto_reconnect ? 1 : 0;

  auto protocol_session =
      std::make_unique<protocol::ProtocolSession>(options, transport.get());

  emit_event("native.starting", "info", "native VPN engine starting",
             {{"host", parsed.host}, {"port", std::to_string(parsed.port)}});
  emit_event("auth.started", "info", "password auth started");

  ValidationResult auth = protocol_session->authenticate();
  if (!auth.ok) {
    emit_event("auth.failed", "error", auth.message, {{"code", auth.code}});
    return fail(auth);
  }

  emit_event("auth.succeeded", "info", "password auth succeeded");

  TunnelMetadata metadata;
  ValidationResult cstp = protocol_session->connect_cstp(&metadata);
  if (!cstp.ok) {
    emit_event("cstp.failed", "error", cstp.message, {{"code", cstp.code}});
    transport->disconnect();
    return fail(cstp);
  }

  emit_event("cstp.connected", "info", "CSTP connect succeeded",
             {{"interface", metadata.interface_name},
              {"internal_ip", metadata.internal_ip4_address}});

  if (!dependencies_.packet_device_factory) {
    transport->disconnect();
    return fail(native_packet_device_unimplemented());
  }

  std::unique_ptr<PacketDevice> packet_device;
  try {
    packet_device = dependencies_.packet_device_factory();
  } catch (const std::exception &e) {
    transport->disconnect();
    return fail(invalid("native_packet_device_factory_failed", e.what()));
  } catch (...) {
    transport->disconnect();
    return fail(invalid("native_packet_device_factory_failed",
                        "Native packet device factory failed."));
  }

  if (!packet_device) {
    transport->disconnect();
    return fail(native_packet_device_unimplemented());
  }

  {
    const std::lock_guard<std::mutex> lock(mu_);
    status_.pid = -1;
    status_.interface_name = metadata.interface_name;
    status_.internal_ip = metadata.internal_ip4_address;
    status_.error_code.clear();
    status_.error_message.clear();

    transport_ = std::move(transport);
    protocol_session_ = std::move(protocol_session);
    packet_device_ = std::move(packet_device);
    loop_event_sink_ = std::make_unique<LoopEventSink>(this);
  }

  packet_loop_thread_ = std::thread(&NativeVpnEngineSession::run_packet_loop, this);

  std::unique_lock<std::mutex> lock(mu_);
  startup_cv_.wait(lock,
                   [this]() { return loop_started_ || loop_finished_; });

  if (loop_started_)
    return ValidationResult{};

  ValidationResult failure = loop_start_result_;
  lock.unlock();

  if (packet_loop_thread_.joinable())
    packet_loop_thread_.join();

  if (failure.ok) {
    failure = invalid("packet_loop_not_started",
                      "Native packet loop exited before startup completed.");
  }

  return fail(failure);
}

void NativeVpnEngineSession::stop() {
  cancel_requested_.store(true);

  if (packet_loop_thread_.joinable())
    packet_loop_thread_.join();

  {
    const std::lock_guard<std::mutex> lock(mu_);
    status_.running = false;
    status_.network_ready = false;
    status_.pid = -1;
    protocol_session_.reset();
    transport_.reset();
    packet_device_.reset();
    loop_event_sink_.reset();
    loop_started_ = false;
    loop_finished_ = true;
  }
}

VpnEngineStatus NativeVpnEngineSession::status() const {
  const std::lock_guard<std::mutex> lock(mu_);
  return status_;
}

bool NativeVpnEngineSession::is_cancelled() const {
  return cancel_requested_.load();
}

void NativeVpnEngineSession::emit_event(
    std::string type, std::string level, std::string message,
    std::map<std::string, std::string> fields) {
  if (!dependencies_.event_sink)
    return;

  VpnEngineEvent event;
  event.type = std::move(type);
  event.level = std::move(level);
  event.message = std::move(message);
  event.fields = std::move(fields);
  dependencies_.event_sink->emit(event);
}

void NativeVpnEngineSession::on_loop_event(const VpnEngineEvent &event) {
  if (event.type != "packet.loop.started")
    return;

  {
    const std::lock_guard<std::mutex> lock(mu_);
    status_.running = true;
    status_.network_ready = true;
    status_.pid = -1;
    status_.error_code.clear();
    status_.error_message.clear();
    loop_started_ = true;
    loop_start_result_ = ValidationResult{};
  }
  startup_cv_.notify_all();
}

void NativeVpnEngineSession::run_packet_loop() {
  ValidationResult result;

  std::unique_ptr<protocol::ProtocolTransport> transport;
  std::unique_ptr<protocol::ProtocolSession> session;
  std::unique_ptr<PacketDevice> device;
  std::unique_ptr<LoopEventSink> events;
  {
    const std::lock_guard<std::mutex> lock(mu_);
    transport = std::move(transport_);
    session = std::move(protocol_session_);
    device = std::move(packet_device_);
    events = std::move(loop_event_sink_);
  }

  if (!session || !device) {
    result = invalid("packet_loop_missing_dependency",
                     "Native packet loop dependency is missing.");
  } else {
    result = session->run_packet_loop(device.get(), events.get(), this);
  }

  if (session) {
    session->disconnect();
  } else if (transport) {
    transport->disconnect();
  }

  if (device)
    device->close();

  {
    const std::lock_guard<std::mutex> lock(mu_);
    loop_finished_ = true;
    status_.running = false;
    status_.network_ready = false;
    status_.pid = -1;
    if (!loop_started_) {
      loop_start_result_ = result;
      if (!result.ok)
        set_failure_locked(result);
    } else if (!result.ok) {
      if (result.code != "session_cancelled")
        set_failure_locked(result);
    }
  }

  startup_cv_.notify_all();
}

void NativeVpnEngineSession::set_failure_locked(
    const ValidationResult &failure) {
  status_.running = false;
  status_.network_ready = false;
  status_.pid = -1;
  status_.error_code = failure.code;
  status_.error_message = failure.message;
}

} // namespace vpn_engine
} // namespace ecnuvpn
