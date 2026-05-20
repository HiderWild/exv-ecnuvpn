#pragma once

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

struct TunnelScriptContext {
  std::string route_ready_path;
  std::vector<std::string> custom_routes;
  std::vector<std::string> server_route_exceptions;
  bool has_runtime_owner = false;
  unsigned int runtime_owner_uid = 0;
  unsigned int runtime_owner_gid = 0;
};

std::string generate_tunnel_script(const TunnelScriptContext &context);
void cleanup_tunnel_routes(const TunnelScriptContext &context);

} // namespace platform
} // namespace ecnuvpn