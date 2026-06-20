#pragma once

#include <string>

namespace exv {
namespace virtual_network {

struct AdapterInfo {
  std::string name;
  std::string detail;
  std::string kind;
  std::string role;
  std::string if_index;
  std::string route_reason;
};

} // namespace virtual_network
} // namespace exv
