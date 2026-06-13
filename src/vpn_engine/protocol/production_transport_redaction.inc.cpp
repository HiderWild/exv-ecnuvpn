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

