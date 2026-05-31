#pragma once

#include "vpn_engine/protocol/session.hpp"
#include "vpn_engine/protocol/tls_stream.hpp"

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

struct HttpResponse;

class ProductionProtocolTransport final : public ProtocolTransport {
public:
  explicit ProductionProtocolTransport(
      TlsStream *stream, std::string client_hostname = "ecnu-vpn");
  explicit ProductionProtocolTransport(
      std::unique_ptr<TlsStream> stream,
      std::string client_hostname = "ecnu-vpn");

  AuthResult authenticate(const ProtocolSessionOptions &options) override;
  ValidationResult connect_cstp(const std::string &cookie,
                                TunnelMetadata *metadata) override;
  ValidationResult
  exchange_packet(const std::vector<std::uint8_t> &packet,
                  std::vector<std::uint8_t> *response_packet) override;

  void disconnect() override;
  void reset_for_reconnect() override;

private:
  ValidationResult read_more();
  ValidationResult read_http_response(bool leave_body_in_buffer,
                                      HttpResponse *response);

  std::unique_ptr<TlsStream> owned_stream_;
  TlsStream *stream_ = nullptr;
  std::string client_hostname_;

  ParsedVpnUrl server_;
  std::string useragent_;
  std::string current_password_;
  std::string current_password_form_encoded_;
  AuthCookieJar cookies_;
  std::vector<std::uint8_t> read_buffer_;
  bool stream_connected_ = false;
  bool cstp_connected_ = false;
};

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
