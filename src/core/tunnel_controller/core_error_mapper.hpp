#pragma once
#include "tunnel_state.hpp"
#include <string>

namespace exv::core {

class CoreErrorMapper {
public:
    static ErrorInfo from_transport_error(int native_code, const std::string& api);
    static ErrorInfo from_auth_error(int native_code, const std::string& detail);
    static ErrorInfo from_helper_error(const std::string& code, const std::string& message);
    static ErrorInfo from_platform_error(const std::string& domain, int native_code, const std::string& api);
    static bool is_recoverable(const ErrorInfo& error);
};

} // namespace exv::core
