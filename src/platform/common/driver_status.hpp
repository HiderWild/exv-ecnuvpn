#pragma once

#include "platform/common/config_view.hpp"

#include <nlohmann/json.hpp>
#include <functional>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

struct WindowsDriverAdapterSnapshot {
  std::vector<std::string> wintun_adapters;
  std::vector<std::string> tap_adapters;
};

nlohmann::json driver_status_json(const ConfigView &cfg);
nlohmann::json install_driver(const ConfigView &cfg,
                              const nlohmann::json &payload);
void invalidate_driver_status_cache();
void set_driver_status_adapter_snapshot_provider_for_testing(
    std::function<WindowsDriverAdapterSnapshot()> provider);
void clear_driver_status_cache_for_testing();

} // namespace platform
} // namespace ecnuvpn
