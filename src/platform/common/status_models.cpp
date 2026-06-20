#include "platform/common/status_models.hpp"

namespace exv {
namespace platform {

std::string runtime_source_from_paths(const std::string &resolved_path,
                                      const std::string &bundled_path,
                                      const std::string &system_path) {
  if (resolved_path.empty())
    return "missing";
  if (!bundled_path.empty() && resolved_path == bundled_path)
    return "bundled";
  if (!system_path.empty() && resolved_path == system_path)
    return "system";
  return "custom";
}

nlohmann::json service_status_to_json(const ServiceStatusSnapshot &status) {
  nlohmann::json json{{"installed", status.installed},
                      {"running", status.running},
                      {"available", status.available},
                      {"capabilities", status.capabilities},
                      {"mode", status.mode},
                      {"path", status.path}};

  if (!status.endpoint.empty())
    json["endpoint"] = status.endpoint;
  if (!status.label.empty())
    json["label"] = status.label;
  if (!status.binary_path.empty())
    json["binary_path"] = status.binary_path;
  if (!status.warning.empty())
    json["warning"] = status.warning;
  if (status.has_service_state)
    json["service_state"] = status.service_state;

  return json;
}

} // namespace platform
} // namespace exv
