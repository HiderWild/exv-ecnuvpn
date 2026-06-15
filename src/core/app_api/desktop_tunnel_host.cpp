#include "core/app_api/desktop_tunnel_host.hpp"

#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_controller_active.hpp"
#include "helper/common/helper_connector.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"
#include "platform/common/process_utils.hpp"

#include <chrono>
#include <filesystem>
#include <memory>

namespace ecnuvpn {
namespace app_api {
namespace {

struct TunnelControllerHolder {
  std::unique_ptr<exv::helper::HelperConnector> connector;
  std::shared_ptr<exv::helper::HelperClient> client;
  std::shared_ptr<exv::platform::HelperDelegatingPlatformNetworkOps> net_ops;
  std::shared_ptr<exv::core::TunnelController> controller;
  bool init_attempted = false;
  std::string init_error;
  std::chrono::steady_clock::time_point last_failure_time;
  static constexpr auto kRetryCooldown = std::chrono::seconds(30);
};

TunnelControllerHolder &tunnel_holder() {
  static TunnelControllerHolder holder;
  return holder;
}

} // namespace

std::string helper_binary_next_to_exv() {
  std::filesystem::path exv_path(platform::get_executable_path());
#ifdef _WIN32
  return (exv_path.parent_path() / "exv-helper.exe").string();
#else
  return (exv_path.parent_path() / "exv-helper").string();
#endif
}

std::shared_ptr<exv::core::TunnelController>
ensure_tunnel_controller(const std::string &endpoint_override) {
  auto &h = tunnel_holder();
  if (h.controller) {
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  }

  const auto now = std::chrono::steady_clock::now();
  if (h.last_failure_time != std::chrono::steady_clock::time_point{}) {
    const auto elapsed = std::chrono::duration_cast<std::chrono::seconds>(
        now - h.last_failure_time);
    if (elapsed < TunnelControllerHolder::kRetryCooldown) {
      return nullptr;
    }
  }

  h.init_attempted = true;
  try {
    h.connector = exv::helper::HelperConnector::create();
    exv::helper::HelperConnectorConfig cc;
    cc.mode = exv::helper::ConnectorMode::Transient;
    if (!endpoint_override.empty()) {
      cc.pipe_endpoint = endpoint_override;
    } else {
      cc.helper_executable_path = helper_binary_next_to_exv();
    }
    h.client = h.connector->connect(cc);
    if (!h.client) {
      h.init_error = "Failed to connect to helper daemon";
      h.last_failure_time = now;
      return nullptr;
    }
    h.net_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            h.client.get());
    h.controller =
        std::make_shared<exv::core::TunnelController>(h.client, h.net_ops);
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  } catch (const std::exception &e) {
    h.init_error = e.what();
    h.last_failure_time = now;
    return nullptr;
  }
}

std::shared_ptr<exv::core::TunnelController>
get_tunnel_controller_if_exists() {
  return tunnel_holder().controller;
}

void reset_tunnel_controller() {
  auto &h = tunnel_holder();
  h.controller.reset();
  h.client.reset();
  h.net_ops.reset();
  h.connector.reset();
  h.init_attempted = false;
  h.init_error.clear();
  h.last_failure_time = std::chrono::steady_clock::time_point{};
  exv::core::set_tunnel_controller_active(false);
}

std::string tunnel_controller_init_error() {
  return tunnel_holder().init_error;
}

} // namespace app_api
} // namespace ecnuvpn
