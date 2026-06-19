#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/network/virtual_network_status.hpp"

#include "platform/common/virtual_network_probe.hpp"
#include "cli/console.hpp"

#include <string>

namespace ecnuvpn {
namespace virtual_network {
namespace {

testing::VirtualNetworkProbeProvider &probe_provider_for_testing() {
  static testing::VirtualNetworkProbeProvider provider = nullptr;
  return provider;
}

std::string join_adapter_names(const std::vector<AdapterInfo> &adapters) {
  std::string joined;
  for (size_t i = 0; i < adapters.size(); ++i) {
    if (i > 0)
      joined += ", ";
    joined += adapters[i].name;
  }
  return joined;
}

void finalize_detection(Detection *detection) {
  if (!detection)
    return;
  detection->detected = !detection->adapters.empty();
  if (detection->detected) {
    detection->message =
        "发现代理 TUN（" + join_adapter_names(detection->adapters) +
        "），EXV 会插入到代理出口前方转发校园内网流量；其他流量继续按系统默认出口处理。";
  }
}

} // namespace

Detection detect_upstream_virtual_network(const std::string &exv_interface) {
  Detection detection;
  if (auto provider = probe_provider_for_testing()) {
    detection.adapters = provider(exv_interface);
  } else {
    detection.adapters = platform::detect_virtual_network_adapters(exv_interface);
  }
  finalize_detection(&detection);
  return detection;
}

nlohmann::json to_json(const Detection &detection) {
  nlohmann::json adapters = nlohmann::json::array();
  for (const auto &adapter : detection.adapters) {
    nlohmann::json item{{"name", adapter.name}, {"detail", adapter.detail}};
    if (!adapter.kind.empty())
      item["kind"] = adapter.kind;
    if (!adapter.role.empty())
      item["role"] = adapter.role;
    if (!adapter.if_index.empty())
      item["if_index"] = adapter.if_index;
    if (!adapter.route_reason.empty())
      item["route_reason"] = adapter.route_reason;
    adapters.push_back(item);
  }
  return nlohmann::json{{"detected", detection.detected},
                        {"adapters", adapters},
                        {"message", detection.message},
                        {"route_policy", detection.detected
                                             ? "exv-before-proxy-tun"
                                             : "normal"}};
}

void add_status_fields(nlohmann::json &status, const std::string &exv_interface) {
  Detection detection = detect_upstream_virtual_network(exv_interface);
  nlohmann::json j = to_json(detection);
  status["upstream_virtual_detected"] = j["detected"];
  status["upstream_virtual_adapters"] = j["adapters"];
  status["upstream_virtual_message"] = j["message"];
  status["route_policy"] = j["route_policy"];
}

void print_notice(const Detection &detection) {
  if (!detection.detected)
    return;
  cli::print_warning(detection.message);
}

namespace testing {

void set_virtual_network_probe_provider_for_testing(
    VirtualNetworkProbeProvider provider) {
  probe_provider_for_testing() = provider;
}

} // namespace testing

} // namespace virtual_network
} // namespace ecnuvpn
