#pragma once

#include "vpn_engine/engine.hpp"
#include "vpn_engine/protocol/http.hpp"
#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

enum class CstpFrameType { data, keepalive, dpd_request, dpd_response, disconnect };

struct CstpFrame {
  CstpFrameType type = CstpFrameType::data;
  std::vector<std::uint8_t> payload;
};

// A minimal C++17 byte reader to support stream-style decoding.
//
// Wire format note: this library encodes/decodes the AnyConnect-compatible
// CSTP data-channel record framing as described by the public CSTP/AnyConnect
// protocol description (the 8-byte "STF" record header). It is written
// independently for interoperability and does not reproduce any third-party
// implementation source.
class ByteReader {
public:
  ByteReader(const std::uint8_t *data, std::size_t size);
  explicit ByteReader(const std::vector<std::uint8_t> &bytes);

  std::size_t position() const;
  std::size_t remaining() const;
  const std::uint8_t *current_data() const;

  void set_position(std::size_t pos);

  bool can_read(std::size_t n) const;

  ValidationResult read_u8(std::uint8_t *out);
  ValidationResult read_be_u16(std::uint16_t *out);
  ValidationResult read_be_u32(std::uint32_t *out);
  ValidationResult read_payload(std::size_t n, std::vector<std::uint8_t> *out);

private:
  const std::uint8_t *data_ = nullptr;
  std::size_t size_ = 0;
  std::size_t pos_ = 0;
};

// Parse CSTP CONNECT response headers into the tunnel metadata used by the
// native engine boundary.
ValidationResult parse_cstp_headers(const HttpResponse &response,
                                   TunnelMetadata *metadata);

// Encode a single CSTP frame into bytes.
ValidationResult encode_cstp_frame(const CstpFrame &frame,
                                  std::vector<std::uint8_t> *out);

// Decode a single CSTP frame from the reader. Consumes exactly one complete
// frame on success and leaves any trailing bytes in the reader.
ValidationResult decode_cstp_frame(ByteReader *reader, CstpFrame *out);

// Decode a single CSTP frame from bytes.
//
// This overload expects the input to contain exactly one complete frame and
// rejects trailing bytes.
ValidationResult decode_cstp_frame(const std::vector<std::uint8_t> &bytes,
                                  CstpFrame *out);

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
