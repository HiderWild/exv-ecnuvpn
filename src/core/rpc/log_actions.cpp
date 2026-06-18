#include "core/rpc/log_actions.hpp"

#include "platform/common/file_system.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/runtime_paths.hpp"
#include "runtime/runtime_context.hpp"

#include <nlohmann/json.hpp>

#include <fstream>
#include <string>
#include <vector>

namespace exv::core_api {
namespace {

using json = nlohmann::json;

json parse_payload(const RpcRequest &req) {
  if (req.payload_json.empty()) {
    return json::object();
  }
  return json::parse(req.payload_json);
}

RpcResponse success_response(const json &payload) {
  RpcResponse resp;
  resp.success = true;
  resp.payload_json = payload.dump();
  return resp;
}

RpcResponse error_response(const std::string &code, const std::string &message) {
  RpcResponse resp;
  resp.success = false;
  resp.error_code = code;
  resp.error_message = message;
  return resp;
}

void prepare_log_runtime() {
  ecnuvpn::platform::ensure_dir(ecnuvpn::platform::get_config_dir());
  ecnuvpn::platform::logging::configure_default_logging(false);
}

std::string log_safe_text(std::string value) {
  for (char &ch : value) {
    if (static_cast<unsigned char>(ch) < 0x20 && ch != '\t') {
      ch = ' ';
    }
  }
  return value;
}

json read_log_lines(const json &payload) {
  prepare_log_runtime();

  const std::string log_path = ecnuvpn::runtime::paths().log_path;
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1) {
    max_lines = 1;
  }
  if (max_lines > 10000) {
    max_lines = 10000;
  }
  const std::string filter = payload.value("filter", std::string());

  json lines = json::array();
  std::vector<std::string> all_lines;
  std::ifstream ifs(log_path);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    if (filter.empty() || line.find(filter) != std::string::npos) {
      all_lines.push_back(line);
    }
  }

  const std::size_t start =
      all_lines.size() > static_cast<std::size_t>(max_lines)
          ? all_lines.size() - static_cast<std::size_t>(max_lines)
          : 0;
  for (std::size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", log_safe_text(all_lines[i])}});
  }
  return lines;
}

json clear_log_file() {
  prepare_log_runtime();

  const std::string log_path = ecnuvpn::runtime::paths().log_path;
  std::ofstream out(log_path, std::ios::out | std::ios::trunc);
  if (!out.is_open()) {
    return {{"ok", false},
            {"code", "log_clear_failed"},
            {"message", "Unable to clear the runtime log."}};
  }
  out.close();
  return {{"ok", true},
          {"cleared", true},
          {"logDirectory", "<redacted>"},
          {"filesRemoved", 0}};
}

} // namespace

void LogActions::register_handlers(AppRpcDispatcher &dispatcher) {
  dispatcher.register_handler("logs.list",
                              [this](const RpcRequest &req) { return list(req); });
  dispatcher.register_handler("logs.clear",
                              [this](const RpcRequest &req) { return clear(req); });
}

RpcResponse LogActions::list(const RpcRequest &req) {
  try {
    return success_response(read_log_lines(parse_payload(req)));
  } catch (const std::exception &e) {
    return error_response("invalid_payload", e.what());
  }
}

RpcResponse LogActions::clear(const RpcRequest &req) {
  try {
    (void)parse_payload(req);
    const auto result = clear_log_file();
    if (!result.value("ok", false)) {
      return error_response(result.value("code", "log_clear_failed"),
                            result.value("message", "Unable to clear the runtime log."));
    }
    return success_response(result);
  } catch (const std::exception &e) {
    return error_response("invalid_payload", e.what());
  }
}

} // namespace exv::core_api
