#pragma once

#include <string>

namespace exv::core::lifecycle {

std::string ipc_protocol_name();

std::string core_ipc_path();
std::string core_ipc_path(const std::string& state_dir);

std::string core_lock_path();
std::string core_lock_path(const std::string& state_dir);

std::string core_registry_path();
std::string core_registry_path(const std::string& state_dir);

} // namespace exv::core::lifecycle
