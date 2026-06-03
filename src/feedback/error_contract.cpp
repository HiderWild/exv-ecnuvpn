#include "error_contract.hpp"
#include <nlohmann/json.hpp>

using json = nlohmann::json;

namespace exv::feedback {

std::string ErrorInfo::to_json() const {
    json j = {
        {"domain", domain},
        {"code", code},
        {"message", message},
        {"native_api", native_api},
        {"recoverable", recoverable},
        {"recommended_action", recommended_action}
    };
    if (native_code.has_value()) {
        j["native_code"] = native_code.value();
    }
    return j.dump();
}

ErrorInfo ErrorInfo::from_json(const std::string& json_str) {
    auto j = json::parse(json_str);
    ErrorInfo info;
    info.domain = j.value("domain", "");
    info.code = j.value("code", "");
    info.message = j.value("message", "");
    info.native_api = j.value("native_api", "");
    info.recoverable = j.value("recoverable", false);
    info.recommended_action = j.value("recommended_action", "");
    if (j.contains("native_code") && !j["native_code"].is_null()) {
        info.native_code = j["native_code"].get<int>();
    }
    return info;
}

} // namespace exv::feedback
