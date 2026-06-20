#pragma once

namespace exv::core_api {
class DesktopRpcAdapter;
}

namespace exv {
namespace app_api {

void register_desktop_config_actions(exv::core_api::DesktopRpcAdapter &adapter);

} // namespace app_api
} // namespace exv
