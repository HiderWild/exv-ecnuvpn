#pragma once

#include <nlohmann/json.hpp>

namespace exv {
namespace app_api {

void apply_desktop_runtime_context(const nlohmann::json &payload);
void add_desktop_owner_context(nlohmann::json &request);

} // namespace app_api
} // namespace exv
