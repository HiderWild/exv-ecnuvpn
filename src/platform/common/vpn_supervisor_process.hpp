#pragma once

#include "config.hpp"

#include <string>

namespace ecnuvpn {
namespace platform {

using SupervisorEntryPoint = int (*)(const Config &, const std::string &, int);

bool spawn_vpn_supervisor_process(const Config &cfg,
                                  const std::string &password,
                                  int retry_limit,
                                  SupervisorEntryPoint entry_point,
                                  int *pid);

} // namespace platform
} // namespace ecnuvpn