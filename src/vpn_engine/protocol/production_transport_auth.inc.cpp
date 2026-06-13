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
    ValidationResult written =
        stream_->write_all(to_bytes(make_login_get_request(server_, useragent_)));
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
                                "unexpected HTTP status in login preflight",
                                current_password_,
                                current_password_form_encoded_,
                                cookies_.header());
  }

  cookies_.collect_from_response(preflight);

  {
    ValidationResult written = stream_->write_all(to_bytes(make_login_post_request(
        server_, useragent_, options.username, current_password_form_encoded_,
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

  AuthResult parsed = parse_auth_response(submitted);
  if (!parsed.ok) {
    parsed.error_message =
        sanitized_message(parsed.error_message, current_password_,
                          current_password_form_encoded_, cookies_.header());
    return parsed;
  }

  if (cookies_.empty()) {
    return sanitized_auth_error("protocol_error",
                                "missing Set-Cookie in auth response",
                                current_password_,
                                current_password_form_encoded_,
                                cookies_.header());
  }

  AuthResult result;
  result.ok = true;
  result.cookie = cookies_.header();
  return result;
}

