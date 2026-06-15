#include "app/ui_shell/host_bridge.hpp"

#include "contracts/generated/system_contract.hpp"

#include <algorithm>
#include <nlohmann/json.hpp>

namespace ecnuvpn::ui_shell {
namespace {

std::string error_response(int id, std::string_view code,
                           std::string_view message) {
  nlohmann::ordered_json out;
  out["id"] = id;
  out["ok"] = false;
  out["code"] = code;
  out["message"] = message;
  return out.dump();
}

} // namespace

bool is_allowed_host_action(std::string_view action) {
  const auto &actions = exv::contracts::generated::DESKTOP_RPC_ACTIONS;
  return std::find(actions.begin(), actions.end(), action) != actions.end();
}

std::string handle_host_request(const std::string &request_json,
                                const CoreRpcInvoker &invoke_core) {
  nlohmann::json parsed;
  try {
    parsed = nlohmann::json::parse(request_json);
  } catch (const nlohmann::json::exception &) {
    return error_response(0, "invalid_request", "Invalid desktop request");
  }

  const int id = parsed.value("id", 0);
  const std::string action = parsed.value("action", "");
  if (!is_allowed_host_action(action)) {
    return error_response(id, "unknown_action", "Unknown desktop action");
  }

  CoreRpcRequest request;
  request.action = action;
  request.payload_json = parsed.contains("payload") ? parsed.at("payload").dump()
                                                    : nlohmann::json::object().dump();
  request.request_id = std::to_string(id);

  const CoreRpcResponse response = invoke_core(request);
  nlohmann::ordered_json out;
  out["id"] = id;
  out["ok"] = response.ok;
  if (response.ok) {
    out["data"] = response.data_json.empty()
                      ? nlohmann::json::object()
                      : nlohmann::json::parse(response.data_json);
  } else {
    out["code"] = response.code;
    out["message"] = response.message;
  }
  return out.dump();
}

} // namespace ecnuvpn::ui_shell
