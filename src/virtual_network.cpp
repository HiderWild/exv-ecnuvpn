#include "virtual_network.hpp"

#include "platform/common/virtual_network_probe.hpp"
#include "utils.hpp"

#include <string>

namespace ecnuvpn {
namespace virtual_network {
namespace {

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
        "发现其他虚拟网卡（" + join_adapter_names(detection->adapters) +
        "），正在把 EXV 串联到它们前面提前路由校园流量；其他流量继续按系统默认出口处理。";
  }
}

} // namespace

Detection detect_upstream_virtual_network(const std::string &exv_interface) {
  Detection detection;
  detection.adapters = platform::detect_virtual_network_adapters(exv_interface);
  finalize_detection(&detection);
  return detection;
}

nlohmann::json to_json(const Detection &detection) {
  nlohmann::json adapters = nlohmann::json::array();
  for (const auto &adapter : detection.adapters) {
    adapters.push_back({{"name", adapter.name}, {"detail", adapter.detail}});
  }
  return nlohmann::json{{"detected", detection.detected},
                        {"adapters", adapters},
                        {"message", detection.message},
                        {"route_policy", detection.detected
                                             ? "exv-campus-routes-first"
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
  utils::print_warning(detection.message);
}

} // namespace virtual_network
} // namespace ecnuvpn
