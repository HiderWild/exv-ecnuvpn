#include "core_error_mapper.hpp"

#include <algorithm>
#include <cctype>
#include <sstream>

namespace exv::core {

namespace {

std::string to_lower_ascii(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
        return static_cast<char>(std::tolower(c));
    });
    return value;
}

bool contains_ci(const std::string& value, const std::string& needle) {
    return to_lower_ascii(value).find(to_lower_ascii(needle)) != std::string::npos;
}

void replace_all(std::string& text, const std::string& needle,
                 const std::string& replacement) {
    if (needle.empty()) {
        return;
    }
    std::size_t pos = 0;
    while ((pos = text.find(needle, pos)) != std::string::npos) {
        text.replace(pos, needle.size(), replacement);
        pos += replacement.size();
    }
}

void redact_prefixed_value(std::string& text, const std::string& marker) {
    std::string lower = to_lower_ascii(text);
    const std::string lower_marker = to_lower_ascii(marker);
    std::size_t pos = lower.find(lower_marker);
    while (pos != std::string::npos) {
        std::size_t end = pos + lower_marker.size();
        while (end < text.size() && text[end] != ' ' && text[end] != '&' &&
               text[end] != ';' && text[end] != '\r' && text[end] != '\n') {
            ++end;
        }
        text.replace(pos, end - pos, marker + "[REDACTED]");
        lower = to_lower_ascii(text);
        pos = lower.find(lower_marker, pos + marker.size() + 10);
    }
}

std::string redact_sensitive_text(std::string text) {
    auto find_url = [](const std::string& value, std::size_t start) {
        const std::size_t https = value.find("https://", start);
        const std::size_t http = value.find("http://", start);
        if (https == std::string::npos) {
            return http;
        }
        if (http == std::string::npos) {
            return https;
        }
        return std::min(https, http);
    };

    std::size_t url_pos = find_url(text, 0);
    while (url_pos != std::string::npos) {
        std::size_t end = url_pos;
        while (end < text.size() && !std::isspace(static_cast<unsigned char>(text[end]))) {
            ++end;
        }
        text.replace(url_pos, end - url_pos, "[REDACTED_URL]");
        url_pos = find_url(text, url_pos + 14);
    }

    redact_prefixed_value(text, "password=");
    redact_prefixed_value(text, "token=");
    redact_prefixed_value(text, "cookie=");
    redact_prefixed_value(text, "webvpn=");
    redact_prefixed_value(text, "SAMLRequest=");

    std::istringstream in(text);
    std::ostringstream out;
    std::string word;
    bool first = true;
    while (in >> word) {
        if (!first) {
            out << ' ';
        }
        first = false;
        if (contains_ci(word, "secret")) {
            out << "[REDACTED]";
        } else {
            out << word;
        }
    }
    const std::string redacted = out.str();
    return redacted.empty() && !text.empty() ? "[REDACTED]" : redacted;
}

ErrorInfo native_error(std::string domain, std::string code, std::string message,
                       bool recoverable, std::string action) {
    ErrorInfo info;
    info.domain = std::move(domain);
    info.code = code.empty() ? "native_error" : std::move(code);
    info.message = redact_sensitive_text(message.empty() ? info.code : message);
    info.recoverable = recoverable;
    info.recommended_action = std::move(action);
    return info;
}

} // namespace

ErrorInfo CoreErrorMapper::from_transport_error(int native_code, const std::string& api) {
    ErrorInfo info;
    info.domain = "transport";
    info.code = "transport_closed";
    info.message = "Transport connection closed";
    info.native_code = native_code;
    info.native_api = api;
    info.recoverable = true;
    info.recommended_action = "Retry connection";
    return info;
}

ErrorInfo CoreErrorMapper::from_auth_error(int native_code, const std::string& detail) {
    ErrorInfo info;
    info.domain = "auth";
    info.code = "auth_failed";
    info.message = "Authentication failed: " + detail;
    info.native_code = native_code;
    info.recoverable = false;
    info.recommended_action = "Check credentials and try again";
    return info;
}

ErrorInfo CoreErrorMapper::from_native_error(const std::string& code, const std::string& message) {
    const std::string hint = to_lower_ascii(code + " " + message);

    if (code == "tls_verify_failed" || contains_ci(hint, "certificate") ||
        contains_ci(hint, "tls verify")) {
        return native_error("security", "tls_verify_failed", message, false,
                            "Verify the VPN server certificate and network trust settings");
    }
    if (code == "saml_required_unsupported") {
        return native_error("auth", code, message, false,
                            "Use a supported browser-based SSO flow or administrator-provided native profile");
    }
    if (code == "auth_failed" || contains_ci(hint, "auth")) {
        return native_error("auth", "auth_failed", message, false,
                            "Check credentials and try again");
    }
    if (code == "dtls_fell_back_to_tls") {
        return native_error("transport", code, message, true,
                            "Continue on CSTP/TLS or disable DTLS for this profile");
    }
    if (contains_ci(hint, "dns") || contains_ci(hint, "network") ||
        contains_ci(hint, "timeout") || contains_ci(hint, "unreachable")) {
        return native_error("transport", "network_unreachable", message, true,
                            "Check network connectivity and retry");
    }
    if (code == "protocol_error" || contains_ci(hint, "parse") ||
        contains_ci(hint, "http_invalid")) {
        return native_error("protocol", "protocol_error", message, false,
                            "Capture sanitized protocol diagnostics and report the gateway behavior");
    }

    return native_error("native", code, message, false,
                        "Review native engine diagnostics");
}

ErrorInfo CoreErrorMapper::from_helper_error(const std::string& code, const std::string& message) {
    ErrorInfo info;
    info.domain = "helper";
    info.code = code;
    info.message = message;
    info.recoverable = (code != "permission_denied");
    info.recommended_action = "Check helper service status";
    return info;
}

ErrorInfo CoreErrorMapper::from_platform_error(const std::string& domain, int native_code, const std::string& api) {
    ErrorInfo info;
    info.domain = domain;
    info.code = domain + "_failed";
    info.message = "Platform operation failed: " + api;
    info.native_code = native_code;
    info.native_api = api;
    info.recoverable = false;
    info.recommended_action = "Check system configuration";
    return info;
}

bool CoreErrorMapper::is_recoverable(const ErrorInfo& error) {
    return error.recoverable;
}

} // namespace exv::core
