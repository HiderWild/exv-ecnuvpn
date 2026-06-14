#pragma once

#include "core/config/config.hpp"

#include <string>

namespace ecnuvpn {
namespace platform {

struct OpenconnectProcess {
  int pid = -1;
  void *wait_handle = nullptr;
};

bool spawn_openconnect_process(const Config &cfg, const std::string &password,
                               OpenconnectProcess *process);
void close_openconnect_process(OpenconnectProcess *process);

} // namespace platform
} // namespace ecnuvpn
