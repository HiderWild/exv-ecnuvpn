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

std::string make_login_get_request(const ParsedVpnUrl &server,
                                   const std::string &useragent) {
  std::ostringstream out;
  out << "GET " << kLoginPath << " HTTP/1.1\r\n";
  out << "Host: " << host_header(server) << "\r\n";
  out << "User-Agent: " << useragent_or_default(useragent) << "\r\n";
  out << "Accept: text/html, */*\r\n";
  out << "Connection: keep-alive\r\n";
  out << "\r\n";
  return out.str();
}

std::string make_login_post_request(const ParsedVpnUrl &server,
                                    const std::string &useragent,
                                    const std::string &username,
                                    const std::string &encoded_password,
                                    const std::string &cookie_header) {
  const std::string body = "username=" + form_url_encode(username) +
                           "&password=" + encoded_password;

  std::ostringstream out;
  out << "POST " << kLoginPath << " HTTP/1.1\r\n";
  out << "Host: " << host_header(server) << "\r\n";
  out << "User-Agent: " << useragent_or_default(useragent) << "\r\n";
  out << "Content-Type: application/x-www-form-urlencoded; charset=utf-8\r\n";
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

