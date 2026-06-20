#pragma once

namespace exv {
namespace platform {

struct HelperPlatformConfig {
  const char *service_name;
  const char *service_label;
  const char *service_definition_path;
  const char *endpoint;
  const char *session_state_path;
  const char *stable_install_path;
  const char *default_service_binary_path;
  const char *service_mode;
};

const HelperPlatformConfig &helper_platform_config();
void wake_helper_daemon_for_shutdown();

} // namespace platform
} // namespace exv