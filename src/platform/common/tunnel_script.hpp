#pragma once

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

struct TunnelScriptContext {
  std::string vpn_engine = "legacy_openconnect";
  std::string route_ready_path;
  std::vector<std::string> custom_routes;
  std::vector<std::string> server_route_exceptions;
  int configured_mtu = 0;
  bool has_runtime_owner = false;
  unsigned int runtime_owner_uid = 0;
  unsigned int runtime_owner_gid = 0;
};

struct OpenconnectLogConfigureResult {
  bool ok = false;
  std::string code;
};

std::string generate_tunnel_script(const TunnelScriptContext &context);
int run_tunnel_script(const TunnelScriptContext &context);
OpenconnectLogConfigureResult
configure_from_openconnect_log(const TunnelScriptContext &context,
                               const std::string &log_path);
void cleanup_tunnel_routes(const TunnelScriptContext &context);

} // namespace platform
} // namespace ecnuvpn
