#pragma once
#include <string>
#include <optional>
#include <map>

namespace exv::feedback {

struct ErrorInfo {
    std::string domain;           // transport|auth|helper|os.route|os.dns|packet|config
    std::string code;             // Domain-specific error code
    std::string message;          // Human-readable message
    std::optional<int> native_code;  // OS/protocol error code
    std::string native_api;       // Which API failed
    bool recoverable = false;     // Should retry?
    std::string recommended_action;  // What user should do

    // Serialization
    std::string to_json() const;
    static ErrorInfo from_json(const std::string& json);
};

// Error code constants
namespace error_codes {
    // Transport errors
    constexpr const char* TRANSPORT_CLOSED = "transport_closed";
    constexpr const char* TRANSPORT_TIMEOUT = "transport_timeout";
    constexpr const char* TLS_ERROR = "tls_error";

    // Auth errors
    constexpr const char* AUTH_FAILED = "auth_failed";
    constexpr const char* CERT_ERROR = "cert_error";
    constexpr const char* CREDENTIAL_EXPIRED = "credential_expired";

    // Helper errors
    constexpr const char* HELPER_UNAVAILABLE = "helper_unavailable";
    constexpr const char* HELPER_PROTOCOL_REJECTED = "helper_protocol_rejected";
    constexpr const char* HELPER_TIMEOUT = "helper_timeout";

    // OS errors
    constexpr const char* ROUTE_FAILED = "route_failed";
    constexpr const char* DNS_FAILED = "dns_failed";
    constexpr const char* DEVICE_FAILED = "device_failed";
    constexpr const char* FIREWALL_FAILED = "firewall_failed";

    // Config errors
    constexpr const char* INVALID_CONFIG = "invalid_config";
    constexpr const char* PROFILE_NOT_FOUND = "profile_not_found";
} // namespace error_codes

// Error domain constants
namespace error_domains {
    constexpr const char* TRANSPORT = "transport";
    constexpr const char* AUTH = "auth";
    constexpr const char* HELPER = "helper";
    constexpr const char* OS_ROUTE = "os.route";
    constexpr const char* OS_DNS = "os.dns";
    constexpr const char* PACKET = "packet";
    constexpr const char* CONFIG = "config";
} // namespace error_domains

} // namespace exv::feedback
