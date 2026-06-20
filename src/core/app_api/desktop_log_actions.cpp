#include "core/app_api/desktop_log_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/rpc/log_actions.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"

#include <string>

namespace exv {
namespace app_api {
namespace {

nlohmann::json dispatch_log_action(const std::string &action,
                                   const nlohmann::json &payload) {
  exv::core_api::LogActions logs;
  exv::core_api::RpcRequest req;
  req.action = action;
  req.payload_json = payload.dump();

  exv::core_api::RpcResponse resp =
      action == "logs.clear" ? logs.clear(req) : logs.list(req);
  if (!resp.success) {
    return error(resp.error_message, resp.error_code);
  }
  try {
    return nlohmann::json::parse(resp.payload_json);
  } catch (const std::exception &e) {
    return error(e.what(), "invalid_payload");
  }
}

} // namespace

void register_desktop_log_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "logs.list", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return dispatch_log_action("logs.list", payload);
      });
  adapter.register_legacy_handler(
      "logs.clear", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return dispatch_log_action("logs.clear", payload);
      });
}

} // namespace app_api
} // namespace exv
