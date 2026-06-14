#include "core_error_mapper.hpp"

namespace exv::core {

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
