#include "core/lifecycle/core_paths.hpp"

#include "runtime/runtime_context.hpp"

#include <filesystem>

namespace exv::core::lifecycle {
namespace {

std::string state_leaf(const std::string& state_dir, const char* file_name) {
    return (std::filesystem::path(state_dir) / file_name).string();
}

} // namespace

std::string ipc_protocol_name() { return "ipc-v1"; }

std::string core_ipc_path() {
    return core_ipc_path(exv::runtime::paths().state_dir);
}

std::string core_ipc_path(const std::string& state_dir) {
#ifdef _WIN32
    (void)state_dir;
    return R"(\\.\pipe\exv-core-ipc-v1)";
#else
    return state_leaf(state_dir, "exv-core-ipc-v1.sock");
#endif
}

std::string core_lock_path() {
    return core_lock_path(exv::runtime::paths().state_dir);
}

std::string core_lock_path(const std::string& state_dir) {
    return state_leaf(state_dir, "exv-core-ipc-v1.lock");
}

std::string core_registry_path() {
    return core_registry_path(exv::runtime::paths().state_dir);
}

std::string core_registry_path(const std::string& state_dir) {
    return state_leaf(state_dir, "exv-core-ipc-v1.registry.json");
}

} // namespace exv::core::lifecycle
