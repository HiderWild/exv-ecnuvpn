#include "vpn_engine/protocol/cstp.hpp"

#include <cctype>
#include <cstdint>
#include <limits>
#include <string>
#include <string_view>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

constexpr std::uint32_t kMaxFramePayloadBytes = 1024 * 1024; // 1 MiB
constexpr std::size_t kMaxRouteCount = 256;
constexpr std::size_t kMaxHeaderValueBytes = 512;

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult r;
  r.ok = false;
  r.code = std::move(code);
  r.message = std::move(message);
  return r;
}

ValidationResult incomplete_frame() {
  return invalid("cstp_frame_incomplete", "insufficient bytes to decode a complete frame");
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

ValidationResult parse_u32_decimal(std::string_view s, std::uint32_t *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  s = trim_ascii(s);
  if (s.empty())
    return invalid("cstp_invalid_number", "empty numeric value");

  std::uint64_t v = 0;
  for (unsigned char ch : s) {
    if (ch < '0' || ch > '9')
      return invalid("cstp_invalid_number", "numeric value contains non-digit");
    v = v * 10 + (ch - '0');
    if (v > std::numeric_limits<std::uint32_t>::max())
      return invalid("cstp_invalid_number", "numeric value out of range");
  }

  *out = static_cast<std::uint32_t>(v);
  return ValidationResult{};
}

ValidationResult validate_ipv4_dotted(std::string_view s) {
  s = trim_ascii(s);
  if (s.empty())
    return invalid("cstp_invalid_ipv4", "IPv4 value is empty");
  if (s.size() > 64)
    return invalid("cstp_invalid_ipv4", "IPv4 value is too long");

  int octets = 0;
  while (!s.empty()) {
    if (octets >= 4)
      return invalid("cstp_invalid_ipv4", "too many octets");

    std::size_t dot = s.find('.');
    std::string_view part = (dot == std::string_view::npos) ? s : s.substr(0, dot);
    s = (dot == std::string_view::npos) ? std::string_view() : s.substr(dot + 1);

    part = trim_ascii(part);
    if (part.empty() || part.size() > 3)
      return invalid("cstp_invalid_ipv4", "invalid octet");

    std::uint32_t value = 0;
    for (unsigned char ch : part) {
      if (ch < '0' || ch > '9')
        return invalid("cstp_invalid_ipv4", "octet contains non-digit");
      value = value * 10 + (ch - '0');
      if (value > 255)
        return invalid("cstp_invalid_ipv4", "octet out of range");
    }

    ++octets;
  }

  if (octets != 4)
    return invalid("cstp_invalid_ipv4", "IPv4 must have 4 octets");

  return ValidationResult{};
}

ValidationResult append_header_values(const HttpResponse &response,
                                     const char *header_name,
                                     std::vector<std::string> *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  const auto *values = response.header_values_ci(header_name);
  if (!values)
    return ValidationResult{};

  for (const std::string &raw : *values) {
    std::string_view v = trim_ascii(raw);
    if (v.empty())
      continue;
    if (v.size() > kMaxHeaderValueBytes)
      return invalid("cstp_header_too_large", std::string(header_name) + " value too large");
    out->push_back(std::string(v));
    if (out->size() > kMaxRouteCount)
      return invalid("cstp_too_many_routes", "too many route headers");
  }

  return ValidationResult{};
}

std::uint32_t read_be_u32_raw(const std::uint8_t *p) {
  return (static_cast<std::uint32_t>(p[0]) << 24) |
         (static_cast<std::uint32_t>(p[1]) << 16) |
         (static_cast<std::uint32_t>(p[2]) << 8) |
         (static_cast<std::uint32_t>(p[3]) << 0);
}

} // namespace

ByteReader::ByteReader(const std::uint8_t *data, std::size_t size)
    : data_(data), size_(size), pos_(0) {}

ByteReader::ByteReader(const std::vector<std::uint8_t> &bytes)
    : ByteReader(bytes.data(), bytes.size()) {}

std::size_t ByteReader::position() const { return pos_; }

std::size_t ByteReader::remaining() const { return (pos_ <= size_) ? (size_ - pos_) : 0; }

const std::uint8_t *ByteReader::current_data() const {
  if (!data_ || pos_ > size_)
    return nullptr;
  return data_ + pos_;
}

void ByteReader::set_position(std::size_t pos) {
  pos_ = (pos <= size_) ? pos : size_;
}

bool ByteReader::can_read(std::size_t n) const { return remaining() >= n; }

ValidationResult ByteReader::read_u8(std::uint8_t *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");
  if (!can_read(1))
    return incomplete_frame();
  *out = data_[pos_];
  ++pos_;
  return ValidationResult{};
}

ValidationResult ByteReader::read_be_u32(std::uint32_t *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");
  if (!can_read(4))
    return incomplete_frame();
  *out = read_be_u32_raw(&data_[pos_]);
  pos_ += 4;
  return ValidationResult{};
}

ValidationResult ByteReader::read_payload(std::size_t n, std::vector<std::uint8_t> *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");
  if (!can_read(n))
    return incomplete_frame();
  out->assign(data_ + pos_, data_ + pos_ + n);
  pos_ += n;
  return ValidationResult{};
}

namespace {

void write_be_u32(std::uint32_t v, std::vector<std::uint8_t> *out) {
  out->push_back(static_cast<std::uint8_t>((v >> 24) & 0xff));
  out->push_back(static_cast<std::uint8_t>((v >> 16) & 0xff));
  out->push_back(static_cast<std::uint8_t>((v >> 8) & 0xff));
  out->push_back(static_cast<std::uint8_t>((v >> 0) & 0xff));
}

ValidationResult type_to_wire(CstpFrameType t, std::uint8_t *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  // These type tags are fake-server/test-harness framing only. They are not
  // verified production CSTP wire-format tags.
  switch (t) {
  case CstpFrameType::data:
    *out = 0;
    return ValidationResult{};
  case CstpFrameType::keepalive:
    *out = 1;
    return ValidationResult{};
  case CstpFrameType::dpd_request:
    *out = 2;
    return ValidationResult{};
  case CstpFrameType::dpd_response:
    *out = 3;
    return ValidationResult{};
  case CstpFrameType::disconnect:
    *out = 4;
    return ValidationResult{};
  }

  return invalid("cstp_unknown_type", "unknown frame type");
}

ValidationResult wire_to_type(std::uint8_t t, CstpFrameType *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  // These type tags are fake-server/test-harness framing only. They are not
  // verified production CSTP wire-format tags.
  switch (t) {
  case 0:
    *out = CstpFrameType::data;
    return ValidationResult{};
  case 1:
    *out = CstpFrameType::keepalive;
    return ValidationResult{};
  case 2:
    *out = CstpFrameType::dpd_request;
    return ValidationResult{};
  case 3:
    *out = CstpFrameType::dpd_response;
    return ValidationResult{};
  case 4:
    *out = CstpFrameType::disconnect;
    return ValidationResult{};
  default:
    return invalid("cstp_unknown_type", "unknown frame type tag");
  }
}

} // namespace

ValidationResult parse_cstp_headers(const HttpResponse &response,
                                   TunnelMetadata *metadata) {
  if (!metadata)
    return invalid("cstp_null_metadata", "metadata output must not be null");

  if (response.status < 200 || response.status > 299)
    return invalid("cstp_connect_failed", "CSTP CONNECT response status is not 2xx");

  const std::string *addr = response.header_ci("X-CSTP-Address");
  const std::string *mask = response.header_ci("X-CSTP-Netmask");
  const std::string *mtu = response.header_ci("X-CSTP-MTU");

  if (!addr || trim_ascii(*addr).empty())
    return invalid("cstp_missing_address", "missing X-CSTP-Address header");
  if (!mask || trim_ascii(*mask).empty())
    return invalid("cstp_missing_netmask", "missing X-CSTP-Netmask header");
  if (!mtu || trim_ascii(*mtu).empty())
    return invalid("cstp_missing_mtu", "missing X-CSTP-MTU header");

  {
    ValidationResult v = validate_ipv4_dotted(*addr);
    if (!v.ok)
      return v;
  }
  {
    ValidationResult v = validate_ipv4_dotted(*mask);
    if (!v.ok)
      return v;
  }

  std::uint32_t mtu_value = 0;
  {
    ValidationResult v = parse_u32_decimal(*mtu, &mtu_value);
    if (!v.ok)
      return invalid("cstp_invalid_mtu", "invalid X-CSTP-MTU value");
  }
  if (mtu_value < 576 || mtu_value > 1500)
    return invalid("cstp_invalid_mtu", "X-CSTP-MTU out of allowed range");

  metadata->internal_ip4_address = std::string(trim_ascii(*addr));
  metadata->internal_ip4_netmask = std::string(trim_ascii(*mask));
  metadata->mtu = static_cast<int>(mtu_value);

  metadata->routes.clear();
  metadata->server_bypass_ips.clear();

  {
    ValidationResult v = append_header_values(response, "X-CSTP-Split-Include",
                                             &metadata->routes);
    if (!v.ok)
      return v;
  }
  {
    ValidationResult v = append_header_values(response, "X-CSTP-Bypass-Route",
                                             &metadata->server_bypass_ips);
    if (!v.ok)
      return v;
  }

  return ValidationResult{};
}

ValidationResult encode_cstp_frame(const CstpFrame &frame,
                                  std::vector<std::uint8_t> *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  if (frame.payload.size() > kMaxFramePayloadBytes)
    return invalid("cstp_frame_oversized", "payload exceeds maximum size");

  std::uint8_t type_tag = 0;
  {
    ValidationResult v = type_to_wire(frame.type, &type_tag);
    if (!v.ok)
      return v;
  }

  std::vector<std::uint8_t> encoded;
  encoded.reserve(1 + 4 + frame.payload.size());
  encoded.push_back(type_tag);
  write_be_u32(static_cast<std::uint32_t>(frame.payload.size()), &encoded);
  encoded.insert(encoded.end(), frame.payload.begin(), frame.payload.end());

  *out = std::move(encoded);
  return ValidationResult{};
}

ValidationResult decode_cstp_frame(ByteReader *reader, CstpFrame *out) {
  if (!reader)
    return invalid("cstp_null_reader", "reader must not be null");
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  // Wire format (fake-server/test-harness framing only; NOT verified production CSTP):
  // [type_tag:1][payload_len_be:4][payload:payload_len]
  const std::size_t start_pos = reader->position();

  std::uint8_t type_tag = 0;
  {
    ValidationResult v = reader->read_u8(&type_tag);
    if (!v.ok) {
      reader->set_position(start_pos);
      return v;
    }
  }

  CstpFrameType type;
  {
    ValidationResult v = wire_to_type(type_tag, &type);
    if (!v.ok) {
      reader->set_position(start_pos);
      return v;
    }
  }

  std::uint32_t payload_len = 0;
  {
    ValidationResult v = reader->read_be_u32(&payload_len);
    if (!v.ok) {
      reader->set_position(start_pos);
      return v;
    }
  }

  if (payload_len > kMaxFramePayloadBytes) {
    reader->set_position(start_pos);
    return invalid("cstp_frame_oversized", "frame payload exceeds maximum size");
  }

  CstpFrame decoded;
  decoded.type = type;

  {
    ValidationResult v = reader->read_payload(static_cast<std::size_t>(payload_len),
                                             &decoded.payload);
    if (!v.ok) {
      reader->set_position(start_pos);
      return v;
    }
  }

  *out = std::move(decoded);
  return ValidationResult{};
}

ValidationResult decode_cstp_frame(const std::vector<std::uint8_t> &bytes,
                                  CstpFrame *out) {
  if (!out)
    return invalid("cstp_null_out", "output pointer is null");

  ByteReader reader(bytes);
  ValidationResult v = decode_cstp_frame(&reader, out);
  if (!v.ok)
    return v;

  if (reader.remaining() != 0)
    return invalid("cstp_frame_length_mismatch", "trailing bytes after frame");

  return ValidationResult{};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
