#pragma once

#include "core/config/config.hpp"

#include <string>

namespace ecnuvpn {
namespace helper {

struct DaemonOptions {
  std::string mode = "service";
  std::string endpoint;
  std::string owner;
  int parent_pid = 0;
  bool oneshot = false;
  int first_request_timeout_ms = 5000;
};

bool is_available();

int install_service(const std::string &executable_path);
int uninstall_service();
int show_service_status();

int daemon_main(const DaemonOptions &options);
void request_daemon_stop();

} // namespace helper
} // namespace ecnuvpn
