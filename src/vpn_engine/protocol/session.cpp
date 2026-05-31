#include "vpn_engine/protocol/session.hpp"

#include <atomic>
#include <chrono>
#include <cstdint>
#include <map>
#include <mutex>
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

  // Defensive tunnel-MTU normalization at the device seam: the gateway value
  // is validated by the CSTP parser, but a misbehaving/forged transport could
  // still yield an unusable MTU. Fall back to the configured safe default
  // rather than handing an out-of-range value to the platform packet device.
  if (connected_metadata.mtu < 576 || connected_metadata.mtu > 1500) {
    int fallback = options_.mtu_fallback;
    if (fallback < 576 || fallback > 1500)
      fallback = 1290;
    connected_metadata.mtu = fallback;
  }

  metadata_ = connected_metadata;
  *metadata = connected_metadata;
  cstp_connected_ = true;
  state_.tunnel_configured(connected_metadata);
  return ValidationResult{};}

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

  while (true) {
    ForwardingOutcome outcome = run_forwarding(device, events, cancel);

    if (outcome.cancelled) {
      return stop_cancelled(device, events);
    }

    if (outcome.result.ok) {
      // Graceful end of the forwarding session (e.g. the device drained). Close
      // the device on this (the loop) thread but leave the session marked
      // network-ready and report success.
      device->close();
      current_device_ = nullptr;
      return outcome.result;
    }

    const ValidationResult &failed = outcome.result;
    emit_event(events, "transport.closed", "error", failed.message,
               {{"code", failed.code}});

    const bool can_reconnect =
        failed.code == "transport_closed" && options_.auto_reconnect &&
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
    state_.failed(failed.code, failed.message);
    return failed;
  }
}

ProtocolSession::ForwardingOutcome
ProtocolSession::run_forwarding(PacketDevice *device, EventSink *events,
                                CancellationToken *cancel) {
  std::atomic<bool> stop{false};

  // Monotonic counter of inbound CSTP frames seen by the reader thread. Any
  // frame (data, keepalive, DPD request/response) proves the peer is alive and
  // is used by the outbound thread's dead-peer detector.
  std::atomic<std::uint64_t> inbound_activity{0};

  // terminate reason: 0 = none, 1 = graceful, 2 = cancelled, 3 = fatal.
  std::mutex reason_mu;
  int reason = 0;
  ValidationResult fatal;

  auto set_reason = [&](int new_reason, ValidationResult result) {
    const std::lock_guard<std::mutex> lock(reason_mu);
    if (reason == 0) {
      reason = new_reason;
      if (new_reason == 3)
        fatal = std::move(result);
    }
    stop.store(true);
  };

  // Inbound: drain CSTP frames from the gateway and route them to the device.
  // Runs until the transport reports a closed/failed read (which the outbound
  // side triggers via transport_->disconnect()), a peer disconnect frame, a
  // device write failure, or cancellation. It intentionally does not gate on
  // `stop` so that frames already queued when the outbound side ends are still
  // drained before the transport read reports closure.
  std::thread inbound([&]() {
    while (true) {
      if (cancellation_requested(cancel)) {
        set_reason(2, ValidationResult{});
        break;
      }

      InboundFrame frame;
      ValidationResult received = transport_->receive_frame(&frame);
      if (!received.ok) {
        set_reason(3, received);
        break;
      }

      // Any decoded frame is liveness evidence for the dead-peer detector.
      inbound_activity.fetch_add(1, std::memory_order_relaxed);

      if (frame.kind == InboundFrameKind::data) {
        ValidationResult written = device->write_packet(frame.payload);
        if (!written.ok) {
          set_reason(3, written);
          break;
        }
        emit_event(events, "packet.inbound", "info", "inbound packet",
                   {{"bytes", std::to_string(frame.payload.size())}});
        continue;
      }

      if (frame.kind == InboundFrameKind::dpd_request) {
        // Servicing an inbound DPD request is mandatory regardless of timers:
        // answer with a DPD response so the gateway sees us as alive.
        ValidationResult responded =
            transport_->send_control(InboundFrameKind::dpd_response);
        if (!responded.ok) {
          set_reason(3, responded);
          break;
        }
        emit_event(events, "dpd.responded", "info", "DPD response sent");
        continue;
      }

      if (frame.kind == InboundFrameKind::disconnect) {
        set_reason(3, invalid("transport_closed", "CSTP peer disconnected"));
        break;
      }

      // dpd_response / keepalive / none: already counted as activity above;
      // no further data-plane action.
    }

    // Signal the outbound (loop) thread to stop. The device is non-blocking and
    // polled by the outbound thread, so it observes `stop` and exits on its own;
    // the loop thread owns closing the device.
    stop.store(true);
  });

  // Outbound: read IP packets from the device and frame them to the gateway.
  int retryable_read_count = 0;
  const int max_retryable_reads =
      options_.packet_loop_no_data_poll_limit < 0
          ? 0
          : options_.packet_loop_no_data_poll_limit;

  // Dead-peer / keepalive state for the idle path.
  bool dpd_probe_outstanding = false;
  std::uint64_t dpd_probe_baseline = 0;
  int dpd_wait_polls = 0;

  // Services keepalive/DPD timers on each idle outbound poll. Returns a non-ok
  // result to terminate forwarding (transport_closed for a dead peer, or a
  // propagated control-write failure). `idle_polls` is the current count of
  // consecutive idle polls.
  auto service_liveness = [&](int idle_polls) -> ValidationResult {
    // Dead-peer detection: if a DPD probe is outstanding and no inbound frame
    // has arrived since it was sent, count down the budget.
    if (dpd_probe_outstanding) {
      if (inbound_activity.load(std::memory_order_relaxed) !=
          dpd_probe_baseline) {
        dpd_probe_outstanding = false;
        dpd_wait_polls = 0;
      } else if (options_.dead_peer_poll_budget > 0) {
        ++dpd_wait_polls;
        if (dpd_wait_polls >= options_.dead_peer_poll_budget) {
          return invalid("transport_closed",
                         "dead peer detected: no response to DPD probe");
        }
      }
    }

    if (options_.keepalive_idle_poll_interval > 0 &&
        (idle_polls % options_.keepalive_idle_poll_interval) == 0) {
      ValidationResult sent = transport_->send_control(InboundFrameKind::keepalive);
      if (!sent.ok)
        return sent;
      emit_event(events, "dpd.keepalive", "info", "keepalive sent");
    }

    if (options_.dpd_idle_poll_interval > 0 && !dpd_probe_outstanding &&
        (idle_polls % options_.dpd_idle_poll_interval) == 0) {
      ValidationResult sent =
          transport_->send_control(InboundFrameKind::dpd_request);
      if (!sent.ok)
        return sent;
      dpd_probe_outstanding = true;
      dpd_probe_baseline = inbound_activity.load(std::memory_order_relaxed);
      dpd_wait_polls = 0;
      emit_event(events, "dpd.request", "info", "DPD probe sent");
    }

    return {};
  };

  while (!stop.load()) {
    if (cancellation_requested(cancel)) {
      set_reason(2, ValidationResult{});
      break;
    }

    std::vector<std::uint8_t> packet;
    ValidationResult read = device->read_packet(&packet);
    if (!read.ok) {
      if (is_clean_packet_loop_end(read)) {
        set_reason(1, ValidationResult{});
        break;
      }

      if (is_retryable_packet_read(read)) {
        ++retryable_read_count;
        if (cancellation_requested(cancel)) {
          set_reason(2, ValidationResult{});
          break;
        }
        if (retryable_read_count <= max_retryable_reads && !stop.load()) {
          ValidationResult live = service_liveness(retryable_read_count);
          if (!live.ok) {
            set_reason(3, live);
            break;
          }
          std::this_thread::yield();
          std::this_thread::sleep_for(std::chrono::milliseconds(1));
          continue;
        }
        if (stop.load())
          break;
      }

      set_reason(3, read);
      break;
    }

    retryable_read_count = 0;

    ValidationResult sent = transport_->send_packet(packet);
    if (!sent.ok) {
      set_reason(3, sent);
      break;
    }

    emit_event(events, "packet.outbound", "info", "outbound packet",
               {{"bytes", std::to_string(packet.size())}});
  }

  stop.store(true);
  // Unblock a blocking inbound transport read.
  transport_->disconnect();
  inbound.join();

  ForwardingOutcome outcome;
  {
    const std::lock_guard<std::mutex> lock(reason_mu);
    outcome.cancelled = reason == 2;
    if (reason == 3)
      outcome.result = fatal;
  }
  return outcome;
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
