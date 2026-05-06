#pragma once

#include "config.hpp"

#include <string>

namespace ecnuvpn {
namespace helper {

bool is_available();
bool start_via_helper(const Config &cfg, const std::string &plaintext_password,
                      int retry_limit);
bool stop_via_helper();
bool show_status_via_helper();

int install_service(const std::string &executable_path);
int uninstall_service();
int show_service_status();

int daemon_main();
int worker_main(const std::string &request_path);

} // namespace helper
} // namespace ecnuvpn
