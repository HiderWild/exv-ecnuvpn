#include "vpn_engine/protocol/session.hpp"

#include <chrono>
#include <map>
#include <string>
#include <thread>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

void emit_event(EventSink *events, std::string type, std::string level,
                std::string message,
                std::map<std::string, std::string> fields = {}) {
  if (!events)
    return;

  VpnEngineEvent event;
  event.type = std::move(type);
  event.level = std::move(level);
  event.message = std::move(message);
  event.fields = std::move(fields);
  events->emit(event);
}

bool is_clean_packet_loop_end(const ValidationResult &result) {
  return result.code == "packet_device_empty";
}

bool is_retryable_packet_read(const ValidationResult &result) {
  return result.code == "no_data" || result.code == "try_again" ||
         result.code == "would_block";
}

ValidationResult cancelled() {
  return invalid("session_cancelled", "protocol session is cancelled");
}

} // namespace

ProtocolSession::ProtocolSession(ProtocolSessionOptions options,
                                 ProtocolTransport *transport)
    : options_(std::move(options)), transport_(transport) {}

ValidationResult ProtocolSession::authenticate() {
  if (!transport_)
    return invalid("transport_missing", "protocol transport is not configured");
  if (disconnect_requested_.load())
    return invalid("session_cancelled", "protocol session is cancelled");

  state_.auth_started();

  AuthResult auth = transport_->authenticate(options_);
  if (!auth.ok) {
    authenticated_ = false;
    cookie_.clear();
    state_.failed(auth.error_code, auth.error_message);
    return invalid(auth.error_code, auth.error_message);
  }

  authenticated_ = true;
  cookie_ = std::move(auth.cookie);
  state_.auth_succeeded();
  return ValidationResult{};
}

ValidationResult ProtocolSession::connect_cstp(TunnelMetadata *metadata) {
  if (!metadata)
    return invalid("cstp_null_metadata", "metadata output must not be null");
  if (!transport_)
    return invalid("transport_missing", "protocol transport is not configured");
  if (disconnect_requested_.load())
    return invalid("session_cancelled", "protocol session is cancelled");
  if (!authenticated_)
    return invalid("auth_required", "authenticate must succeed before CSTP connect");

  TunnelMetadata connected_metadata;
  ValidationResult connected =
      transport_->connect_cstp(cookie_, &connected_metadata);
  if (!connected.ok) {
    cstp_connected_ = false;
    state_.failed(connected.code, connected.message);
    return connected;
  }

  metadata_ = connected_metadata;
  *metadata = connected_metadata;
  cstp_connected_ = true;
  state_.tunnel_configured(connected_metadata);
  return ValidationResult{};
}

ValidationResult ProtocolSession::run_packet_loop(PacketDevice *device,
                                                  EventSink *events,
                                                  CancellationToken *cancel) {
  if (!device)
    return invalid("packet_device_missing", "packet device is not configured");
  if (!transport_)
    return invalid("transport_missing", "protocol transport is not configured");
  if (cancellation_requested(cancel)) {
    return stop_cancelled(nullptr, events);
  }
  if (!cstp_connected_)
    return invalid("cstp_not_connected",
                   "connect_cstp must succeed before packet loop");

  current_device_ = device;
  ValidationResult opened = device->open(metadata_);
  if (!opened.ok) {
    current_device_ = nullptr;
    state_.failed(opened.code, opened.message);
    emit_event(events, "packet_device.failed", "error", opened.message,
               {{"code", opened.code}});
    return opened;
  }

  state_.packet_loop_started();
  emit_event(events, "packet.loop.started", "info", "packet loop started");

  int retryable_read_count = 0;
  const int max_retryable_reads =
      options_.packet_loop_no_data_poll_limit < 0
          ? 0
          : options_.packet_loop_no_data_poll_limit;

  while (true) {
    if (cancellation_requested(cancel)) {
      return stop_cancelled(device, events);
    }

    std::vector<std::uint8_t> packet;
    ValidationResult read = device->read_packet(&packet);
    if (!read.ok) {
      if (is_clean_packet_loop_end(read)) {
        current_device_ = nullptr;
        return ValidationResult{};
      }

      if (is_retryable_packet_read(read)) {
        ++retryable_read_count;
        if (cancellation_requested(cancel)) {
          return stop_cancelled(device, events);
        }
        if (retryable_read_count <= max_retryable_reads) {
          std::this_thread::yield();
          std::this_thread::sleep_for(std::chrono::milliseconds(1));
          continue;
        }
      }

      device->close();
      current_device_ = nullptr;
      state_.failed(read.code, read.message);
      return read;
    }

    retryable_read_count = 0;

    if (cancellation_requested(cancel)) {
      return stop_cancelled(device, events);
    }

    std::vector<std::uint8_t> response_packet;
    ValidationResult exchanged =
        transport_->exchange_packet(packet, &response_packet);
    if (!exchanged.ok) {
      emit_event(events, "transport.closed", "error", exchanged.message,
                 {{"code", exchanged.code}});

      const bool can_reconnect =
          exchanged.code == "transport_closed" && options_.auto_reconnect &&
          reconnect_attempts_ < options_.max_reconnects &&
          !cancellation_requested(cancel);

      if (can_reconnect) {
        ValidationResult reconnected = reconnect(device, events, cancel);
        if (reconnected.ok)
          continue;
        current_device_ = nullptr;
        return reconnected;
      }

      device->close();
      current_device_ = nullptr;
      state_.failed(exchanged.code, exchanged.message);
      return exchanged;
    }

    ValidationResult written = device->write_packet(response_packet);
    if (!written.ok) {
      device->close();
      current_device_ = nullptr;
      state_.failed(written.code, written.message);
      return written;
    }

    emit_event(events, "packet.echo", "info", "packet echoed",
               {{"bytes", std::to_string(response_packet.size())}});
  }
}

void ProtocolSession::disconnect() {
  disconnect_requested_.store(true);

  if (transport_)
    transport_->disconnect();
  if (current_device_)
    current_device_->close();

  authenticated_ = false;
  cstp_connected_ = false;
  cookie_.clear();
  state_.stopped();
}

const SessionState &ProtocolSession::state() const { return state_; }

int ProtocolSession::reconnect_attempts() const { return reconnect_attempts_; }

ValidationResult ProtocolSession::reconnect(PacketDevice *device, EventSink *events,
                                            CancellationToken *cancel) {
  ++reconnect_attempts_;
  state_.phase = SessionPhase::reconnecting;
  emit_event(events, "reconnect_started", "info", "reconnect started",
             {{"attempt", std::to_string(reconnect_attempts_)}});

  device->close();
  transport_->disconnect();
  transport_->reset_for_reconnect();

  authenticated_ = false;
  cstp_connected_ = false;
  cookie_.clear();
  metadata_ = TunnelMetadata{};

  if (cancellation_requested(cancel)) {
    return stop_cancelled(nullptr, events);
  }

  ValidationResult auth = authenticate();
  if (!auth.ok) {
    emit_event(events, "reconnect_failed", "error", auth.message,
               {{"code", auth.code},
                {"attempt", std::to_string(reconnect_attempts_)}});
    return auth;
  }

  TunnelMetadata metadata;
  ValidationResult connected = connect_cstp(&metadata);
  if (!connected.ok) {
    emit_event(events, "reconnect_failed", "error", connected.message,
               {{"code", connected.code},
                {"attempt", std::to_string(reconnect_attempts_)}});
    return connected;
  }

  if (cancellation_requested(cancel)) {
    return stop_cancelled(nullptr, events);
  }

  ValidationResult opened = device->open(metadata_);
  if (!opened.ok) {
    state_.failed(opened.code, opened.message);
    emit_event(events, "packet_device.failed", "error", opened.message,
               {{"code", opened.code}});
    emit_event(events, "reconnect_failed", "error", opened.message,
               {{"code", opened.code},
                {"attempt", std::to_string(reconnect_attempts_)}});
    return opened;
  }

  state_.packet_loop_started();
  emit_event(events, "packet.loop.started", "info", "packet loop started");
  emit_event(events, "reconnect_succeeded", "info", "reconnect succeeded",
             {{"attempt", std::to_string(reconnect_attempts_)}});
  return ValidationResult{};
}

ValidationResult ProtocolSession::stop_cancelled(PacketDevice *device,
                                                 EventSink *events) {
  if (device)
    device->close();
  current_device_ = nullptr;
  state_.stopped();
  emit_event(events, "packet.loop.stopped", "info", "packet loop stopped");
  return cancelled();
}

bool ProtocolSession::cancellation_requested(
    const CancellationToken *cancel) const {
  return disconnect_requested_.load() || (cancel && cancel->is_cancelled());
}

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
