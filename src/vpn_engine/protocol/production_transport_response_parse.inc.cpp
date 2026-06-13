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

