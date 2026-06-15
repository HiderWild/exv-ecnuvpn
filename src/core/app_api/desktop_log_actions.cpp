#include "core/app_api/desktop_log_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "common/diagnostics/logger.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"
#include "runtime/runtime_context.hpp"

#include <fstream>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace app_api {
namespace {

nlohmann::json logs_json(const nlohmann::json &payload) {
  platform::ensure_dir(platform::get_config_dir());
  logger::init();

  const std::string log_path = runtime::paths().log_path;
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1) {
    max_lines = 1;
  }
  if (max_lines > 10000) {
    max_lines = 10000;
  }
  const std::string filter = payload.value("filter", std::string());

  nlohmann::json lines = nlohmann::json::array();
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

  const size_t start = all_lines.size() > static_cast<size_t>(max_lines)
                           ? all_lines.size() - static_cast<size_t>(max_lines)
                           : 0;
  for (size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", json_safe_text(all_lines[i])}});
  }
  return lines;
}

} // namespace

void register_desktop_log_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "logs.list", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        return logs_json(payload);
      });
}

} // namespace app_api
} // namespace ecnuvpn
