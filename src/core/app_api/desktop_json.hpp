#pragma once

#include <nlohmann/json.hpp>

#include <cstdint>
#include <string>

namespace exv {
namespace app_api {

nlohmann::json error(const std::string &message,
                     const std::string &code = std::string());
bool helper_unavailable(const nlohmann::json &response);
bool json_bool(const nlohmann::json &object, const char *key, bool fallback);
int json_int(const nlohmann::json &object, const char *key, int fallback);
uint64_t json_u64(const nlohmann::json &object, const char *key,
                  uint64_t fallback);
std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback = std::string());
nlohmann::json helper_error(const nlohmann::json &response,
                            const std::string &fallback_message);
std::string json_safe_text(const std::string &text);

} // namespace app_api
} // namespace exv
