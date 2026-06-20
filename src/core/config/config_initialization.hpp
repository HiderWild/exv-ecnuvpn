#pragma once

#include "core/config/config.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace exv::config {

enum class ConfigInitializationStatus {
  Normal,
  Missing,
  Invalid,
};

struct ConfigInitializationResult {
  ConfigInitializationStatus status = ConfigInitializationStatus::Normal;
  Config config{};
  std::string reason = "normal";

  bool should_request_quick_start() const {
    return status == ConfigInitializationStatus::Missing ||
           status == ConfigInitializationStatus::Invalid;
  }
};

bool is_complete_initialized_config_json(const nlohmann::json &value,
                                         std::string *reason = nullptr);
ConfigInitializationResult ensure_initialized_config(
    const std::string &config_dir);
nlohmann::json quick_start_request_data(
    const ConfigInitializationResult &result);

} // namespace exv::config
