#include "platform/common/service_status.hpp"

#include "helper.hpp"
#include "platform/common/helper_platform.hpp"
#include "utils.hpp"

namespace ecnuvpn {
namespace platform {
namespace {

bool contains_token(const std::string &text, const std::string &token) {
  return !token.empty() && text.find(token) != std::string::npos;
}

} // namespace

ServiceStatusSnapshot current_service_status() {
  const auto &config = helper_platform_config();
  ServiceStatusSnapshot status;
  bool plist_exists = utils::file_exists(config.service_definition_path);
  std::string plist_content =
      plist_exists ? utils::read_file(config.service_definition_path) : "";
  bool helper_binary_exists =
      utils::file_exists(config.default_service_binary_path);
  bool helper_service_declared =
      contains_token(plist_content, config.default_service_binary_path) &&
      contains_token(plist_content, "--service");
  bool legacy_service_declared =
      contains_token(plist_content, config.stable_install_path) &&
      contains_token(plist_content, "__helper-daemon");

  status.installed =
      plist_exists && helper_service_declared && helper_binary_exists;
  status.available = helper::is_available();
  status.running = status.available;
  status.mode = config.service_mode;
  status.path = config.service_definition_path;
  status.endpoint = config.endpoint;
  status.label = config.service_label;
  if (helper_service_declared)
    status.binary_path = config.default_service_binary_path;
  else if (legacy_service_declared)
    status.binary_path = config.stable_install_path;

  if (plist_exists && legacy_service_declared) {
    status.warning =
        "Detected legacy launchd service pointing to /usr/local/bin/exv "
        "__helper-daemon. Reinstall the service to migrate to "
        "/usr/local/bin/exv-helper --service.";
  } else if (plist_exists && !helper_service_declared) {
    status.warning =
        "LaunchDaemon exists, but it does not point to "
        "/usr/local/bin/exv-helper --service.";
  } else if (helper_service_declared && !helper_binary_exists) {
    status.warning =
        "LaunchDaemon points to /usr/local/bin/exv-helper --service, but the "
        "stable exv-helper binary is missing.";
  }

  status.capabilities = nlohmann::json{{"service_mode", true},
                                       {"oneshot_mode", true},
                                       {"temporary_connect", true},
                                       {"direct_fallback", false},
                                       {"helper_binary", true}};
  return status;
}

} // namespace platform
} // namespace ecnuvpn
