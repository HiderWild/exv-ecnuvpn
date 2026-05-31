#pragma once

#include <nlohmann/json.hpp>

#include <string>
#include <vector>

namespace ecnuvpn {
namespace virtual_network {

struct AdapterInfo {
  std::string name;
  std::string detail;
  std::string kind;
  std::string role;
  std::string if_index;
  std::string route_reason;
};

struct Detection {
  bool detected = false;
  std::vector<AdapterInfo> adapters;
  std::string message;
};

Detection detect_upstream_virtual_network(const std::string &exv_interface = "");
nlohmann::json to_json(const Detection &detection);
void add_status_fields(nlohmann::json &status, const std::string &exv_interface = "");
void print_notice(const Detection &detection);

} // namespace virtual_network
} // namespace ecnuvpn
