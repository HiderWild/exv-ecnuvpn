#pragma once

#include "core/config/config.hpp"
#include "core/config/config_manager.hpp"
#include "core/use_cases/use_case_result.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace exv::core {

class ConfigUseCases {
public:
  ConfigUseCases();
  explicit ConfigUseCases(std::string config_dir);

  ecnuvpn::Config load_config();
  UseCaseResult get_config();
  UseCaseResult save_config(const nlohmann::json &payload);
  UseCaseResult get_profile(const nlohmann::json &payload);
  UseCaseResult save_profile(const nlohmann::json &payload);

  UseCaseResult get_auth();
  UseCaseResult save_auth(const nlohmann::json &payload);
  UseCaseResult get_settings();
  UseCaseResult save_settings(const nlohmann::json &payload);
  UseCaseResult get_key_status();

  UseCaseResult list_routes();
  UseCaseResult add_route(const nlohmann::json &payload);
  UseCaseResult remove_route(const nlohmann::json &payload);
  UseCaseResult reset_routes();
  UseCaseResult route_enable_unsupported();
  UseCaseResult route_disable_unsupported();

  UseCaseResult reset_config();
  UseCaseResult reset_key();

private:
  ecnuvpn::config::ConfigManager manager_;
};

} // namespace exv::core
