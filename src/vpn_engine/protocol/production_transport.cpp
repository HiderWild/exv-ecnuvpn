#include "vpn_engine/protocol/production_transport.hpp"

#include "vpn_engine/protocol/aggregate_auth.hpp"
#include "vpn_engine/protocol/cstp.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <sstream>
#include <string_view>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

constexpr const char *kAggregateAuthPath = "/";
constexpr const char *kCstpPath = "/CSCOSSLC/tunnel";
constexpr const char *kDefaultUserAgent = "ECNU-VPN Native";
constexpr std::size_t kMaxHttpHeaderBytes = 64 * 1024;
constexpr std::size_t kMaxHttpBodyBytes = 16 * 1024 * 1024;
// Begin inlined from vpn_engine/protocol/production_transport_redaction include-unit
ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

AuthResult auth_error(std::string code, std::string message) {
  AuthResult result;
  result.ok = false;
  result.error_code = std::move(code);
  result.error_message = std::move(message);
  return result;
}

bool is_ascii_space(unsigned char ch) {
  return ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n';
}

std::string_view trim_ascii(std::string_view s) {
  while (!s.empty() && is_ascii_space(static_cast<unsigned char>(s.front())))
    s.remove_prefix(1);
  while (!s.empty() && is_ascii_space(static_cast<unsigned char>(s.back())))
    s.remove_suffix(1);
  return s;
}

void replace_all(std::string *text, const std::string &needle,
                 const std::string &replacement) {
  if (!text || needle.empty())
    return;

  std::size_t pos = 0;
  while ((pos = text->find(needle, pos)) != std::string::npos) {
    text->replace(pos, needle.size(), replacement);
    pos += replacement.size();
  }
}

void redact_cookie_header(std::string *text, std::string_view cookie_header) {
  if (!text)
    return;

  cookie_header = trim_ascii(cookie_header);
  if (cookie_header.empty())
    return;

  const std::string full(cookie_header);
  replace_all(text, full, "[REDACTED_COOKIE]");

  while (!cookie_header.empty()) {
    const std::size_t semi = cookie_header.find(';');
    std::string_view pair =
        semi == std::string_view::npos ? cookie_header
                                       : cookie_header.substr(0, semi);
    cookie_header =
        semi == std::string_view::npos ? std::string_view()
                                       : cookie_header.substr(semi + 1);

    pair = trim_ascii(pair);
    const std::size_t eq = pair.find('=');
    if (eq == std::string_view::npos || eq + 1 >= pair.size())
      continue;

    std::string_view value = trim_ascii(pair.substr(eq + 1));
    if (!value.empty())
      replace_all(text, std::string(value), "[REDACTED_COOKIE_VALUE]");
  }
}

std::string sanitized_message(
    const std::string &message, const std::string &password,
    const std::string &encoded_password, const std::string &cookie_header,
    const std::string &extra_cookie_header = "") {
  std::string out = message;
  replace_all(&out, password, "[REDACTED_PASSWORD]");
  replace_all(&out, encoded_password, "[REDACTED_PASSWORD]");
  redact_cookie_header(&out, cookie_header);
  redact_cookie_header(&out, extra_cookie_header);
  return out;
}

ValidationResult sanitized_result(
    const ValidationResult &result, const std::string &password,
    const std::string &encoded_password, const std::string &cookie_header,
    const std::string &extra_cookie_header = "") {
  if (result.ok)
    return result;

  return invalid(result.code,
                 sanitized_message(result.message, password, encoded_password,
                                   cookie_header, extra_cookie_header));
}

AuthResult sanitized_auth_error(
    const std::string &code, const std::string &message,
    const std::string &password, const std::string &encoded_password,
    const std::string &cookie_header) {
  return auth_error(code,
                    sanitized_message(message, password, encoded_password,
                                      cookie_header));
}
// End inlined from vpn_engine/protocol/production_transport_redaction include-unit
// Begin inlined from vpn_engine/protocol/production_transport_requests include-unit
std::string useragent_or_default(const std::string &useragent) {
  return useragent.empty() ? std::string(kDefaultUserAgent) : useragent;
}

std::string host_header(const ParsedVpnUrl &server) {
  std::string host = server.host;
  if (host.find(':') != std::string::npos &&
      (host.empty() || host.front() != '[')) {
    host = "[" + host + "]";
  }

  if (server.port != 443)
    host += ":" + std::to_string(server.port);
  return host;
}

std::string form_url_encode(const std::string &value) {
  static constexpr char kHex[] = "0123456789ABCDEF";

  std::string out;
  out.reserve(value.size());
  for (unsigned char ch : value) {
    const bool unreserved =
        std::isalnum(ch) || ch == '-' || ch == '_' || ch == '.' || ch == '*';
    if (unreserved) {
      out.push_back(static_cast<char>(ch));
    } else if (ch == ' ') {
      out.push_back('+');
    } else {
      out.push_back('%');
      out.push_back(kHex[(ch >> 4) & 0x0f]);
      out.push_back(kHex[ch & 0x0f]);
    }
  }
  return out;
}

std::vector<std::uint8_t> to_bytes(const std::string &text) {
  return std::vector<std::uint8_t>(text.begin(), text.end());
}

std::string make_aggregate_auth_post_request(const ParsedVpnUrl &server,
                                             const std::string &useragent,
                                             const std::string &body,
                                             const std::string &cookie_header) {
  std::ostringstream out;
  out << "POST " << kAggregateAuthPath << " HTTP/1.1\r\n";
  out << "Host: " << host_header(server) << "\r\n";
  out << "User-Agent: " << useragent_or_default(useragent) << "\r\n";
  out << "X-Transcend-Version: 1\r\n";
  out << "X-Aggregate-Auth: 1\r\n";
  out << "Accept: */*\r\n";
  out << "Accept-Encoding: identity\r\n";
  out << "Content-Type: text/xml; charset=utf-8\r\n";
  out << "Content-Length: " << body.size() << "\r\n";
  if (!cookie_header.empty())
    out << "Cookie: " << cookie_header << "\r\n";
  out << "Connection: keep-alive\r\n";
  out << "\r\n";
  out << body;
  return out.str();
}

std::string make_cstp_connect_request(const ParsedVpnUrl &server,
                                      const std::string &useragent,
                                      const std::string &client_hostname,
                                      const std::string &cookie_header) {
  std::ostringstream out;
  out << "CONNECT " << kCstpPath << " HTTP/1.1\r\n";
  out << "Host: " << host_header(server) << "\r\n";
  out << "User-Agent: " << useragent_or_default(useragent) << "\r\n";
  out << "Cookie: " << cookie_header << "\r\n";
  out << "X-CSTP-Version: 1\r\n";
  out << "X-CSTP-Hostname: " << client_hostname << "\r\n";
  out << "X-CSTP-Address-Type: IPv4\r\n";
  out << "\r\n";
  return out.str();
}
// End inlined from vpn_engine/protocol/production_transport_requests include-unit
// Begin inlined from vpn_engine/protocol/production_transport_response_parse include-unit
bool find_header_terminator(const std::vector<std::uint8_t> &bytes,
                            std::size_t *header_end,
                            std::size_t *delimiter_size) {
  if (!header_end || !delimiter_size)
    return false;

  for (std::size_t i = 0; i + 3 < bytes.size(); ++i) {
    if (bytes[i] == '\r' && bytes[i + 1] == '\n' && bytes[i + 2] == '\r' &&
        bytes[i + 3] == '\n') {
      *header_end = i;
      *delimiter_size = 4;
      return true;
    }
  }

  for (std::size_t i = 0; i + 1 < bytes.size(); ++i) {
    if (bytes[i] == '\n' && bytes[i + 1] == '\n') {
      *header_end = i;
      *delimiter_size = 2;
      return true;
    }
  }

  return false;
}

ValidationResult parse_content_length(const HttpResponse &response,
                                      bool *present, std::size_t *out) {
  if (!present)
    return invalid("http_invalid", "content length presence output is null");
  if (!out)
    return invalid("http_invalid", "content length output is null");

  const std::string *value = response.header_ci("content-length");
  if (!value) {
    *present = false;
    *out = 0;
    return {};
  }

  *present = true;

  std::string_view s = trim_ascii(*value);
  if (s.empty())
    return invalid("http_invalid", "empty Content-Length header");

  std::size_t parsed = 0;
  for (unsigned char ch : s) {
    if (ch < '0' || ch > '9')
      return invalid("http_invalid", "invalid Content-Length header");
    const std::size_t digit = static_cast<std::size_t>(ch - '0');
    if (parsed >
        (std::numeric_limits<std::size_t>::max() - digit) /
            static_cast<std::size_t>(10)) {
      return invalid("http_invalid", "Content-Length header is too large");
    }
    parsed = parsed * 10 + digit;
  }

  *out = parsed;
  return {};
}
// End inlined from vpn_engine/protocol/production_transport_response_parse include-unit
} // namespace
// Begin inlined from vpn_engine/protocol/production_transport_auth include-unit
ProductionProtocolTransport::ProductionProtocolTransport(
    TlsStream *stream, std::string client_hostname)
    : stream_(stream), client_hostname_(std::move(client_hostname)) {}

ProductionProtocolTransport::ProductionProtocolTransport(
    std::unique_ptr<TlsStream> stream, std::string client_hostname)
    : owned_stream_(std::move(stream)), stream_(owned_stream_.get()),
      client_hostname_(std::move(client_hostname)) {}

AuthResult ProductionProtocolTransport::authenticate(
    const ProtocolSessionOptions &options) {
  server_ = options.server;
  useragent_ = options.useragent;
  current_password_ = options.password;
  current_password_form_encoded_ = form_url_encode(options.password);
  cookies_.clear();
  read_buffer_.clear();
  cstp_connected_ = false;

  if (!stream_) {
    return auth_error("transport_missing", "TLS stream is not configured");
  }

  if (!stream_connected_) {
    TlsEndpoint endpoint;
    endpoint.host = server_.host;
    endpoint.port = server_.port;
    endpoint.sni_host = server_.host;

    ValidationResult connected = stream_->connect(endpoint);
    if (!connected.ok) {
      return sanitized_auth_error(connected.code, connected.message,
                                  current_password_,
                                  current_password_form_encoded_,
                                  cookies_.header());
    }
    stream_connected_ = true;
  }

  {
    const std::string body = make_aggregate_auth_init_xml();
    ValidationResult written = stream_->write_all(to_bytes(
        make_aggregate_auth_post_request(server_, useragent_, body, {})));
    if (!written.ok) {
      ValidationResult sanitized =
          sanitized_result(written, current_password_,
                           current_password_form_encoded_, cookies_.header());
      return auth_error(sanitized.code, sanitized.message);
    }
  }

  HttpResponse preflight;
  {
    ValidationResult read = read_http_response(false, &preflight);
    if (!read.ok) {
      return auth_error(read.code, read.message);
    }
  }

  if (preflight.status < 200 || preflight.status >= 300) {
    AuthResult parsed = parse_auth_response(preflight);
    if (!parsed.ok) {
      parsed.error_message =
          sanitized_message(parsed.error_message, current_password_,
                            current_password_form_encoded_, cookies_.header());
      return parsed;
    }
    return sanitized_auth_error("protocol_error",
                                "unexpected HTTP status in aggregate-auth init",
                                current_password_,
                                current_password_form_encoded_,
                                cookies_.header());
  }

  cookies_.collect_from_response(preflight);
  {
    AggregateAuthResult parsed_init =
        parse_aggregate_auth_response(preflight);
    if (parsed_init.ok) {
      AuthResult result;
      result.ok = true;
      result.cookie = parsed_init.cookie;
      return result;
    }
    if (parsed_init.error_code == "csd_required_unsupported" ||
        parsed_init.error_code == "unsupported_auth_flow" ||
        parsed_init.error_code == "auth_failed") {
      return sanitized_auth_error(parsed_init.error_code,
                                  parsed_init.error_message,
                                  current_password_,
                                  current_password_form_encoded_,
                                  cookies_.header());
    }
  }

  {
    const std::string body =
        make_aggregate_auth_reply_xml(options.username, options.password);
    ValidationResult written = stream_->write_all(to_bytes(
        make_aggregate_auth_post_request(server_, useragent_, body,
                                         cookies_.header())));
    if (!written.ok) {
      ValidationResult sanitized =
          sanitized_result(written, current_password_,
                           current_password_form_encoded_, cookies_.header());
      return auth_error(sanitized.code, sanitized.message);
    }
  }

  HttpResponse submitted;
  {
    ValidationResult read =
        read_http_response(false, &submitted);
    if (!read.ok) {
      return auth_error(read.code, read.message);
    }
  }

  cookies_.collect_from_response(submitted);

  AggregateAuthResult parsed = parse_aggregate_auth_response(submitted);
  if (!parsed.ok &&
      (parsed.error_code == "auth_challenge_required" ||
       parsed.error_code == "auth_group_required") &&
      options.auth_interaction_handler) {
    AuthInteractionRequest request;
    request.id = "auth-continuation-1";
    request.kind = parsed.prompt.kind;
    request.label = parsed.prompt.label;
    request.input_type = parsed.prompt.input_type;
    request.options = parsed.prompt.options;

    AuthInteractionResponse response = options.auth_interaction_handler(request);
    if (response.ok && !response.value.empty()) {
      const std::string group =
          parsed.prompt.kind == "group" ? response.value : std::string();
      const std::string secondary =
          parsed.prompt.kind == "challenge" ? response.value : std::string();
      const std::string body = make_aggregate_auth_reply_xml(
          options.username, options.password, group, secondary);
      ValidationResult written = stream_->write_all(to_bytes(
          make_aggregate_auth_post_request(server_, useragent_, body,
                                           cookies_.header())));
      if (!written.ok) {
        ValidationResult sanitized =
            sanitized_result(written, current_password_,
                             current_password_form_encoded_, cookies_.header());
        return auth_error(sanitized.code, sanitized.message);
      }

      HttpResponse continued;
      ValidationResult read = read_http_response(false, &continued);
      if (!read.ok)
        return auth_error(read.code, read.message);
      cookies_.collect_from_response(continued);
      parsed = parse_aggregate_auth_response(continued);
    }
  }

  if (!parsed.ok) {
    return sanitized_auth_error(parsed.error_code, parsed.error_message,
                                current_password_,
                                current_password_form_encoded_,
                                cookies_.header());
  }

  AuthResult result;
  result.ok = true;
  result.cookie = parsed.cookie;
  return result;
}
// End inlined from vpn_engine/protocol/production_transport_auth include-unit
// Begin inlined from vpn_engine/protocol/production_transport_cstp include-unit
ValidationResult
ProductionProtocolTransport::connect_cstp(const std::string &cookie,
                                          TunnelMetadata *metadata) {
  if (!metadata)
    return invalid("cstp_null_metadata", "metadata output must not be null");
  if (!stream_)
    return invalid("transport_missing", "TLS stream is not configured");
  if (!stream_connected_)
    return invalid("transport_closed", "TLS stream is not connected");

  const std::string effective_cookie = cookie.empty() ? cookies_.header() : cookie;
  if (effective_cookie.empty())
    return invalid("auth_cookie_missing", "CSTP connect requires auth cookie");

  ValidationResult written = stream_->write_all(to_bytes(make_cstp_connect_request(
      server_, useragent_, client_hostname_, effective_cookie)));
  if (!written.ok) {
    return sanitized_result(written, current_password_,
                            current_password_form_encoded_, cookies_.header(),
                            effective_cookie);
  }

  HttpResponse response;
  ValidationResult read =
      read_http_response(true, &response);
  if (!read.ok) {
    cstp_connected_ = false;
    read_buffer_.clear();
    return sanitized_result(read, current_password_,
                            current_password_form_encoded_, cookies_.header(),
                            effective_cookie);
  }

  ValidationResult parsed = parse_cstp_headers(response, metadata);
  if (!parsed.ok) {
    cstp_connected_ = false;
    read_buffer_.clear();
    return sanitized_result(parsed, current_password_,
                            current_password_form_encoded_, cookies_.header(),
                            effective_cookie);
  }

  cstp_connected_ = true;
  return {};
}

ValidationResult ProductionProtocolTransport::write_frame_locked(
    const std::vector<std::uint8_t> &wire) {
  const std::lock_guard<std::mutex> lock(write_mutex_);
  if (!stream_ || !stream_connected_ || !cstp_connected_)
    return invalid("transport_closed", "CSTP transport is not connected");

  ValidationResult written = stream_->write_all(wire);
  if (!written.ok) {
    return sanitized_result(written, current_password_,
                            current_password_form_encoded_, cookies_.header());
  }
  return {};
}

ValidationResult ProductionProtocolTransport::send_packet(
    const std::vector<std::uint8_t> &packet) {
  CstpFrame outbound;
  outbound.type = CstpFrameType::data;
  outbound.payload = packet;

  std::vector<std::uint8_t> wire;
  ValidationResult encoded = encode_cstp_frame(outbound, &wire);
  if (!encoded.ok)
    return encoded;

  return write_frame_locked(wire);
}

ValidationResult
ProductionProtocolTransport::send_control(InboundFrameKind kind) {
  CstpFrame outbound;
  switch (kind) {
  case InboundFrameKind::dpd_request:
    outbound.type = CstpFrameType::dpd_request;
    break;
  case InboundFrameKind::dpd_response:
    outbound.type = CstpFrameType::dpd_response;
    break;
  case InboundFrameKind::keepalive:
    outbound.type = CstpFrameType::keepalive;
    break;
  case InboundFrameKind::disconnect:
    outbound.type = CstpFrameType::disconnect;
    break;
  case InboundFrameKind::data:
  case InboundFrameKind::none:
  default:
    return invalid("cstp_control_invalid",
                   "send_control requires a control frame kind");
  }

  std::vector<std::uint8_t> wire;
  ValidationResult encoded = encode_cstp_frame(outbound, &wire);
  if (!encoded.ok)
    return encoded;

  return write_frame_locked(wire);
}

ValidationResult
ProductionProtocolTransport::receive_frame(InboundFrame *out) {
  if (!out)
    return invalid("packet_null_out", "inbound frame output must not be null");
  out->kind = InboundFrameKind::none;
  out->payload.clear();

  if (!stream_ || !stream_connected_ || !cstp_connected_)
    return invalid("transport_closed", "CSTP transport is not connected");

  while (true) {
    ByteReader reader(read_buffer_);
    CstpFrame inbound;
    ValidationResult decoded = decode_cstp_frame(&reader, &inbound);
    if (decoded.ok) {
      read_buffer_.erase(read_buffer_.begin(),
                         read_buffer_.begin() +
                             static_cast<std::ptrdiff_t>(reader.position()));

      switch (inbound.type) {
      case CstpFrameType::data:
        out->kind = InboundFrameKind::data;
        break;
      case CstpFrameType::keepalive:
        out->kind = InboundFrameKind::keepalive;
        break;
      case CstpFrameType::dpd_request:
        out->kind = InboundFrameKind::dpd_request;
        break;
      case CstpFrameType::dpd_response:
        out->kind = InboundFrameKind::dpd_response;
        break;
      case CstpFrameType::disconnect:
        out->kind = InboundFrameKind::disconnect;
        break;
      }
      out->payload = std::move(inbound.payload);
      return {};
    }

    if (decoded.code != "cstp_frame_incomplete") {
      return sanitized_result(decoded, current_password_,
                              current_password_form_encoded_, cookies_.header());
    }

    ValidationResult read = read_more();
    if (!read.ok)
      return read;
  }
}

void ProductionProtocolTransport::disconnect() {
  const std::lock_guard<std::mutex> lock(write_mutex_);
  if (stream_ && stream_connected_) {
    CstpFrame frame;
    frame.type = CstpFrameType::disconnect;

    std::vector<std::uint8_t> encoded;
    if (encode_cstp_frame(frame, &encoded).ok)
      (void)stream_->write_all(encoded);

    stream_->close();
  }

  stream_connected_ = false;
  cstp_connected_ = false;
  read_buffer_.clear();
  cookies_.clear();
  current_password_.clear();
  current_password_form_encoded_.clear();
}
// End inlined from vpn_engine/protocol/production_transport_cstp include-unit
// Begin inlined from vpn_engine/protocol/production_transport_read_http include-unit
void ProductionProtocolTransport::reset_for_reconnect() {
  const std::lock_guard<std::mutex> lock(write_mutex_);
  if (stream_ && stream_connected_)
    stream_->close();

  stream_connected_ = false;
  cstp_connected_ = false;
  read_buffer_.clear();
  cookies_.clear();
  current_password_.clear();
  current_password_form_encoded_.clear();
}

ValidationResult ProductionProtocolTransport::read_more() {
  std::vector<std::uint8_t> chunk;
  ValidationResult read = stream_->read_some(&chunk);
  if (!read.ok) {
    return sanitized_result(read, current_password_,
                            current_password_form_encoded_, cookies_.header());
  }
  if (chunk.empty())
    return invalid("transport_closed", "TLS stream closed during CSTP read");

  read_buffer_.insert(read_buffer_.end(), chunk.begin(), chunk.end());
  return {};
}

ValidationResult ProductionProtocolTransport::read_http_response(
    bool leave_body_in_buffer, HttpResponse *response) {
  if (!response)
    return invalid("http_invalid", "HTTP response output is null");

  std::size_t header_end = 0;
  std::size_t delimiter_size = 0;
  while (!find_header_terminator(read_buffer_, &header_end, &delimiter_size)) {
    if (read_buffer_.size() > kMaxHttpHeaderBytes) {
      return invalid("http_header_too_large",
                     "HTTP response header exceeds maximum size");
    }

    ValidationResult read = read_more();
    if (!read.ok)
      return read;
  }

  const std::size_t body_start = header_end + delimiter_size;
  if (body_start > kMaxHttpHeaderBytes) {
    return invalid("http_header_too_large",
                   "HTTP response header exceeds maximum size");
  }

  std::string header_only(read_buffer_.begin(),
                          read_buffer_.begin() +
                              static_cast<std::ptrdiff_t>(body_start));

  HttpResponse header_response;
  ValidationResult parsed_header =
      parse_http_response(header_only, &header_response);
  if (!parsed_header.ok)
    return parsed_header;

  if (leave_body_in_buffer) {
    *response = std::move(header_response);
    read_buffer_.erase(read_buffer_.begin(),
                       read_buffer_.begin() +
                           static_cast<std::ptrdiff_t>(body_start));
    return {};
  }

  std::size_t content_length = 0;
  bool has_content_length = false;
  ValidationResult length_result =
      parse_content_length(header_response, &has_content_length,
                           &content_length);
  if (!length_result.ok)
    return length_result;

  if (has_content_length) {
    if (content_length >
        std::numeric_limits<std::size_t>::max() - body_start) {
      return invalid("http_content_length_overflow",
                     "HTTP Content-Length overflows response buffer size");
    }
    if (content_length > kMaxHttpBodyBytes) {
      return invalid("http_body_too_large",
                     "HTTP response body exceeds maximum size");
    }

    const std::size_t target = body_start + content_length;
    while (read_buffer_.size() < target) {
      ValidationResult read = read_more();
      if (!read.ok)
        return read;
    }

    std::string raw(read_buffer_.begin(),
                    read_buffer_.begin() +
                        static_cast<std::ptrdiff_t>(target));
    ValidationResult parsed = parse_http_response(raw, response);
    if (!parsed.ok)
      return parsed;

    read_buffer_.erase(read_buffer_.begin(),
                       read_buffer_.begin() +
                           static_cast<std::ptrdiff_t>(target));
    return {};
  }

  if (read_buffer_.size() - body_start > kMaxHttpBodyBytes) {
    return invalid("http_body_too_large",
                   "HTTP response body exceeds maximum size");
  }

  std::string raw(read_buffer_.begin(), read_buffer_.end());
  ValidationResult parsed = parse_http_response(raw, response);
  if (!parsed.ok)
    return parsed;

  read_buffer_.clear();
  return {};
}
// End inlined from vpn_engine/protocol/production_transport_read_http include-unit
} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
