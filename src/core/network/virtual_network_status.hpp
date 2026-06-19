#pragma once

#include <nlohmann/json.hpp>

#include "platform/common/virtual_network_model.hpp"

#include <string>
#include <vector>

namespace ecnuvpn {
namespace virtual_network {

struct Detection {
  bool detected = false;
  std::vector<AdapterInfo> adapters;
  std::string message;
};

Detection detect_upstream_virtual_network(const std::string &exv_interface = "");
nlohmann::json to_json(const Detection &detection);
void add_status_fields(nlohmann::json &status, const std::string &exv_interface = "");
void print_notice(const Detection &detection);

namespace testing {
using VirtualNetworkProbeProvider =
    std::vector<AdapterInfo> (*)(const std::string &exv_interface);
void set_virtual_network_probe_provider_for_testing(
    VirtualNetworkProbeProvider provider);
} // namespace testing

} // namespace virtual_network
} // namespace ecnuvpn
