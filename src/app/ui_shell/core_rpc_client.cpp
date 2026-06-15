#include "app/ui_shell/core_rpc_client.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn::ui_shell {

CoreRpcResponse parse_core_rpc_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcResponse out;
  out.id = parsed.value("id", 0);
  out.ok = parsed.value("ok", false);
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  out.code = parsed.value("code", "");
  out.message = parsed.value("message", "");
  return out;
}

CoreRpcEvent parse_core_rpc_event_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcEvent out;
  out.event = parsed.value("event", "");
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  return out;
}

} // namespace ecnuvpn::ui_shell
