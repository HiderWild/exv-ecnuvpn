#pragma once

#include "config.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/protocol/native_authenticator.hpp"

#include <nlohmann/json.hpp>

#include <functional>
#include <string>

namespace ecnuvpn {
namespace app_api {

struct NativeAuthFirstInputs {
  Config config;
  std::string password;
  std::string home;
  std::string config_dir;
  int retry_limit = 0;
};

struct NativeAuthFirstDeps {
  std::function<vpn_engine::ValidationResult(
      const Config &, const std::string &,
      vpn_engine::protocol::NativeAuthSession *)>
      authenticate;
  std::function<vpn_engine::ValidationResult(const nlohmann::json &)>
      send_helper_start;
  std::function<vpn_engine::ValidationResult()>
      ensure_service_available_or_start_oneshot;
};

vpn_engine::ValidationResult
orchestrate_native_auth_first(const NativeAuthFirstInputs &inputs,
                              const NativeAuthFirstDeps &deps);

vpn_engine::ValidationResult build_native_user_mode_auth_request(
    const Config &cfg, const std::string &password,
    vpn_engine::protocol::NativeAuthRequest *out);

vpn_engine::ValidationResult authenticate_native_user_mode(
    const Config &cfg, const std::string &password,
    vpn_engine::protocol::NativeAuthSession *session);

} // namespace app_api
} // namespace ecnuvpn
