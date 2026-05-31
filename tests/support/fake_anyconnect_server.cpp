#include "fake_anyconnect_server.hpp"

#include "vpn_engine/protocol/cstp.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <sstream>
#include <utility>

namespace ecnuvpn {
namespace tests {
namespace support {

namespace {

vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

vpn_engine::TunnelMetadata default_tunnel_metadata() {
  vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "fake-cstp0";
  metadata.interface_index = 7;
  metadata.internal_ip4_address = "10.255.0.10";
  metadata.internal_ip4_netmask = "255.255.255.0";
  metadata.mtu = 1400;
  metadata.routes = {"198.51.100.0/24", "203.0.113.0/24"};
  metadata.server_bypass_ips = {"192.0.2.0/24"};
  return metadata;
}

vpn_engine::protocol::AuthResult auth_parse_error(
    const vpn_engine::ValidationResult &result) {
  vpn_engine::protocol::AuthResult auth;
  auth.ok = false;
  auth.error_code = result.code;
  auth.error_message = result.message;
  return auth;
}

std::string auth_success_response(const std::string &session_cookie) {
  return "HTTP/1.1 200 OK\r\n"
         "Content-Type: text/html; charset=utf-8\r\n"
         "Set-Cookie: " +
         session_cookie +
         "; Path=/; Secure; HttpOnly\r\n"
         "\r\n"
         "<html><body>Login OK</body></html>";
}

std::string auth_failure_response() {
  return "HTTP/1.1 401 Unauthorized\r\n"
         "Content-Type: application/json\r\n"
         "Cache-Control: no-store\r\n"
         "\r\n"
         "{\"error\":\"auth_failed\",\"message\":\"invalid username or password\"}";
}

std::string cstp_connect_response(const vpn_engine::TunnelMetadata &metadata) {
  std::ostringstream out;
  out << "HTTP/1.1 200 OK\r\n";
  out << "Content-Type: application/octet-stream\r\n";
  out << "X-CSTP-Address: " << metadata.internal_ip4_address << "\r\n";
  out << "X-CSTP-Netmask: " << metadata.internal_ip4_netmask << "\r\n";
  out << "X-CSTP-MTU: " << metadata.mtu << "\r\n";
  for (const std::string &route : metadata.routes)
    out << "X-CSTP-Split-Include: " << route << "\r\n";
  for (const std::string &route : metadata.server_bypass_ips)
    out << "X-CSTP-Bypass-Route: " << route << "\r\n";
  out << "\r\n";
  return out.str();
}

void emit_event(vpn_engine::EventSink &sink, std::string type,
                std::string level, std::string message,
                std::map<std::string, std::string> fields = {}) {
  vpn_engine::VpnEngineEvent event;
  event.type = std::move(type);
  event.level = std::move(level);
  event.message = std::move(message);
  event.fields = std::move(fields);
  sink.emit(event);
}

vpn_engine::ValidationResult connect_once(FakeAnyConnectServer &server,
                                          vpn_engine::PacketDevice &device,
                                          vpn_engine::EventSink &events,
                                          const FakeAnyConnectRunOptions &options,
                                          vpn_engine::SessionState *state) {
  if (!state)
    return invalid("session_state_missing", "session state pointer is null");

  state->auth_started();
  emit_event(events, "auth.started", "info", "password auth started");

  const auto auth = server.password_authenticate(options.credentials);
  if (!auth.ok) {
    state->failed(auth.error_code, auth.error_message);
    emit_event(events, "auth.failed", "error", auth.error_message,
               {{"code", auth.error_code}});
    return invalid(auth.error_code, auth.error_message);
  }

  state->auth_succeeded();
  emit_event(events, "auth.succeeded", "info", "password auth succeeded");

  vpn_engine::TunnelMetadata metadata;
  vpn_engine::ValidationResult connected =
      server.connect_cstp(auth.cookie, &metadata);
  if (!connected.ok) {
    state->failed(connected.code, connected.message);
    emit_event(events, "cstp.failed", "error", connected.message,
               {{"code", connected.code}});
    return connected;
  }

  state->tunnel_configured(metadata);
  emit_event(events, "cstp.connected", "info", "CSTP connect succeeded",
             {{"address", metadata.internal_ip4_address}});

  vpn_engine::ValidationResult opened = device.open(metadata);
  if (!opened.ok) {
    state->failed(opened.code, opened.message);
    emit_event(events, "packet_device.failed", "error", opened.message,
               {{"code", opened.code}});
    return opened;
  }

  state->packet_loop_started();
  emit_event(events, "packet.loop.started", "info", "packet loop started");
  return vpn_engine::ValidationResult{};
}

} // namespace

FakeAnyConnectServerOptions::FakeAnyConnectServerOptions()
    : tunnel_metadata(default_tunnel_metadata()) {}

FakeAnyConnectServer::FakeAnyConnectServer(
    FakeAnyConnectServerOptions options)
    : options_(std::move(options)) {}

vpn_engine::protocol::AuthResult FakeAnyConnectServer::password_authenticate(
    const FakeAnyConnectCredentials &credentials) {
  ++auth_attempts_;

  const bool accepted =
      credentials.username == options_.expected_credentials.username &&
      credentials.password == options_.expected_credentials.password;
  const std::string raw =
      accepted ? auth_success_response(options_.session_cookie)
               : auth_failure_response();

  vpn_engine::protocol::HttpResponse response;
  vpn_engine::ValidationResult parsed =
      vpn_engine::protocol::parse_http_response(raw, &response);
  if (!parsed.ok)
    return auth_parse_error(parsed);

  return vpn_engine::protocol::parse_auth_response(response);
}

vpn_engine::ValidationResult FakeAnyConnectServer::connect_cstp(
    const std::string &cookie, vpn_engine::TunnelMetadata *metadata) {
  if (!metadata)
    return invalid("cstp_null_metadata", "metadata output must not be null");
  if (cookie != options_.session_cookie)
    return invalid("auth_cookie_invalid", "CSTP connect requires auth cookie");
  if (closed_)
    return invalid("transport_closed", "server transport is closed");

  ++cstp_connects_;

  vpn_engine::protocol::HttpResponse response;
  vpn_engine::ValidationResult parsed =
      vpn_engine::protocol::parse_http_response(
          cstp_connect_response(options_.tunnel_metadata), &response);
  if (!parsed.ok)
    return parsed;

  vpn_engine::TunnelMetadata parsed_metadata;
  vpn_engine::ValidationResult cstp =
      vpn_engine::protocol::parse_cstp_headers(response, &parsed_metadata);
  if (!cstp.ok)
    return cstp;

  parsed_metadata.interface_name = options_.tunnel_metadata.interface_name;
  parsed_metadata.interface_index = options_.tunnel_metadata.interface_index;
  *metadata = std::move(parsed_metadata);
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult FakeAnyConnectServer::exchange_packet(
    const std::vector<std::uint8_t> &packet,
    std::vector<std::uint8_t> *echoed_packet) {
  if (!echoed_packet)
    return invalid("packet_null_out", "echoed packet output must not be null");
  echoed_packet->clear();

  if (closed_)
    return invalid("transport_closed", "server transport is closed");

  vpn_engine::protocol::CstpFrame outbound;
  outbound.type = vpn_engine::protocol::CstpFrameType::data;
  outbound.payload = packet;

  std::vector<std::uint8_t> outbound_wire;
  vpn_engine::ValidationResult encoded =
      vpn_engine::protocol::encode_cstp_frame(outbound, &outbound_wire);
  if (!encoded.ok)
    return encoded;

  vpn_engine::protocol::CstpFrame decoded;
  vpn_engine::ValidationResult decoded_result =
      vpn_engine::protocol::decode_cstp_frame(outbound_wire, &decoded);
  if (!decoded_result.ok)
    return decoded_result;
  if (decoded.type != vpn_engine::protocol::CstpFrameType::data)
    return invalid("cstp_unexpected_frame", "expected data frame");

  ++data_frames_received_;
  ++data_frames_on_transport_;

  const bool should_close =
      options_.close_on_data_frame_number != 0 &&
      data_frames_on_transport_ == options_.close_on_data_frame_number &&
      (!options_.close_only_once || !close_triggered_);
  if (should_close) {
    close_triggered_ = true;
    closed_ = true;
    return invalid("transport_closed", "server closed during packet loop");
  }

  vpn_engine::protocol::CstpFrame echo;
  echo.type = vpn_engine::protocol::CstpFrameType::data;
  echo.payload = decoded.payload;

  std::vector<std::uint8_t> echo_wire;
  vpn_engine::ValidationResult echo_encoded =
      vpn_engine::protocol::encode_cstp_frame(echo, &echo_wire);
  if (!echo_encoded.ok)
    return echo_encoded;

  vpn_engine::protocol::CstpFrame echo_decoded;
  vpn_engine::ValidationResult echo_decoded_result =
      vpn_engine::protocol::decode_cstp_frame(echo_wire, &echo_decoded);
  if (!echo_decoded_result.ok)
    return echo_decoded_result;
  if (echo_decoded.type != vpn_engine::protocol::CstpFrameType::data)
    return invalid("cstp_unexpected_frame", "expected echoed data frame");

  *echoed_packet = std::move(echo_decoded.payload);
  return vpn_engine::ValidationResult{};
}

void FakeAnyConnectServer::reset_transport() {
  closed_ = false;
  data_frames_on_transport_ = 0;
}

bool FakeAnyConnectServer::closed() const { return closed_; }

int FakeAnyConnectServer::auth_attempts() const { return auth_attempts_; }

int FakeAnyConnectServer::cstp_connects() const { return cstp_connects_; }

int FakeAnyConnectServer::data_frames_received() const {
  return data_frames_received_;
}

void RecordingEventSink::emit(const vpn_engine::VpnEngineEvent &event) {
  events_.push_back(event);
}

const std::vector<vpn_engine::VpnEngineEvent> &
RecordingEventSink::events() const {
  return events_;
}

void RecordingEventSink::clear() { events_.clear(); }

ScriptedPacketDevice::ScriptedPacketDevice(
    std::vector<std::vector<std::uint8_t>> packets)
    : packets_(packets.begin(), packets.end()) {}

vpn_engine::ValidationResult
ScriptedPacketDevice::open(const vpn_engine::TunnelMetadata &metadata) {
  last_open_metadata_ = metadata;
  open_ = true;
  ++open_count_;
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult
ScriptedPacketDevice::read_packet(std::vector<std::uint8_t> *packet) {
  if (!packet)
    return invalid("packet_null_out", "packet output must not be null");
  if (!open_)
    return invalid("packet_device_closed", "packet device is closed");
  if (packets_.empty())
    return invalid("packet_device_empty", "no packet is queued");

  *packet = std::move(packets_.front());
  packets_.pop_front();
  return vpn_engine::ValidationResult{};
}

vpn_engine::ValidationResult ScriptedPacketDevice::write_packet(
    const std::vector<std::uint8_t> &packet) {
  if (!open_)
    return invalid("packet_device_closed", "packet device is closed");

  written_packets_.push_back(packet);
  return vpn_engine::ValidationResult{};
}

void ScriptedPacketDevice::close() {
  if (open_) {
    open_ = false;
    ++close_count_;
  }
}

const std::vector<std::vector<std::uint8_t>> &
ScriptedPacketDevice::written_packets() const {
  return written_packets_;
}

const vpn_engine::TunnelMetadata &
ScriptedPacketDevice::last_open_metadata() const {
  return last_open_metadata_;
}

int ScriptedPacketDevice::open_count() const { return open_count_; }

int ScriptedPacketDevice::close_count() const { return close_count_; }

bool ScriptedPacketDevice::is_open() const { return open_; }

FakeAnyConnectRunResult run_fake_anyconnect_session(
    FakeAnyConnectServer &server, vpn_engine::PacketDevice &device,
    vpn_engine::EventSink &events, const FakeAnyConnectRunOptions &options) {
  FakeAnyConnectRunResult run;

  run.result = connect_once(server, device, events, options, &run.state);
  if (!run.result.ok)
    return run;

  while (true) {
    std::vector<std::uint8_t> packet;
    vpn_engine::ValidationResult read = device.read_packet(&packet);
    if (!read.ok) {
      if (read.code == "packet_device_empty") {
        run.result = vpn_engine::ValidationResult{};
        return run;
      }

      run.state.failed(read.code, read.message);
      device.close();
      run.result = read;
      return run;
    }

    std::vector<std::uint8_t> echoed_packet;
    vpn_engine::ValidationResult exchanged =
        server.exchange_packet(packet, &echoed_packet);
    if (!exchanged.ok) {
      emit_event(events, "transport.closed", "error", exchanged.message,
                 {{"code", exchanged.code}});

      const bool can_reconnect =
          exchanged.code == "transport_closed" && options.auto_reconnect &&
          run.reconnects < options.max_reconnects;
      if (can_reconnect) {
        run.state.phase = vpn_engine::SessionPhase::reconnecting;
        ++run.reconnects;
        emit_event(events, "reconnect.started", "info",
                   "reconnect started",
                   {{"attempt", std::to_string(run.reconnects)}});

        device.close();
        server.reset_transport();

        vpn_engine::ValidationResult reconnected =
            connect_once(server, device, events, options, &run.state);
        if (!reconnected.ok) {
          emit_event(events, "reconnect.failed", "error",
                     reconnected.message, {{"code", reconnected.code}});
          run.result = reconnected;
          return run;
        }

        emit_event(events, "reconnect.succeeded", "info",
                   "reconnect succeeded",
                   {{"attempt", std::to_string(run.reconnects)}});
        continue;
      }

      run.state.failed(exchanged.code, exchanged.message);
      device.close();
      run.result = exchanged;
      return run;
    }

    vpn_engine::ValidationResult written = device.write_packet(echoed_packet);
    if (!written.ok) {
      run.state.failed(written.code, written.message);
      device.close();
      run.result = written;
      return run;
    }

    emit_event(events, "packet.echo", "info", "packet echoed",
               {{"bytes", std::to_string(echoed_packet.size())}});
  }
}

} // namespace support
} // namespace tests
} // namespace ecnuvpn
