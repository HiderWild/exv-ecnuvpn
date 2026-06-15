#pragma once

#include <optional>
#include <string>

namespace exv::base {

namespace error_domains {
inline constexpr const char *Transport = "transport";
inline constexpr const char *Auth = "auth";
inline constexpr const char *Helper = "helper";
inline constexpr const char *Platform = "platform";
inline constexpr const char *Config = "config";
inline constexpr const char *Packet = "packet";
} // namespace error_domains

namespace error_codes {
inline constexpr const char *InvalidConfig = "config_invalid";
inline constexpr const char *UnknownAction = "unknown_action";
inline constexpr const char *HelperUnavailable = "helper_unavailable";
inline constexpr const char *TransportClosed = "transport_closed";
inline constexpr const char *AuthFailed = "auth_failed";
} // namespace error_codes

struct ErrorInfo {
  std::string domain;
  std::string code;
  std::string message;
  std::optional<int> native_code;
  std::string native_api;
  bool recoverable = false;
  std::string recommended_action;
};

} // namespace exv::base
