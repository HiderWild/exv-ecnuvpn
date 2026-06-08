#pragma once

#include "config.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/protocol/native_authenticator.hpp"

#include <functional>
#include <optional>
#include <string>

namespace ecnuvpn {
namespace platform {

enum class SupervisorStartMode { password, auth_session };

struct SupervisorStartPayload {
  Config config;
  std::string password;
  int retry_limit = 0;
  std::string home;
  std::string config_dir;
  SupervisorStartMode native_start_mode = SupervisorStartMode::password;
  std::optional<vpn_engine::protocol::NativeAuthSession> auth_session;
};

using SupervisorEntryPoint = int (*)(const Config &, const std::string &, int);
using SupervisorPasswordRunner =
    std::function<int(const Config &, const std::string &, int)>;
using SupervisorAuthSessionRunner = std::function<int(
    const Config &, const vpn_engine::protocol::NativeAuthSession &, int)>;

vpn_engine::ValidationResult
encode_vpn_supervisor_payload(const SupervisorStartPayload &payload,
                              nlohmann::json *out);

vpn_engine::ValidationResult
parse_vpn_supervisor_payload(const std::string &payload,
                             SupervisorStartPayload *out);

vpn_engine::ValidationResult
decode_vpn_supervisor_payload(const nlohmann::json &payload,
                              SupervisorStartPayload *out);

nlohmann::json
summarize_vpn_supervisor_payload(const SupervisorStartPayload &payload);

int run_vpn_supervisor_payload(
    const SupervisorStartPayload &payload,
    const SupervisorPasswordRunner &password_runner,
    const SupervisorAuthSessionRunner &auth_session_runner);

bool spawn_vpn_supervisor_process(const Config &cfg,
                                  const std::string &password,
                                  int retry_limit,
                                  SupervisorEntryPoint entry_point,
                                  int *pid);

} // namespace platform
} // namespace ecnuvpn
