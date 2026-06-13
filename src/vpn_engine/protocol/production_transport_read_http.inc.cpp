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
