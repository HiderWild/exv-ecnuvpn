#pragma once

#include "vpn_engine/protocol/session.hpp"
#include "vpn_engine/protocol/tls_stream.hpp"

#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace exv {
namespace vpn_engine {
namespace protocol {

struct HttpResponse;

class ProductionProtocolTransport final : public ProtocolTransport {
public:
  explicit ProductionProtocolTransport(
      TlsStream *stream, std::string client_hostname = "exv");
  explicit ProductionProtocolTransport(
      std::unique_ptr<TlsStream> stream,
      std::string client_hostname = "exv");

  AuthResult authenticate(const ProtocolSessionOptions &options) override;
  ValidationResult connect_cstp(const std::string &cookie,
                                TunnelMetadata *metadata) override;
  ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) override;
  ValidationResult send_control(InboundFrameKind kind) override;
  ValidationResult receive_frame(InboundFrame *out) override;

  void disconnect() override;
  void reset_for_reconnect() override;

private:
  ValidationResult read_more();
  ValidationResult read_http_response(bool leave_body_in_buffer,
                                      HttpResponse *response);
  ValidationResult write_frame_locked(const std::vector<std::uint8_t> &wire);

  std::unique_ptr<TlsStream> owned_stream_;
  TlsStream *stream_ = nullptr;
  std::string client_hostname_;

  ParsedVpnUrl server_;
  std::string useragent_;
  std::string current_password_;
  std::string current_password_form_encoded_;
  std::string auth_cookie_;
  int requested_mtu_ = 1290;
  AuthCookieJar cookies_;
  std::vector<std::uint8_t> read_buffer_;
  bool stream_connected_ = false;
  bool cstp_connected_ = false;
  bool dtls_disabled_ = true;

  // Serializes outbound writes (send_packet / send_control / disconnect frame)
  // so the inbound read thread and outbound write thread can share one stream.
  std::mutex write_mutex_;
};

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
