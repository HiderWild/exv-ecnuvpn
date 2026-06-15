#pragma once

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

struct ConfigView {
  std::string server;
  std::string username;
  int mtu = 1290;
  std::string useragent;
  bool disable_dtls = false;
  std::vector<std::string> extra_args;
  std::string log_file;
  std::string vpn_engine = "native";
  std::string openconnect_runtime = "bundled";
  std::string windows_tunnel_driver = "auto";
  std::string windows_tap_interface;
};

} // namespace platform
} // namespace ecnuvpn
