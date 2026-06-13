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
