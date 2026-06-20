#include "core/app_api/desktop_json.hpp"

#include "feedback/feedback.hpp"
#include "platform/common/helper_client.hpp"

namespace exv {
namespace app_api {

nlohmann::json error(const std::string &message, const std::string &code) {
  feedback::ErrorInfo info = feedback::lookup_error(code, message);
  return nlohmann::json{{"ok", false},
                        {"error", message},
                        {"code", info.code},
                        {"recoverable", info.recoverable},
                        {"recommended_action", info.recommended_action}};
}

bool helper_unavailable(const nlohmann::json &response) {
  return json_string(response, "code") == platform::kHelperUnavailableCode ||
         json_string(response, "message") == "Helper daemon not available";
}

bool json_bool(const nlohmann::json &object, const char *key, bool fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null()) {
    return fallback;
  }
  if (object[key].is_boolean()) {
    return object[key].get<bool>();
  }
  return fallback;
}

int json_int(const nlohmann::json &object, const char *key, int fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null()) {
    return fallback;
  }
  if (object[key].is_number_integer()) {
    return object[key].get<int>();
  }
  return fallback;
}

uint64_t json_u64(const nlohmann::json &object, const char *key,
                  uint64_t fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null()) {
    return fallback;
  }
  if (object[key].is_number_unsigned()) {
    return object[key].get<uint64_t>();
  }
  if (object[key].is_number_integer()) {
    const int64_t value = object[key].get<int64_t>();
    return value < 0 ? fallback : static_cast<uint64_t>(value);
  }
  return fallback;
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null()) {
    return fallback;
  }
  if (object[key].is_string()) {
    return object[key].get<std::string>();
  }
  return fallback;
}

nlohmann::json helper_error(const nlohmann::json &response,
                            const std::string &fallback_message) {
  return error(json_string(response, "message", fallback_message),
               json_string(response, "code"));
}

std::string json_safe_text(const std::string &text) {
  std::string out;
  out.reserve(text.size());
  for (unsigned char c : text) {
    if (c == '\t' || c == '\n' || c == '\r' || (c >= 0x20 && c < 0x80)) {
      out.push_back(static_cast<char>(c));
    } else {
      out.push_back('?');
    }
  }
  return out;
}

} // namespace app_api
} // namespace exv
